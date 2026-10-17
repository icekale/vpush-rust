//! 财经资讯的 RSS/Atom 抓取。只接受 https，不抓本机和内网地址。

use std::time::Duration;

use crate::db::{Db, NewsEntry};

pub fn spawn(db: Db) {
    crate::supervise::spawn("news", move || {
        let db = db.clone();
        async move {
            loop {
                if db.news_flag("news_enabled", false).await.unwrap_or(false) {
                    if let Err(err) = refresh_all(&db).await {
                        tracing::warn!("资讯抓取: {err}");
                        let _ = db.note_platform("news", Some(&err)).await;
                    } else {
                        let _ = db.note_platform("news", None).await;
                    }
                }
                let minutes = db
                    .setting("news_refresh_minutes")
                    .await
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(30);
                tokio::time::sleep(Duration::from_secs(minutes.clamp(5, 1440) * 60)).await;
            }
        }
    });
}

pub async fn refresh_all(db: &Db) -> Result<usize, String> {
    let feeds = db.news_feeds().await.map_err(|e| e.to_string())?;
    let mut added = 0;
    for (feed_id, source_id, _name, url) in feeds {
        added += pull_feed(db, feed_id, source_id, &url).await;
    }
    Ok(added)
}

pub async fn refresh_ids(db: &Db, ids: &[i64]) -> Result<serde_json::Value, String> {
    if !db
        .news_flag("news_enabled", false)
        .await
        .map_err(|e| e.to_string())?
    {
        return Err("财经资讯采集已关闭".into());
    }
    let mut accepted = Vec::new();
    for id in ids {
        let Some((source_id, url, enabled, archived)) =
            db.news_feed_target(*id).await.map_err(|e| e.to_string())?
        else {
            continue;
        };
        if !enabled || archived || url.is_empty() {
            continue;
        }
        pull_feed(db, *id, source_id, &url).await;
        accepted.push(*id);
    }
    Ok(serde_json::json!({"accepted_feed_ids": accepted, "busy_feed_ids": []}))
}

async fn pull_feed(db: &Db, feed_id: i64, source_id: i64, url: &str) -> usize {
    match fetch_feed(url).await {
        Ok(parsed) => match db
            .save_news_entries(source_id, feed_id, &parsed.entries)
            .await
        {
            Ok(n) => {
                let _ = db.note_feed_success(feed_id).await;
                n
            }
            Err(err) => {
                tracing::warn!(source = source_id, "资讯入库失败: {err}");
                let _ = db.note_feed_failure(feed_id, &err.to_string()).await;
                0
            }
        },
        Err(err) => {
            tracing::warn!(source = source_id, "资讯抓取失败: {err}");
            let _ = db.note_feed_failure(feed_id, &err).await;
            0
        }
    }
}

pub struct Parsed {
    pub format: String,
    pub title: String,
    pub entries: Vec<NewsEntry>,
}

pub async fn fetch_feed(url: &str) -> Result<Parsed, String> {
    public_https(url)?;
    let url = url.to_string();
    tokio::task::spawn_blocking(move || parse_feed(&http_text(&url)?))
        .await
        .map_err(|e| e.to_string())?
}

pub fn public_https(url: &str) -> Result<(), String> {
    crate::url_guard::validate_url(url, "https")
}

fn http_text(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(15))
        .redirects(0)
        .build();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    resp.into_string().map_err(|e| e.to_string())
}

pub fn parse_feed(xml: &str) -> Result<Parsed, String> {
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<rss") || lower.contains("<rdf") || lower.contains("<item") {
        let entries = blocks(xml, "item")
            .into_iter()
            .filter_map(rss_entry)
            .collect::<Vec<_>>();
        return Ok(Parsed {
            format: "RSS".into(),
            title: text_of(xml, "title"),
            entries,
        });
    }
    if lower.contains("<feed") || lower.contains("<entry") {
        let entries = blocks(xml, "entry")
            .into_iter()
            .filter_map(atom_entry)
            .collect::<Vec<_>>();
        return Ok(Parsed {
            format: "Atom".into(),
            title: text_of(xml, "title"),
            entries,
        });
    }
    Err("不是 RSS 或 Atom".into())
}

