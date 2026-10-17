use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const HOSTS: &[&str] = &[
    "pbs.twimg.com",
    "video.twimg.com",
    "abs.twimg.com",
    "xqimg.imedao.com",
    "xueqiuimg.com",
    "wx1.sinaimg.cn",
    "wx2.sinaimg.cn",
    "wx3.sinaimg.cn",
    "wx4.sinaimg.cn",
    "static-assets-1.truthsocial.com",
];
const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/gif"];
const VIDEO_TYPES: &[&str] = &["video/mp4", "video/quicktime", "video/webm"];
const IMAGE_MAX: usize = 10 * 1024 * 1024;
const VIDEO_MAX: usize = 60 * 1024 * 1024;
/// 单次 Range 请求最多向上游要这么多字节；浏览器会按 206 的 Content-Range 继续请求后续分片。
const VIDEO_CHUNK: u64 = 4 * 1024 * 1024;
const VIDEO_PROBE: u64 = 1024 * 1024;
const STREAM_CHUNK: usize = 64 * 1024;

/// 同时在途的上游请求上限。视频走流式转发，每个请求只占一个 64KB 缓冲。
pub static UPSTREAM: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);
const WINDOW: u64 = 60;
const IMAGE_QUOTA: u32 = 60;
const VIDEO_QUOTA: u32 = 180;

static QUOTA: Mutex<Option<HashMap<String, (u64, u32)>>> = Mutex::new(None);

#[derive(Clone)]
pub struct Target {
    pub url: String,
    pub host: String,
    pub video: bool,
}

pub struct Proxied {
    pub status: u16,
    pub media_type: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: ProxiedBody,
}

pub enum ProxiedBody {
    Full(Vec<u8>),
    Stream(Box<dyn Read + Send>),
}

impl ProxiedBody {
    #[cfg(test)]
    fn len_hint(&self) -> usize {
        match self {
            ProxiedBody::Full(body) => body.len(),
            ProxiedBody::Stream(_) => 0,
        }
    }
}

/// 客户端这么久不读走一个分块，就放弃这次转发并释放并发许可。
const CLIENT_IDLE: Duration = Duration::from_secs(15);
/// 单次转发（最多一个 4MB 分片）的总时长上限，防止极慢的客户端长期占槽。
const STREAM_LIFETIME: Duration = Duration::from_secs(300);

/// 在阻塞线程里按 64KB 分块读上游，经 channel 交给响应体；客户端断开、
/// 长时间不读或超过总时长时停止读取。`guard` 随读取线程一起释放（用来持有并发许可）。
pub fn stream_body<G: Send + 'static>(
    reader: Box<dyn Read + Send>,
    guard: G,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    stream_body_within(reader, guard, CLIENT_IDLE, STREAM_LIFETIME)
}

fn stream_body_within<G: Send + 'static>(
    mut reader: Box<dyn Read + Send>,
    guard: G,
    idle: Duration,
    lifetime: Duration,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let deadline = std::time::Instant::now() + lifetime;
        let send = |item| {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            !left.is_zero()
                && handle
                    .block_on(tx.send_timeout(item, idle.min(left)))
                    .is_ok()
        };
        loop {
            let mut buf = vec![0u8; STREAM_CHUNK];
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    buf.truncate(n);
                    if !send(Ok(buf)) {
                        break;
                    }
                }
                Err(err) => {
                    send(Err(err));
                    break;
                }
            }
        }
    });
    futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

#[derive(Debug)]
pub struct ImgError {
    pub status: u16,
    pub detail: &'static str,
    pub retry_after: Option<u64>,
}

pub fn validate(raw: &str) -> Result<Target, ImgError> {
    let url = raw.trim();
    if url.len() > 2048 || !url.to_ascii_lowercase().starts_with("https://") {
        return Err(bad());
    }
    let rest = &url[8..];
    if rest.contains('@') {
        return Err(bad());
    }
    let (authority, path) = rest.split_once(['/', '?', '#']).unwrap_or((rest, ""));
    let host = authority
        .rsplit_once(':')
        .map(|(host, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                host
            } else {
                authority
            }
        })
        .unwrap_or(authority);
    let host = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    if host.is_empty() || !HOSTS.contains(&host.as_str()) {
        return Err(bad());
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let video =
        path.to_ascii_lowercase().ends_with(".mp4") || path.to_ascii_lowercase().ends_with(".webm");
    Ok(Target {
        url: url.to_string(),
        host,
        video,
    })
}

