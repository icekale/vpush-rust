//! 财经资讯的 RSS/Atom 抓取。只接受 https，不抓本机和内网地址。

use std::time::Duration;

use crate::db::{Db, NewsEntry};

pub fn spawn(db: Db) {
    tokio::spawn(async move {
        loop {
            if db.news_flag("news_enabled", false).await.unwrap_or(false) {
                if let Err(err) = refresh_all(&db).await {
                    tracing::warn!("资讯抓取: {err}");
                    let _ = db.note_platform("news", Some(&err)).await;
                } else {
                    let _ = db.note_platform("news", None).await;
                }
            }
            let minutes = db.setting("news_refresh_minutes").await.ok().flatten().and_then(|v| v.parse::<u64>().ok()).unwrap_or(30);
            tokio::time::sleep(Duration::from_secs(minutes.clamp(5, 1440) * 60)).await;
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
    if !db.news_flag("news_enabled", false).await.map_err(|e| e.to_string())? {
        return Err("财经资讯采集已关闭".into());
    }
    let mut accepted = Vec::new();
    for id in ids {
        let Some((source_id, url, enabled, archived)) = db.news_feed_target(*id).await.map_err(|e| e.to_string())? else {
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
        Ok(parsed) => match db.save_news_entries(source_id, feed_id, &parsed.entries).await {
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
    let body = tokio::task::spawn_blocking(move || http_text(&url)).await.map_err(|e| e.to_string())??;
    parse_feed(&body)
}

pub fn public_https(url: &str) -> Result<(), String> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("只接受 https Feed".into());
    };
    if rest.contains('@') {
        return Err("Feed 地址不能带账号".into());
    }
    let host = rest.split(['/', '?', ':']).next().unwrap_or("");
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".localhost") {
        return Err("Feed 地址不能指向本机".into());
    }
    if is_private_host(host) {
        return Err("Feed 地址不能指向内网".into());
    }
    Ok(())
}

fn is_private_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    if host.starts_with("::1") || host.starts_with("fc") || host.starts_with("fd") || host.starts_with("fe80") {
        return true;
    }
    let Some(ip) = host.parse::<std::net::Ipv4Addr>().ok() else {
        return false;
    };
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
}

fn http_text(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(15))
        .redirects(3)
        .build();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    resp.into_string().map_err(|e| e.to_string())
}

pub fn parse_feed(xml: &str) -> Result<Parsed, String> {
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<rss") || lower.contains("<rdf") || lower.contains("<item") {
        let entries = blocks(xml, "item").into_iter().filter_map(rss_entry).collect::<Vec<_>>();
        return Ok(Parsed { format: "RSS".into(), title: text_of(xml, "title"), entries });
    }
    if lower.contains("<feed") || lower.contains("<entry") {
        let entries = blocks(xml, "entry").into_iter().filter_map(atom_entry).collect::<Vec<_>>();
        return Ok(Parsed { format: "Atom".into(), title: text_of(xml, "title"), entries });
    }
    Err("不是 RSS 或 Atom".into())
}

fn rss_entry(block: &str) -> Option<NewsEntry> {
    let title = plain(&text_of(block, "title"));
    if title.is_empty() {
        return None;
    }
    let url = text_of(block, "link");
    let guid = text_of(block, "guid");
    let body = text_of(block, "description");
    Some(NewsEntry {
        external_id: first_nonempty(&[&guid, &url, &title]),
        title,
        summary: plain(&body),
        content: plain(&body),
        url,
        author: plain(&text_of(block, "creator")),
        published_at: stamp(&text_of(block, "pubDate")),
    })
}

fn atom_entry(block: &str) -> Option<NewsEntry> {
    let title = plain(&text_of(block, "title"));
    if title.is_empty() {
        return None;
    }
    let url = attr_of(block, "link", "href").unwrap_or_else(|| text_of(block, "link"));
    let body = {
        let summary = text_of(block, "summary");
        if summary.is_empty() { text_of(block, "content") } else { summary }
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
            if updated.is_empty() { text_of(block, "published") } else { updated }
        }),
    })
}

fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    while let Some(start) = find_tag(rest, &open) {
        let after = &rest[start + open.len()..];
        let Some(end) = find_tag(after, &close) else { break };
        let content_start = after.find('>').map(|n| n + 1).unwrap_or(0);
        if content_start <= end {
            out.push(&after[content_start..end]);
        }
        rest = &after[end + close.len()..];
    }
    out
}