fn rss_entry(block: &str) -> Option<NewsEntry> {
    let title = plain(&text_of(block, "title"));
    if title.is_empty() {
        return None;
    }
    let url = web_url(&text_of(block, "link"));
    let guid = text_of(block, "guid");
    let body = text_of(block, "description");
    Some(NewsEntry {
        external_id: first_nonempty(&[&guid, &url, &title]),
        title,
        summary: plain(&body),
        content: plain(&body),
        url,
        author: plain(&{
            let creator = text_of(block, "dc:creator");
            if creator.is_empty() {
                text_of(block, "author")
            } else {
                creator
            }
        }),
        published_at: stamp(&text_of(block, "pubDate")),
    })
}

fn atom_entry(block: &str) -> Option<NewsEntry> {
    let title = plain(&text_of(block, "title"));
    if title.is_empty() {
        return None;
    }
    let url = web_url(&attr_of(block, "link", "href").unwrap_or_else(|| text_of(block, "link")));
    let body = {
        let summary = text_of(block, "summary");
        if summary.is_empty() {
            text_of(block, "content")
        } else {
            summary
        }
    };
    let id = text_of(block, "id");
    Some(NewsEntry {
        external_id: first_nonempty(&[&id, &url, &title]),
        title,
        summary: plain(&body),
        content: plain(&body),
        url,
        author: plain(&text_of(block, "name")),
        published_at: stamp(&{
            let updated = text_of(block, "updated");
            if updated.is_empty() {
                text_of(block, "published")
            } else {
                updated
            }
        }),
    })
}

fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    // ASCII lowercasing keeps byte offsets, so positions found in `lower` index `xml`.
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{tag}").to_ascii_lowercase();
    let close = format!("</{tag}>").to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find(&open) {
        let after = pos + start + open.len();
        let Some(end) = lower[after..].find(&close).map(|n| after + n) else {
            break;
        };
        let content_start = xml[after..]
            .find('>')
            .map(|n| after + n + 1)
            .unwrap_or(after);
        if content_start <= end {
            out.push(&xml[content_start..end]);
        }
        pos = end + close.len();
    }
    out
}

fn text_of(xml: &str, tag: &str) -> String {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{tag}").to_ascii_lowercase();
    let Some(start) = lower.find(&open) else {
        return String::new();
    };
    let after = &xml[start + open.len()..];
    let Some(end_bracket) = after.find('>') else {
        return String::new();
    };
    if after[..end_bracket].ends_with('/') {
        return String::new();
    }
    let body = &after[end_bracket + 1..];
    let close = format!("</{tag}>").to_ascii_lowercase();
    let Some(end) = body.to_ascii_lowercase().find(&close) else {
        return String::new();
    };
    decode(body[..end].trim())
}

fn attr_of(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{tag}").to_ascii_lowercase();
    let start = lower.find(&open)?;
    let after = &xml[start..];
    let end = after.find('>')?;
    let head = &after[..end];
    let key = format!("{attr}=");
    let at = head.to_ascii_lowercase().find(&key)? + key.len();
    let bytes = head.as_bytes();
    let quote = *bytes.get(at)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value = &head[at + 1..];
    let stop = value.find(quote as char)?;
    Some(decode(&value[..stop]))
}