pub fn resolution_ok(ips: &[IpAddr]) -> bool {
    !ips.is_empty() && ips.iter().all(|ip| ip_allowed(*ip))
}

pub fn ip_allowed(ip: IpAddr) -> bool {
    !crate::url_guard::ip_blocked(ip)
}

pub fn bounded_range(header: Option<&str>) -> String {
    let raw = header.unwrap_or("").trim();
    let lower = raw.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("bytes=") {
        if let Some(n) = rest.strip_prefix('-') {
            if let Ok(n) = n.parse::<u64>() {
                return format!("bytes=-{}", n.clamp(1, VIDEO_CHUNK));
            }
        } else if let Some((start, end)) = rest.split_once('-') {
            if let Ok(start) = start.parse::<u64>() {
                let end = if end.is_empty() {
                    start.saturating_add(VIDEO_PROBE - 1)
                } else {
                    end.parse::<u64>()
                        .unwrap_or(start)
                        .min(start.saturating_add(VIDEO_CHUNK - 1))
                };
                return format!("bytes={start}-{}", end.max(start));
            }
        }
    }
    format!("bytes=0-{}", VIDEO_PROBE - 1)
}

pub fn take_quota(video: bool, ip: &str, now: u64) -> Result<(), ImgError> {
    let limit = if video { VIDEO_QUOTA } else { IMAGE_QUOTA };
    let key = format!("{}:{ip}", if video { "video" } else { "image" });
    let window = now / WINDOW * WINDOW;
    let mut guard = QUOTA.lock().expect("img quota");
    let map = guard.get_or_insert_with(HashMap::new);
    map.retain(|_, (seen, _)| *seen >= window);
    let entry = map.entry(key).or_insert((window, 0));
    if entry.0 != window {
        *entry = (window, 0);
    }
    if entry.1 >= limit {
        return Err(ImgError {
            status: 429,
            detail: if video {
                "视频加载过于频繁，请稍后再试"
            } else {
                "图片加载过于频繁，请稍后再试"
            },
            retry_after: Some((WINDOW - now % WINDOW).max(1)),
        });
    }
    entry.1 += 1;
    Ok(())
}

#[allow(dead_code)]
pub fn reset_quota() {
    *QUOTA.lock().expect("img quota") = Some(HashMap::new());
}