fn find_tag(text: &str, tag: &str) -> Option<usize> {
    text.to_ascii_lowercase().find(&tag.to_ascii_lowercase())
}

fn text_of(xml: &str, tag: &str) -> String {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{tag}").to_ascii_lowercase();
    let Some(start) = lower.find(&open) else { return String::new() };
    let after = &xml[start + open.len()..];
    let Some(end_bracket) = after.find('>') else { return String::new() };
    if after[..end_bracket].ends_with('/') {
        return String::new();
    }
    let body = &after[end_bracket + 1..];
    let close = format!("</{tag}>").to_ascii_lowercase();
    let Some(end) = body.to_ascii_lowercase().find(&close) else { return String::new() };
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
        let end = rest[start..].find('>').map(|n| start + n).unwrap_or(rest.len() - 1);
        let tag = rest[start..=end].to_ascii_lowercase();
        if tag.starts_with("<br") || tag.starts_with("</p") {
            out.push('\n');
        }
        rest = if end + 1 < rest.len() { &rest[end + 1..] } else { "" };
    }
    out.push_str(rest);
    decode(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode(input: &str) -> String {
    input
        .replace("<![CDATA[", "")
        .replace("]]>", "")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

fn first_nonempty(values: &[&str]) -> String {
    values.iter().find(|value| !value.trim().is_empty()).copied().unwrap_or("").trim().to_string()
}

fn stamp(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Some(iso) = raw.get(0..19) {
        if iso.as_bytes().get(4) == Some(&b'-') && iso.as_bytes().get(10).is_some_and(|b| *b == b'T' || *b == b' ') {
            return format!("{} {}", &iso[..10], &iso[11..16]);
        }
    }
    raw.to_string()
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
    if url.len() > 2048 || !url.to_ascii_lowercase().starts_with("https://") || url[8..].contains('@') {
        return Err(ImageFail::Unsafe);
    }
    let rest = &url[8..];
    let authority = rest.split_once(['/', '?', '#']).map(|(host, _)| host).unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (host, Some(port)),
        _ => (authority, None),
    };
    if port.is_some_and(|port| port != "443") || host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return Err(ImageFail::Unsafe);
    }
    let host = host.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    if public_https(&format!("https://{host}/")).is_err() {
        return Err(ImageFail::Unsafe);
    }
    Ok(ImageTarget { url: url.to_string(), host })
}

pub fn fetch_image(url: &str) -> Result<ArticleImage, ImageFail> {
    use std::io::Read;
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build()
        .get(url)
        .call()
        .map_err(|_| ImageFail::Upstream)?;
    if !(200..300).contains(&response.status()) {
        return Err(ImageFail::Upstream);
    }
    let media_type = response.header("content-type").unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if !IMAGE_TYPES.contains(&media_type.as_str()) {
        return Err(ImageFail::Unsafe);
    }
    if response.header("content-length").and_then(|v| v.parse::<usize>().ok()).is_some_and(|n| n > IMAGE_MAX) {
        return Err(ImageFail::Unsafe);
    }
    let mut body = Vec::new();
    response.into_reader().take(IMAGE_MAX as u64 + 1).read_to_end(&mut body).map_err(|_| ImageFail::Upstream)?;
    if body.len() > IMAGE_MAX {
        return Err(ImageFail::Unsafe);
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&body);
    let etag = format!("\"news-img-{}\"", digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect::<String>());
    Ok(ArticleImage { body, media_type, etag })
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
        assert_eq!(parsed.entries[0].published_at, "2026-09-26 12:30");
        assert_eq!(parsed.entries[0].external_id, "a1");
        assert!(public_https("http://example.com/a").is_err());
        assert!(public_https("https://127.0.0.1/a").is_err());
        assert!(public_https("https://10.1.2.3/feed").is_err());
        assert!(public_https("https://example.com/feed").is_ok());
        assert!(image_target("http://img.example/a.jpg").is_err());
        assert!(image_target("https://user:pass@img.example/a.jpg").is_err());
        assert!(image_target("https://10.1.2.3/a.jpg").is_err());
        assert!(image_target("https://img.example:8443/a.jpg").is_err());
        assert_eq!(image_target("https://img.example/a.jpg").unwrap().host, "img.example");
    }
}