pub(crate) fn plain(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('>')
            .map(|n| start + n)
            .unwrap_or(rest.len() - 1);
        let tag = rest[start..=end].to_ascii_lowercase();
        if tag.starts_with("<br") || tag.starts_with("</p") {
            out.push('\n');
        }
        rest = if end + 1 < rest.len() {
            &rest[end + 1..]
        } else {
            ""
        };
    }
    out.push_str(rest);
    decode(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 单遍解码：`&amp;lt;` 只解一层成 `&lt;`，不会被链式替换还原成 `<`。
fn decode(input: &str) -> String {
    let input = input.replace("<![CDATA[", "").replace("]]>", "");
    let mut out = String::with_capacity(input.len());
    let mut rest = input.as_str();
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest
            .get(1..)
            .and_then(|tail| tail.find(';').filter(|end| *end <= 10))
            .map(|end| &rest[1..1 + end]);
        let decoded = entity.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => {
                let code = if let Some(hex) =
                    name.strip_prefix("#x").or_else(|| name.strip_prefix("#X"))
                {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    name.strip_prefix('#').and_then(|dec| dec.parse().ok())
                };
                code.and_then(char::from_u32)
            }
        });
        match (entity, decoded) {
            (Some(name), Some(ch)) => {
                out.push(ch);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// 资讯正文进 `innerHTML` 前的白名单净化：去掉脚本、事件属性、内联样式，
/// 链接和图片只保留 http(s) 绝对地址。
pub(crate) fn sanitize_html(input: &str) -> String {
    ammonia::Builder::default()
        .url_schemes(std::collections::HashSet::from(["http", "https"]))
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(input)
        .to_string()
}

/// "打开原文"等外链只接受 http(s)。
pub(crate) fn web_url(raw: &str) -> String {
    let raw = raw.trim();
    let lower = raw.get(..8).unwrap_or(raw).to_ascii_lowercase();
    if (lower.starts_with("https://") || lower.starts_with("http://"))
        && raw.len() <= 2048
        && !raw.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        raw.to_string()
    } else {
        String::new()
    }
}

fn first_nonempty(values: &[&str]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .copied()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn stamp(raw: &str) -> String {
    utc_stamp(raw).unwrap_or_default()
}

const BEIJING_OFFSET_MIN: i64 = 8 * 60;

/// Canonical storage form for news timestamps: UTC `YYYY-MM-DDTHH:MM:SS+00:00`
/// (sorts lexically, `datetime()` understands it, the frontend parses it).
/// Accepts ISO 8601 / SQLite style (no offset means Beijing time, matching how
/// the frontend always displayed such values) and RFC 822/2822 dates.
pub(crate) fn utc_stamp(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (y, mo, d, h, mi, sec, offset) = parse_iso(raw).or_else(|| parse_rfc2822(raw))?;
    if !(1..=12).contains(&mo)
        || d == 0
        || d > days_in_month(y, mo)
        || h > 23
        || mi > 59
        || sec > 60
    {
        return None;
    }
    let local = days_from_civil(y, mo, d) * 86_400 + (h * 3600 + mi * 60 + sec.min(59)) as i64;
    let utc = local - offset * 60;
    let (y, mo, d) = civil_from_days(utc.div_euclid(86_400));
    let tod = utc.rem_euclid(86_400);
    Some(format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    ))
}

type Civil = (i64, u32, u32, u32, u32, u32, i64);

fn num(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn parse_offset(text: &str) -> Option<i64> {
    let text = text.trim();
    match text.to_ascii_uppercase().as_str() {
        "" => return Some(BEIJING_OFFSET_MIN),
        "Z" | "UT" | "UTC" | "GMT" => return Some(0),
        "EST" => return Some(-5 * 60),
        "EDT" => return Some(-4 * 60),
        "CST" => return Some(-6 * 60),
        "CDT" => return Some(-5 * 60),
        "MST" => return Some(-7 * 60),
        "MDT" => return Some(-6 * 60),
        "PST" => return Some(-8 * 60),
        "PDT" => return Some(-7 * 60),
        _ => {}
    }
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = text[1..].chars().filter(|c| *c != ':').collect();
    if digits.len() != 4 && digits.len() != 2 {
        return None;
    }
    let hours = num(&digits[..2])? as i64;
    let minutes = digits.get(2..).map(num).unwrap_or(Some(0))? as i64;
    Some(sign * (hours * 60 + minutes))
}

fn parse_iso(raw: &str) -> Option<Civil> {
    let bytes = raw.as_bytes();
    if raw.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (num(&raw[..4])? as i64, num(&raw[5..7])?, num(&raw[8..10])?);
    let rest = &raw[10..];
    if rest.is_empty() {
        return Some((y, mo, d, 0, 0, 0, BEIJING_OFFSET_MIN));
    }
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    let h = num(rest.get(..2)?)?;
    let rest = rest.get(2..)?.strip_prefix(':')?;
    let mi = num(rest.get(..2)?)?;
    let mut rest = rest.get(2..)?;
    let mut sec = 0;
    if let Some(tail) = rest.strip_prefix(':') {
        sec = num(tail.get(..2)?)?;
        rest = tail.get(2..)?;
        if let Some(frac) = rest.strip_prefix(['.', ',']) {
            let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
            rest = &frac[digits..];
        }
    }
    Some((y, mo, d, h, mi, sec, parse_offset(rest)?))
}

fn parse_rfc2822(raw: &str) -> Option<Civil> {
    let text = raw.split_once(',').map_or(raw, |(_, rest)| rest);
    let mut parts = text.split_whitespace();
    let d = num(parts.next()?)?;
    let month = parts.next()?.get(..3)?.to_ascii_lowercase();
    let mo = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| *m == month)? as u32
        + 1;
    let year = num(parts.next()?)? as i64;
    let y = match year {
        0..=49 => 2000 + year,
        50..=999 => 1900 + year,
        _ => year,
    };
    let clock: Vec<&str> = parts.next()?.split(':').collect();
    let (h, mi, sec) = match clock.as_slice() {
        [h, m] => (num(h)?, num(m)?, 0),
        [h, m, s] => (num(h)?, num(m)?, num(s)?),
        _ => return None,
    };
    let offset = match parts.next() {
        Some(zone) => parse_offset(zone)?,
        None => 0,
    };
    Some((y, mo, d, h, mi, sec, offset))
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/gif"];
const IMAGE_MAX: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub struct ArticleImage {
    pub body: Vec<u8>,
    pub media_type: String,
    pub etag: String,
}

#[derive(Debug)]
pub enum ImageFail {
    Unsafe,
    Upstream,
}

#[derive(Debug)]
pub struct ImageTarget {
    pub url: String,
    pub host: String,
}

pub fn image_target(raw: &str) -> Result<ImageTarget, ImageFail> {
    let url = raw.trim();
    if url.len() > 2048
        || !url.to_ascii_lowercase().starts_with("https://")
        || url[8..].contains('@')
    {
        return Err(ImageFail::Unsafe);
    }
    let rest = &url[8..];
    let authority = rest
        .split_once(['/', '?', '#'])
        .map(|(host, _)| host)
        .unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (host, Some(port)),
        _ => (authority, None),
    };
    if port.is_some_and(|port| port != "443")
        || host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
    {
        return Err(ImageFail::Unsafe);
    }
    let host = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    if public_https(&format!("https://{host}/")).is_err() {
        return Err(ImageFail::Unsafe);
    }
    Ok(ImageTarget {
        url: url.to_string(),
        host,
    })
}

pub fn fetch_image(url: &str) -> Result<ArticleImage, ImageFail> {
    use std::io::Read;
    let response = ureq::AgentBuilder::new()
        .resolver(crate::url_guard::public_resolver)
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build()
        .get(url)
        .call()
        .map_err(|_| ImageFail::Upstream)?;
    if !(200..300).contains(&response.status()) {
        return Err(ImageFail::Upstream);
    }
    let media_type = response
        .header("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !IMAGE_TYPES.contains(&media_type.as_str()) {
        return Err(ImageFail::Unsafe);
    }
    if response
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n > IMAGE_MAX)
    {
        return Err(ImageFail::Unsafe);
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(IMAGE_MAX as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| ImageFail::Upstream)?;
    if body.len() > IMAGE_MAX {
        return Err(ImageFail::Unsafe);
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&body);
    let etag = format!(
        "\"news-img-{}\"",
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    Ok(ArticleImage {
        body,
        media_type,
        etag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_item_and_rejects_private_urls() {
        let xml = r#"<rss><channel><title>甲</title><item><title>一条 &amp; 新闻</title><link>https://example.com/a</link><guid>a1</guid><description><![CDATA[<p>正文</p>]]></description><pubDate>2026-09-26T12:30:00Z</pubDate></item></channel></rss>"#;
        let parsed = parse_feed(xml).unwrap();
        assert_eq!(parsed.format, "RSS");
        assert_eq!(parsed.entries[0].title, "一条 & 新闻");
        assert_eq!(parsed.entries[0].summary, "正文");
        assert_eq!(parsed.entries[0].published_at, "2026-09-26T12:30:00+00:00");
        assert_eq!(parsed.entries[0].external_id, "a1");
        assert!(public_https("http://example.com/a").is_err());
        assert!(public_https("https://127.0.0.1/a").is_err());
        assert!(public_https("https://10.1.2.3/feed").is_err());
        assert!(public_https("https://example.com/feed").is_ok());
        assert!(image_target("http://img.example/a.jpg").is_err());
        assert!(image_target("https://user:pass@img.example/a.jpg").is_err());
        assert!(image_target("https://10.1.2.3/a.jpg").is_err());
        assert!(image_target("https://img.example:8443/a.jpg").is_err());
        assert_eq!(
            image_target("https://img.example/a.jpg").unwrap().host,
            "img.example"
        );
    }

    #[test]
    fn decode_is_single_pass() {
        assert_eq!(decode("&amp;lt;b&amp;gt;"), "&lt;b&gt;");
        assert_eq!(decode("a &amp; b &#39;c&#x27; &#20013;"), "a & b 'c' 中");
        assert_eq!(decode("AT&T &unknown; &"), "AT&T &unknown; &");
        assert_eq!(decode("%中&#xZZ;"), "%中&#xZZ;");
    }

    #[test]
    fn double_encoded_description_cannot_smuggle_markup() {
        let xml = r#"<rss><item><title>t</title><link>javascript:alert(1)</link><description>正文 &amp;amp;lt;img src=x onerror=alert(1)&amp;amp;gt; 结尾</description></item></rss>"#;
        let parsed = parse_feed(xml).unwrap();
        let entry = &parsed.entries[0];
        assert!(!entry.content.contains("<img"));
        assert_eq!(entry.url, "");
        let html = sanitize_html(&entry.content);
        assert!(!html.contains("<img"));
        assert!(!html.contains("onerror=\""));
    }

    #[test]
    fn sanitizer_keeps_reader_markup_and_strips_active_content() {
        let dirty = r#"<p onclick="x()">段落<script>alert(1)</script></p><img src="https://img.example/a.jpg" onerror="x()"><img src="data:image/png;base64,AA"><a href="javascript:alert(1)">坏</a><a href="https://example.com/">好</a><iframe src="https://evil"></iframe><figure><blockquote>引用</blockquote></figure><div style="background:url(x)">样式</div>"#;
        let clean = sanitize_html(dirty);
        assert!(clean.contains("<p>段落</p>"));
        assert!(clean.contains(r#"<img src="https://img.example/a.jpg">"#));
        assert!(!clean.contains("data:"));
        assert!(!clean.contains("javascript:"));
        assert!(!clean.contains("<script"));
        assert!(!clean.contains("onclick"));
        assert!(!clean.contains("onerror"));
        assert!(!clean.contains("<iframe"));
        assert!(!clean.contains("style="));
        assert!(clean.contains(r#"href="https://example.com/""#));
        assert!(clean.contains("noopener"));
        assert!(clean.contains("<blockquote>引用</blockquote>"));
    }

    #[test]
    fn web_url_only_allows_http_schemes() {
        assert_eq!(web_url(" https://a.example/x "), "https://a.example/x");
        assert_eq!(web_url("HTTP://a.example/"), "HTTP://a.example/");
        assert_eq!(web_url("javascript:alert(1)"), "");
        assert_eq!(web_url("data:text/html,x"), "");
        assert_eq!(web_url("https://a.example/\nx"), "");
        assert_eq!(web_url("//evil.example/"), "");
    }

    #[test]
    fn timestamps_are_normalised_to_utc() {
        for (raw, expected) in [
            ("Sat, 17 Oct 2026 08:05:09 GMT", "2026-10-17T08:05:09+00:00"),
            ("17 Oct 2026 08:05 +0800", "2026-10-17T00:05:00+00:00"),
            (
                "Fri, 01 Jan 2027 01:00:00 -0500",
                "2027-01-01T06:00:00+00:00",
            ),
            ("Thu, 31 Dec 2026 20:00:00 EST", "2027-01-01T01:00:00+00:00"),
            ("2026-10-17T08:05:09.123Z", "2026-10-17T08:05:09+00:00"),
            ("2026-10-17T08:05:09+08:00", "2026-10-17T00:05:09+00:00"),
            ("2026-10-17 08:05", "2026-10-17T00:05:00+00:00"),
            ("2026-03-01", "2026-02-28T16:00:00+00:00"),
            ("2026-10-17T00:05:09+00:00", "2026-10-17T00:05:09+00:00"),
        ] {
            assert_eq!(utc_stamp(raw).as_deref(), Some(expected), "{raw}");
        }
        for raw in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-02-30 10:00",
            "32 Oct 2026 10:00 GMT",
        ] {
            assert_eq!(utc_stamp(raw), None, "{raw}");
        }
    }

    #[test]
    fn rss_reads_rfc822_dates_and_dc_creator() {
        let xml = r#"<rss xmlns:dc="http://purl.org/dc/elements/1.1/"><channel><item><title>T</title><dc:creator>作者甲</dc:creator><pubDate>Sat, 17 Oct 2026 08:00:00 GMT</pubDate></item></channel></rss>"#;
        let parsed = parse_feed(xml).unwrap();
        assert_eq!(parsed.entries[0].author, "作者甲");
        assert_eq!(parsed.entries[0].published_at, "2026-10-17T08:00:00+00:00");
    }

    #[test]
    fn large_feeds_split_into_blocks_in_linear_time() {
        let item = "<ITEM><title>t</title><link>https://a.example/x</link></ITEM>";
        let xml = format!("<rss><channel>{}</channel></rss>", item.repeat(20_000));
        let started = std::time::Instant::now();
        let parsed = parse_feed(&xml).unwrap();
        assert_eq!(parsed.entries.len(), 20_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(blocks("<a>1</a><A x=\"y\">2</A><a>", "a"), ["1", "2"]);
    }
}
