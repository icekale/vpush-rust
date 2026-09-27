//! 微博扫码登录。只访问微博和新浪的登录地址，确认后把 Cookie 写入 settings。

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::Db;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

struct Crumb {
    name: String,
    value: String,
    domain: String,
}

struct Session {
    jar: Vec<Crumb>,
    created: u64,
}

struct Reply {
    body: String,
    cookies: Vec<Crumb>,
}

pub enum QrFail {
    Bad(&'static str),
    Detail(String),
    Missing,
}

static SESSIONS: Mutex<Option<HashMap<String, Session>>> = Mutex::new(None);

pub async fn start() -> Result<Value, QrFail> {
    let now = now_secs();
    {
        let mut guard = SESSIONS.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(sessions) = guard.as_mut() {
            sessions.retain(|_, session| now.saturating_sub(session.created) <= 300);
        }
    }
    let (qrid, qrurl, jar) =
        tokio::task::spawn_blocking(move || create_with(|url| live_get(url, &[])))
            .await
            .map_err(|_| QrFail::Bad("获取微博二维码失败，请稍后重试"))?
            .map_err(|_| QrFail::Bad("获取微博二维码失败，请稍后重试"))?;
    {
        let mut guard = SESSIONS.lock().unwrap_or_else(|err| err.into_inner());
        guard
            .get_or_insert_with(HashMap::new)
            .insert(qrid.clone(), Session { jar, created: now });
    }
    Ok(json!({"qrid": qrid, "qrurl": qrurl}))
}

pub async fn status(db: &Db, qrid: &str) -> Result<Value, QrFail> {
    let qrid = qrid.trim();
    if !qrid_ok(qrid) {
        return Err(QrFail::Missing);
    }
    let jar = {
        let guard = SESSIONS.lock().unwrap_or_else(|err| err.into_inner());
        let Some(session) = guard.as_ref().and_then(|sessions| sessions.get(qrid)) else {
            return Err(QrFail::Missing);
        };
        session.jar.iter().map(crumb_clone).collect::<Vec<_>>()
    };
    let qrid_owned = qrid.to_string();
    let polled = tokio::task::spawn_blocking(move || poll_with(jar, &qrid_owned, live_get))
        .await
        .map_err(|_| QrFail::Bad("微博登录状态获取失败，请重试"))?
        .map_err(|_| QrFail::Bad("微博登录状态获取失败，请重试"))?;
    match polled.status.as_str() {
        "pending" | "scanned" => {
            store_jar(qrid, polled.jar);
            Ok(json!({"status": polled.status}))
        }
        "expired" => {
            drop_session(qrid);
            Err(QrFail::Bad("二维码已失效，请重新生成"))
        }
        "ok" => {
            db.set_setting("weibo_cookie", &polled.cookie)
                .await
                .map_err(|_| QrFail::Bad("微博登录状态获取失败，请重试"))?;
            drop_session(qrid);
            Ok(json!({"status": "ok"}))
        }
        _ => Err(QrFail::Detail(format!("微博登录异常: {}", polled.detail))),
    }
}

struct Poll {
    status: String,
    cookie: String,
    detail: String,
    jar: Vec<Crumb>,
}

fn create_with(
    mut fetch: impl FnMut(&str) -> Result<Reply, String>,
) -> Result<(String, String, Vec<Crumb>), String> {
    let url = format!(
        "https://login.sina.com.cn/sso/qrcode/image?entry=weibo&size=180&callback={}",
        now_millis()
    );
    let reply = fetch(&url)?;
    let data = parse_jsonp(&reply.body).ok_or("获取微博二维码失败")?;
    let qrid = data["data"]["qrid"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    let image = data["data"]["image"].as_str().unwrap_or("");
    if !qrid_ok(&qrid) || !image.starts_with("//") {
        return Err("获取微博二维码失败".into());
    }
    Ok((qrid, format!("https:{image}"), reply.cookies))
}

fn poll_with(
    mut jar: Vec<Crumb>,
    qrid: &str,
    mut fetch: impl FnMut(&str, &[Crumb]) -> Result<Reply, String>,
) -> Result<Poll, String> {
    let check = format!(
        "https://login.sina.com.cn/sso/qrcode/check?entry=weibo&qrid={}&callback=STK_{}",
        enc(qrid),
        now_micros()
    );
    let reply = fetch(&check, &jar)?;
    absorb(&mut jar, &reply.cookies);
    let data = parse_jsonp(&reply.body).ok_or("微博登录状态获取失败")?;
    let code = data["retcode"].as_i64().unwrap_or(0);
    let status = match code {
        50114001 => "pending",
        50114002 => "scanned",
        50114004 => "expired",
        20000000 => "confirm",
        _ => "error",
    };
    if status != "confirm" {
        return Ok(Poll {
            status: if status == "error" {
                "error".into()
            } else {
                status.into()
            },
            cookie: String::new(),
            detail: if status == "error" {
                clip(&data.to_string(), 200)
            } else {
                String::new()
            },
            jar,
        });
    }
    let alt = data["data"]["alt"].as_str().unwrap_or("").trim();
    if alt.is_empty() {
        return Ok(Poll {
            status: "error".into(),
            cookie: String::new(),
            detail: "登录确认缺少票据".into(),
            jar,
        });
    }
    let login = format!(
        "https://login.sina.com.cn/sso/login.php?entry=weibo&returntype=TEXT&crossdomain=1&cdult=3&domain=weibo.com&alt={}&savestate=30&callback=STK_{}",
        enc(alt),
        now_millis()
    );
    let login_reply = fetch(&login, &jar)?;
    absorb(&mut jar, &login_reply.cookies);
    let login_data = parse_jsonp(&login_reply.body).ok_or("微博登录状态获取失败")?;
    let mut urls: Vec<String> = login_data["crossDomainUrlList"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(first) = urls.first_mut() {
        first.push_str("&action=login");
    }
    for url in urls {
        if !allowed(&url) {
            return Ok(Poll {
                status: "error".into(),
                cookie: String::new(),
                detail: "登录跳转地址无效".into(),
                jar,
            });
        }
        let reply = fetch(&url, &jar)?;
        absorb(&mut jar, &reply.cookies);
    }
    if !jar.iter().any(|crumb| crumb.name == "SUB") {
        return Ok(Poll {
            status: "error".into(),
            cookie: String::new(),
            detail: "登录后未获取到微博会话".into(),
            jar,
        });
    }
    Ok(Poll {
        status: "ok".into(),
        cookie: cookie_header(&jar),
        detail: String::new(),
        jar,
    })
}

fn live_get(url: &str, jar: &[Crumb]) -> Result<Reply, String> {
    if !allowed(url) {
        return Err("地址不允许".into());
    }
    let host = host_of(url).unwrap_or_default();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .redirects(0)
        .build();
    let cookie = request_cookie(jar, &host);
    let request = agent
        .get(url)
        .set("User-Agent", UA)
        .set("Referer", "https://weibo.com/");
    let request = if cookie.is_empty() {
        request
    } else {
        request.set("Cookie", &cookie)
    };
    let response = match request.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(err) => return Err(err.to_string()),
    };
    let cookies = response
        .all("set-cookie")
        .into_iter()
        .filter_map(|line| parse_set_cookie(line, &host))
        .collect();
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(1_000_000)
        .read_to_end(&mut buf)
        .map_err(|err| err.to_string())?;
    Ok(Reply {
        body: String::from_utf8_lossy(&buf).to_string(),
        cookies,
    })
}

fn parse_jsonp(text: &str) -> Option<Value> {
    let start = text.find('(')?;
    let end = text.rfind(')')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start + 1..end]).ok()
}