pub fn fetch(target: &Target, range: Option<&str>) -> Result<Proxied, ImgError> {
    if target.host == "static-assets-1.truthsocial.com" && !target.video {
        return fetch_truth(target);
    }
    let builder = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .redirects(0);
    // 视频的 body 按客户端的速度读，整体超时会截断慢网用户；只限制连接和单次读的空闲时间。
    let agent = if target.video {
        builder
            .timeout_connect(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
    } else {
        builder.timeout(Duration::from_secs(15))
    }
    .build();
    let mut req = agent.get(&target.url).set(
        "User-Agent",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
    );
    if target.video {
        req = req.set("Accept", "*/*").set("Range", &bounded_range(range));
    } else {
        req = req.set("Referer", "https://weibo.com/");
    }
    let response = req.call().map_err(|_| upstream(target.video))?;
    let status = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let content_length = response
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok());
    let content_range = response.header("content-range").map(str::to_string);
    if !target.video {
        if status != 200 {
            return Err(upstream(false));
        }
        if !IMAGE_TYPES.contains(&content_type.as_str()) {
            return Err(ImgError {
                status: 400,
                detail: "非图片内容",
                retry_after: None,
            });
        }
        if content_length.is_some_and(|n| n > IMAGE_MAX) {
            return Err(ImgError {
                status: 400,
                detail: "图片过大",
                retry_after: None,
            });
        }
    } else {
        if status != 200 && status != 206 {
            return Err(upstream(true));
        }
        if !VIDEO_TYPES.contains(&content_type.as_str()) {
            return Err(ImgError {
                status: 400,
                detail: "非视频内容",
                retry_after: None,
            });
        }
    }
    if target.video {
        let limit = video_body_limit(status, content_length)?;
        let reader: Box<dyn Read + Send> = Box::new(response.into_reader().take(limit));
        return finish(
            true,
            status,
            content_type,
            content_length,
            content_range,
            ProxiedBody::Stream(reader),
        );
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(IMAGE_MAX as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| upstream(false))?;
    if body.len() > IMAGE_MAX {
        return Err(ImgError {
            status: 400,
            detail: "图片过大",
            retry_after: None,
        });
    }
    finish(
        false,
        status,
        content_type,
        content_length,
        content_range,
        ProxiedBody::Full(body),
    )
}

/// 上游忽略 Range 回一个超大的 206 时直接拒绝，免得把截断的数据连同原始
/// Content-Length 一起转给浏览器。
fn video_body_limit(status: u16, content_length: Option<usize>) -> Result<u64, ImgError> {
    let limit = if status == 206 {
        VIDEO_CHUNK
    } else {
        VIDEO_MAX as u64
    };
    if content_length.is_some_and(|n| n as u64 > limit) {
        return Err(ImgError {
            status: if status == 206 { 502 } else { 400 },
            detail: if status == 206 {
                "视频源请求失败"
            } else {
                "视频过大"
            },
            retry_after: None,
        });
    }
    Ok(limit)
}

fn finish(
    video: bool,
    status: u16,
    content_type: String,
    content_length: Option<usize>,
    content_range: Option<String>,
    body: ProxiedBody,
) -> Result<Proxied, ImgError> {
    if video {
        let media_type = if content_type == "video/quicktime" {
            "video/mp4"
        } else {
            &content_type
        }
        .to_string();
        let mut headers = vec![
            ("cache-control", "private, no-store".into()),
            ("vary", "Range".into()),
            ("accept-ranges", "bytes".into()),
        ];
        if let Some(range) = content_range {
            headers.push(("content-range", range));
        }
        if let Some(len) = content_length {
            headers.push(("content-length", len.to_string()));
        }
        Ok(Proxied {
            status,
            media_type,
            headers,
            body,
        })
    } else {
        Ok(Proxied {
            status: 200,
            media_type: content_type,
            headers: vec![("cache-control", "public, max-age=86400".into())],
            body,
        })
    }
}

pub fn resolve(host: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    (host, 443)
        .to_socket_addrs()
        .map(|iter| iter.map(|addr| addr.ip()).collect())
        .unwrap_or_default()
}

fn bad() -> ImgError {
    ImgError {
        status: 400,
        detail: "不支持的图片地址",
        retry_after: None,
    }
}

fn fetch_truth(target: &Target) -> Result<Proxied, ImgError> {
    let url = target.url.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| upstream(false))?;
    let (status, content_type, body) = runtime.block_on(async move {
        let response = chrome()
            .map_err(|_| upstream(false))?
            .get(url)
            .header("Referer", "https://truthsocial.com/")
            .header(
                "Accept",
                "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
            )
            .send()
            .await
            .map_err(|_| upstream(false))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let body = response
            .bytes()
            .await
            .map_err(|_| upstream(false))?
            .to_vec();
        Ok::<_, ImgError>((status, content_type, body))
    })?;
    if status != 200 {
        return Err(upstream(false));
    }
    if !IMAGE_TYPES.contains(&content_type.as_str()) {
        return Err(ImgError {
            status: 400,
            detail: "非图片内容",
            retry_after: None,
        });
    }
    if body.len() > IMAGE_MAX {
        return Err(ImgError {
            status: 400,
            detail: "图片过大",
            retry_after: None,
        });
    }
    finish(
        false,
        200,
        content_type,
        Some(body.len()),
        None,
        ProxiedBody::Full(body),
    )
}

fn chrome() -> Result<wreq::Client, String> {
    static CLIENT: OnceLock<Result<wreq::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        // ponytail: CF challenges ureq and Chrome124; Chrome137 is the newest profile here
        wreq::Client::builder()
            .emulation(wreq_util::Emulation::Chrome137)
            .redirect(wreq::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|err| err.to_string())
    }) {
        Ok(client) => Ok(client.clone()),
        Err(err) => Err(err.clone()),
    }
}

