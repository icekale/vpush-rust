use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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
const VIDEO_PROBE: u64 = 1024 * 1024;
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
    pub body: Vec<u8>,
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
    match unwrap_v4(ip) {
        Some(v4) => !ipv4_blocked(v4),
        None => !ipv6_blocked(match ip {
            IpAddr::V6(v6) => v6,
            IpAddr::V4(_) => return false,
        }),
    }
}

fn unwrap_v4(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().or_else(|| nat64(v6)),
    }
}

fn nat64(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let octets = v6.octets();
    if octets[..12] == [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0] {
        Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ))
    } else {
        None
    }
}

fn ipv4_blocked(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    if a == 198 && (b == 18 || b == 19) {
        return false;
    }
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

fn ipv6_blocked(ip: Ipv6Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (octets[0] & 0xfe) == 0xfc
        || (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80)
}

pub fn bounded_range(header: Option<&str>) -> String {
    let raw = header.unwrap_or("").trim();
    let lower = raw.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("bytes=") {
        if let Some(n) = rest.strip_prefix('-') {
            if let Ok(n) = n.parse::<u64>() {
                return format!("bytes=-{}", n.clamp(1, VIDEO_MAX as u64));
            }
        } else if let Some((start, end)) = rest.split_once('-') {
            if let Ok(start) = start.parse::<u64>() {
                let end = if end.is_empty() {
                    start.saturating_add(VIDEO_PROBE - 1)
                } else {
                    end.parse::<u64>()
                        .unwrap_or(start)
                        .min(start.saturating_add(VIDEO_MAX as u64 - 1))
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
    let agent = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout(Duration::from_secs(if target.video { 30 } else { 15 }))
        .redirects(0)
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
    let max = if target.video { VIDEO_MAX } else { IMAGE_MAX };
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
        if status == 200 && content_length.is_some_and(|n| n > VIDEO_MAX) {
            return Err(ImgError {
                status: 400,
                detail: "视频过大",
                retry_after: None,
            });
        }
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(max as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| upstream(target.video))?;
    if body.len() > max {
        return Err(ImgError {
            status: 400,
            detail: if target.video {
                "视频过大"
            } else {
                "图片过大"
            },
            retry_after: None,
        });
    }
    finish(
        target.video,
        status,
        content_type,
        content_length,
        content_range,
        body,
    )
}

fn finish(
    video: bool,
    status: u16,
    content_type: String,
    content_length: Option<usize>,
    content_range: Option<String>,
    body: Vec<u8>,
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
    finish(false, 200, content_type, Some(body.len()), None, body)
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
        assert!(ip_allowed(IpAddr::V4(Ipv4Addr::new(198, 18, 1, 1))));
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
            format!("bytes=-{VIDEO_MAX}")
        );
        assert_eq!(bounded_range(None), format!("bytes=0-{}", VIDEO_PROBE - 1));
        reset_quota();
        for _ in 0..IMAGE_QUOTA {
            take_quota(false, "1.2.3.4", 100).unwrap();
        }
        let err = take_quota(false, "1.2.3.4", 100).unwrap_err();
        assert_eq!(err.status, 429);
        assert_eq!(err.detail, "图片加载过于频繁，请稍后再试");
        take_quota(false, "1.2.3.4", 160).unwrap();
    }

    #[test]
    fn truth_jpeg_passes_cloudflare_with_chrome() {
        let target = validate("https://static-assets-1.truthsocial.com/tmtg:prime-ts-assets/media_attachments/files/117/345/153/898/008/432/original/3826b8f99055aa3b.jpg").unwrap();
        let got = fetch(&target, None).expect("truth image");
        assert_eq!(got.media_type, "image/jpeg");
        assert!(got.body.len() > 2048);
    }
}