fn parse_set_cookie(line: &str, host: &str) -> Option<Crumb> {
    let (pair, rest) = line.split_once(';').unwrap_or((line, ""));
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.starts_with('$') {
        return None;
    }
    let domain = rest
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            key.eq_ignore_ascii_case("domain")
                .then(|| value.trim().to_string())
        })
        .unwrap_or_else(|| host.to_string());
    Some(Crumb {
        name: name.to_string(),
        value: value.trim().to_string(),
        domain,
    })
}

fn absorb(jar: &mut Vec<Crumb>, cookies: &[Crumb]) {
    for crumb in cookies {
        if let Some(found) = jar.iter_mut().find(|item| item.name == crumb.name) {
            if crumb.domain.contains("weibo.com") {
                found.value.clone_from(&crumb.value);
                found.domain.clone_from(&crumb.domain);
            }
        } else {
            jar.push(crumb_clone(crumb));
        }
    }
}

fn cookie_header(jar: &[Crumb]) -> String {
    jar.iter()
        .map(|crumb| format!("{}={}", crumb.name, crumb.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn request_cookie(jar: &[Crumb], host: &str) -> String {
    jar.iter()
        .filter(|crumb| domain_matches(&crumb.domain, host))
        .map(|crumb| format!("{}={}", crumb.name, crumb.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn domain_matches(domain: &str, host: &str) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

fn allowed(url: &str) -> bool {
    if url.contains('@')
        || url.contains(' ')
        || !(url.starts_with("https://") || url.starts_with("http://"))
    {
        return false;
    }
    let Some(host) = host_of(url) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "login.sina.com.cn"
        || host.ends_with(".sina.com.cn")
        || host.ends_with(".sina.cn")
        || host == "weibo.com"
        || host.ends_with(".weibo.com")
        || host == "weibo.cn"
        || host.ends_with(".weibo.cn")
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let hostport = rest.split(['/', '?', '#']).next()?;
    let host = match hostport.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => hostport,
    };
    if host.is_empty() || host.starts_with('[') {
        None
    } else {
        Some(host.to_string())
    }
}

fn qrid_ok(qrid: &str) -> bool {
    let len = qrid.len();
    (1..=128).contains(&len)
        && qrid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn store_jar(qrid: &str, jar: Vec<Crumb>) {
    let mut guard = SESSIONS.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(session) = guard.as_mut().and_then(|sessions| sessions.get_mut(qrid)) {
        session.jar = jar;
    }
}

fn drop_session(qrid: &str) {
    let mut guard = SESSIONS.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(sessions) = guard.as_mut() {
        sessions.remove(qrid);
    }
}

fn crumb_clone(crumb: &Crumb) -> Crumb {
    Crumb {
        name: crumb.name.clone(),
        value: crumb.value.clone(),
        domain: crumb.domain.clone(),
    }
}

fn enc(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(body: &str, cookies: Vec<Crumb>) -> Reply {
        Reply {
            body: body.into(),
            cookies,
        }
    }

    #[test]
    fn parses_qr_and_saves_sub_after_confirm() {
        let image = r#"window.CB && CB({"retcode":20000000,"data":{"qrid":"Q1abcdefgh","image":"//qr.example/q.png"}});"#;
        let (qrid, url, _) = create_with(|_| Ok(reply(image, Vec::new()))).unwrap();
        assert_eq!(qrid, "Q1abcdefgh");
        assert_eq!(url, "https://qr.example/q.png");
        let mut step = 0;
        let result = poll_with(Vec::new(), &qrid, |url, _| {
            step += 1;
            if url.contains("qrcode/check") && step == 1 {
                return Ok(reply(r#"window.CB && CB({"retcode":50114001,"data":null});"#, Vec::new()));
            }
            if url.contains("qrcode/check") {
                return Ok(reply(r#"window.CB && CB({"retcode":20000000,"data":{"alt":"ALT"}});"#, Vec::new()));
            }
            if url.contains("login.php") {
                assert!(url.contains("alt=ALT"));
                return Ok(reply(
                    r#"window.CB && CB({"crossDomainUrlList":["http://passport.weibo.com/sso/crossdomain"]});"#,
                    Vec::new(),
                ));
            }
            assert!(url.starts_with("http://passport.weibo.com/sso/crossdomain&action=login") || url.contains("action=login"));
            Ok(reply("ok", vec![Crumb { name: "SUB".into(), value: "s1".into(), domain: ".weibo.com".into() }]))
        }).unwrap();
        assert_eq!(step, 1);
        assert_eq!(result.status, "pending");
        let result = poll_with(Vec::new(), &qrid, |url, _| {
            if url.contains("qrcode/check") {
                return Ok(reply(r#"window.CB && CB({"retcode":20000000,"data":{"alt":"ALT"}});"#, Vec::new()));
            }
            if url.contains("login.php") {
                return Ok(reply(
                    r#"window.CB && CB({"crossDomainUrlList":["http://passport.weibo.com/sso/crossdomain"]});"#,
                    Vec::new(),
                ));
            }
            assert!(url.contains("action=login"));
            Ok(reply("ok", vec![Crumb { name: "SUB".into(), value: "s1".into(), domain: ".weibo.com".into() }]))
        }).unwrap();
        assert_eq!(result.status, "ok");
        assert_eq!(result.cookie, "SUB=s1");
        assert!(!allowed("https://example.com/sso"));
        assert!(!allowed("https://user:pass@login.sina.com.cn/sso"));
        assert!(allowed("https://login.sina.com.cn/sso/qrcode/image"));
        let mut jar = vec![Crumb {
            name: "SUB".into(),
            value: "old".into(),
            domain: ".sina.com.cn".into(),
        }];
        absorb(
            &mut jar,
            &[Crumb {
                name: "SUB".into(),
                value: "new".into(),
                domain: ".weibo.com".into(),
            }],
        );
        assert_eq!(cookie_header(&jar), "SUB=new");
    }
}