fn upstream(video: bool) -> ImgError {
    ImgError {
        status: 502,
        detail: if video {
            "视频源请求失败"
        } else {
            "图片源请求失败"
        },
        retry_after: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn rejects_unlisted_and_private_targets() {
        assert!(validate("http://pbs.twimg.com/a.jpg").is_err());
        assert!(validate("https://user:pass@pbs.twimg.com/a.jpg").is_err());
        assert!(validate("https://evil.example/a.jpg").is_err());
        assert!(validate("https://127.0.0.1/a.jpg").is_err());
        let image = validate("https://pbs.twimg.com/media/a.jpg").unwrap();
        assert_eq!(image.host, "pbs.twimg.com");
        assert!(!image.video);
        assert!(
            validate("https://video.twimg.com/ext/a.mp4?tag=1")
                .unwrap()
                .video
        );
        assert!(!resolution_ok(&[]));
        assert!(!resolution_ok(&[IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))]));
        assert!(!resolution_ok(&[IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))]));
        assert!(!ip_allowed("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!ip_allowed("64:ff9b::a00:1".parse().unwrap()));
        assert!(!ip_allowed(IpAddr::V4(Ipv4Addr::new(198, 18, 1, 1))));
        assert!(!ip_allowed(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!ip_allowed(IpAddr::V4(Ipv4Addr::new(0, 1, 2, 3))));
        assert!(!ip_allowed(IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))));
        assert!(!ip_allowed("ff02::1".parse().unwrap()));
        assert!(!ip_allowed("2002:7f00:1::".parse().unwrap()));
        assert!(!ip_allowed("2001:0::1".parse().unwrap()));
        assert!(ip_allowed(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn bounds_video_ranges_and_image_quota() {
        assert_eq!(
            bounded_range(Some("bytes=0-")),
            format!("bytes=0-{}", VIDEO_PROBE - 1)
        );
        assert_eq!(
            bounded_range(Some("bytes=-999999999")),
            format!("bytes=-{VIDEO_CHUNK}")
        );
        assert_eq!(bounded_range(None), format!("bytes=0-{}", VIDEO_PROBE - 1));
        assert_eq!(
            bounded_range(Some("bytes=0-99999999")),
            format!("bytes=0-{}", VIDEO_CHUNK - 1)
        );
        assert_eq!(bounded_range(Some("bytes=100-199")), "bytes=100-199");
        reset_quota();
        QUOTA
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .insert("image:expired".into(), (0, 1));
        take_quota(false, "1.2.3.4", 100).unwrap();
        assert!(!QUOTA
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains_key("image:expired"));
        for _ in 1..IMAGE_QUOTA {
            take_quota(false, "1.2.3.4", 100).unwrap();
        }
        let err = take_quota(false, "1.2.3.4", 100).unwrap_err();
        assert_eq!(err.status, 429);
        assert_eq!(err.detail, "图片加载过于频繁，请稍后再试");
        take_quota(false, "1.2.3.4", 160).unwrap();
    }

    #[test]
    #[ignore = "访问公网，不在 CI 中运行"]
    fn truth_jpeg_passes_cloudflare_with_chrome() {
        let target = validate("https://static-assets-1.truthsocial.com/tmtg:prime-ts-assets/media_attachments/files/117/345/153/898/008/432/original/3826b8f99055aa3b.jpg").unwrap();
        let got = fetch(&target, None).expect("truth image");
        assert_eq!(got.media_type, "image/jpeg");
        assert!(got.body.len_hint() > 2048);
    }

    #[tokio::test]
    async fn video_body_is_streamed_in_chunks_and_releases_the_permit() {
        use futures_util::StreamExt;
        let data: Vec<u8> = (0..(STREAM_CHUNK * 3 + 17)).map(|i| i as u8).collect();
        let reader: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(data.clone()));
        let before = UPSTREAM.available_permits();
        let permit = UPSTREAM.acquire().await.unwrap();
        assert_eq!(UPSTREAM.available_permits(), before - 1);
        let chunks: Vec<Vec<u8>> = stream_body(reader, permit)
            .map(|chunk| chunk.unwrap())
            .collect()
            .await;
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|chunk| chunk.len() <= STREAM_CHUNK));
        assert_eq!(chunks.concat(), data);
        for _ in 0..50 {
            if UPSTREAM.available_permits() == before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(UPSTREAM.available_permits(), before);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stalled_client_releases_the_permit() {
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                buf.fill(7);
                Ok(buf.len())
            }
        }
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = gate.clone().acquire_owned().await.unwrap();
        let stream = stream_body_within(
            Box::new(Endless),
            permit,
            Duration::from_millis(50),
            Duration::from_secs(60),
        );
        let mut released = false;
        for _ in 0..100 {
            if gate.available_permits() == 1 {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(released, "a client that never reads must not hold the slot");
        drop(stream);
    }

    #[test]
    fn oversized_partial_responses_are_rejected() {
        assert_eq!(video_body_limit(206, Some(1024)).unwrap(), VIDEO_CHUNK);
        assert_eq!(video_body_limit(206, None).unwrap(), VIDEO_CHUNK);
        let err = video_body_limit(206, Some(VIDEO_CHUNK as usize + 1)).unwrap_err();
        assert_eq!(err.status, 502);
        assert_eq!(
            video_body_limit(200, Some(VIDEO_MAX)).unwrap(),
            VIDEO_MAX as u64
        );
        assert_eq!(
            video_body_limit(200, Some(VIDEO_MAX + 1))
                .unwrap_err()
                .status,
            400
        );
    }
}
