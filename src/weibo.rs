//! 微博时间线。使用 settings.weibo_cookie；失效时用 weibo_app_cred 换新 cookie。
//! 不走账号密码登录。每轮只拉第一页，超过 36 小时且没有水位的旧帖不入库。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::Db;

const TIMELINE: &str = "https://weibo.com/ajax/statuses/mymblog";
const GETCOOKIE: &str = "https://api.weibo.cn/2/account/getcookie";
const WEB_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
const APP_UA: &str = "Weibo/8350 (Android 14; zh_CN; 23127PN0CC; 2.0.4; sdk=34)";
const APP_COMMON: &[(&str, &str)] = &[
    ("c", "android"),
    ("wb_version", "8173"),
    ("from", "10G9295010"),
    ("wm", "4260_0001"),
    ("oldwm", "4260_0001"),
    ("v_f", "2"),
    ("v_p", "93"),
    ("ft", "0"),
    ("skin", "default"),
    ("dlang", "zh-Hans-CN"),
    ("networktype", "wifi"),
    ("lang", "zh_CN"),
];

struct WeiboPost {
    external_id: String,
    title: String,
    content: String,
    url: String,
    images: Vec<String>,
    published_at: String,
    published_unix: Option<i64>,
    avatar: String,
}

enum Keep {
    Drop,
    Store,
    Notify,
}

enum FetchErr {
    Dead,
    Other(String),
}

impl std::fmt::Display for FetchErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchErr::Dead => write!(f, "微博登录已失效"),
            FetchErr::Other(msg) => write!(f, "{msg}"),
        }
    }
}

pub fn spawn(db: Db) {
    if !crate::xueqiu::fetch_enabled() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = poll(&db).await {
                tracing::warn!("微博抓取: {err}");
                let _ = db.note_platform("weibo", Some(&err)).await;
            } else {
                let _ = db.note_platform("weibo", None).await;
            }
            tokio::time::sleep(Duration::from_secs(crate::xueqiu::poll_wait(&db).await)).await;
        }
    });
}

async fn poll(db: &Db) -> Result<(), String> {
    let kols = db.weibo_kols().await.map_err(|e| e.to_string())?;
    if kols.is_empty() {
        return Ok(());
    }
    let Some(mut cookie) = load_cookie(db).await? else {
        tracing::info!("微博抓取跳过：未配置 weibo_cookie 或 weibo_app_cred");
        return Ok(());
    };
    let exit = crate::proxy_admin::acquire(db, "weibo").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    for (id, name, external_id, original_only) in kols {
        match pull(db, &cookie, id, &name, &external_id, original_only, proxy.clone()).await {
            Ok(()) => {
                crate::proxy_admin::note(db, proxy_id, true, "").await;
                let _ = db.note_kol_fetch(id, None).await;
            }
            Err(FetchErr::Dead) => match refresh_cookie(db).await? {
                Some(fresh) => {
                    cookie = fresh;
                    if let Err(err) = pull(db, &cookie, id, &name, &external_id, original_only, proxy.clone()).await {
                        let detail = err.to_string();
                        let _ = db.note_kol_fetch(id, Some(&detail)).await;
                        tracing::warn!(kol = id, "{err}");
                    } else {
                        let _ = db.note_kol_fetch(id, None).await;
                    }
                }
                None => {
                    let _ = db.note_kol_fetch(id, Some("微博登录已失效")).await;
                    tracing::warn!(kol = id, "微博登录已失效。密码登录未接入，请更新 weibo_cookie 或 weibo_app_cred");
                }
            },
            Err(FetchErr::Other(msg)) => {
                crate::proxy_admin::note(db, proxy_id, false, &msg).await;
                let _ = db.note_kol_fetch(id, Some(&msg)).await;
                tracing::warn!(kol = id, "{msg}");
            }
        }
    }
    Ok(())
}

async fn load_cookie(db: &Db) -> Result<Option<String>, String> {
    if let Some(saved) = db.setting("weibo_cookie").await.map_err(|e| e.to_string())? {
        if saved.contains("SUB=") {
            return Ok(Some(saved));
        }
    }
    if let Ok(env) = std::env::var("WEIBO_COOKIE") {
        if env.contains("SUB=") {
            return Ok(Some(env));
        }
    }
    refresh_cookie(db).await
}

async fn pull(
    db: &Db,
    cookie: &str,
    kol_id: i64,
    name: &str,
    uid: &str,
    original_only: bool,
    proxy: Option<String>,
) -> Result<(), FetchErr> {
    let data = timeline(cookie, uid, original_only, proxy).await?;
    if data.get("ok") != Some(&json!(1)) {
        let msg = field_str(&data, "msg");
        return Err(FetchErr::Other(format!("微博接口异常: {msg}")));
    }
    let rows = data["data"]["list"].as_array().cloned().unwrap_or_default();
    let posts = rows.iter().filter_map(parse_mblog).collect::<Vec<_>>();
    if let Some(avatar) = posts.iter().find_map(|post| (!post.avatar.is_empty()).then(|| post.avatar.clone())) {
        db.set_avatar(kol_id, &avatar).await.map_err(|e| FetchErr::Other(e.to_string()))?;
    }
    let watermark = db.max_published_at(kol_id).await.map_err(|e| FetchErr::Other(e.to_string()))?;
    let now = now_unix();
    for post in posts {
        match keep(&post.published_at, post.published_unix, &watermark, now) {
            Keep::Drop => continue,
            Keep::Store | Keep::Notify => {}
        }
        let is_new = !db
            .has_post("weibo", &post.external_id)
            .await
            .map_err(|e| FetchErr::Other(e.to_string()))?;
        if !is_new {
            continue;
        }
        let images = serde_json::to_string(&post.images).unwrap_or_else(|_| "[]".into());
        db.save_fetched(
            kol_id,
            &post.external_id,
            &post.title,
            &post.content,
            "",
            &images,
            &post.url,
            &post.published_at,
        )
        .await
        .map_err(|e| FetchErr::Other(e.to_string()))?;
        if !matches!(keep(&post.published_at, post.published_unix, &watermark, now), Keep::Notify) {
            continue;
        }
        let push = db.should_push(kol_id, "post").await.map_err(|e| FetchErr::Other(e.to_string()))?;
        if !push {
            continue;
        }
        crate::push::deliver(
            db,
            kol_id,
            &crate::feishu::Note {
                kol_name: name,
                platform: "weibo",
                post_type: "post",
                title: &post.title,
                content: &post.content,
                url: &post.url,
                published_at: &post.published_at,
            },
        )
        .await;
    }
    Ok(())
}

pub(crate) enum Keepalive {
    Alive,
    Dead(String),
    Transient,
    Renewed(String),
}

pub(crate) async fn probe_keepalive(db: &Db, cookie: &str, uid: &str) -> Keepalive {
    match timeline(cookie, uid, false, None).await {
        Ok(_) => Keepalive::Alive,
        Err(FetchErr::Other(msg)) if msg.contains("432") => Keepalive::Transient,
        Err(FetchErr::Dead) => match refresh_cookie(db).await {
            Ok(Some(cookie)) => Keepalive::Renewed(cookie),
            _ => Keepalive::Dead("保活：会话已失效".into()),
        },
        Err(_) => Keepalive::Transient,
    }
}

async fn timeline(cookie: &str, uid: &str, original_only: bool, proxy: Option<String>) -> Result<Value, FetchErr> {
    let cookie = cookie.to_string();
    let uid = uid.to_string();
    let feature = if original_only { "1" } else { "0" }.to_string();
    match tokio::task::spawn_blocking(move || fetch_timeline(&cookie, &uid, &feature, proxy.as_deref())).await {
        Ok(result) => result,
        Err(err) => Err(FetchErr::Other(err.to_string())),
    }
}

fn fetch_timeline(cookie: &str, uid: &str, feature: &str, proxy: Option<&str>) -> Result<Value, FetchErr> {
    let agent = crate::proxy_admin::http_agent(proxy, Duration::from_secs(15), Duration::from_secs(20)).map_err(FetchErr::Other)?;
    let mut req = agent
        .get(TIMELINE)
        .query("uid", uid)
        .query("feature", feature)
        .query("page", "1")
        .set("User-Agent", WEB_UA)
        .set("Accept", "application/json, text/plain, */*")
        .set("Referer", "https://weibo.com/")
        .set("Cookie", cookie);
    let xsrf = cookie_value(cookie, "XSRF-TOKEN");
    if !xsrf.is_empty() {
        req = req.set("X-XSRF-TOKEN", &xsrf);
    }
    let (status, text) = read_response(req.call()).map_err(FetchErr::Other)?;
    if status == 432 {
        return Err(FetchErr::Other("微博反爬拦截（HTTP 432）".into()));
    }
    if status == 401 || status == 403 || text.trim_start().starts_with('<') {
        return Err(FetchErr::Dead);
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| FetchErr::Dead)?;
    let msg = field_str(&value, "msg").to_lowercase();
    if value.get("ok") == Some(&json!(0)) && (msg.contains("login") || msg.contains("登录")) {
        return Err(FetchErr::Dead);
    }
    if status != 200 {
        return Err(FetchErr::Other(format!("微博接口 HTTP {status}")));
    }
    Ok(value)
}

async fn refresh_cookie(db: &Db) -> Result<Option<String>, String> {
    let Some(raw) = db.setting("weibo_app_cred").await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let cred: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let gsid = field_str(&cred, "gsid");
    let aid = field_str(&cred, "aid");
    let secret = field_str(&cred, "s");
    let device = cred.get("device").cloned().unwrap_or(Value::Null);
    if gsid.is_empty() || aid.is_empty() || secret.is_empty() {
        tracing::warn!("微博 App 凭证缺少 gsid、aid 或 s");
        return Ok(None);
    }
    if field_str(&device, "ua").is_empty() || field_str(&device, "android_id").is_empty() {
        tracing::warn!("微博 App 凭证缺少设备参数 ua 或 android_id");
        return Ok(None);
    }
    let old = db.setting("weibo_cookie").await.map_err(|e| e.to_string())?.unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || request_cookie(&gsid, &aid, &secret, &device))
        .await
        .map_err(|e| e.to_string())?;
    let fresh = match result {
        Ok(cookie) => cookie,
        Err(err) => {
            tracing::warn!("微博 App 凭证续期失败: {err}");
            return Ok(None);
        }
    };
    let Some(merged) = merge_cookie(&old, &fresh) else {
        tracing::warn!("微博 getcookie 返回的 cookie 缺少 SUB");
        return Ok(None);
    };
    db.set_setting("weibo_cookie", &merged).await.map_err(|e| e.to_string())?;
    Ok(Some(merged))
}

fn request_cookie(gsid: &str, aid: &str, secret: &str, device: &Value) -> Result<String, String> {
    let mut pairs: Vec<(String, String)> = APP_COMMON
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    if let Some(obj) = device.as_object() {
        for (key, value) in obj {
            if let Some(text) = value.as_str() {
                pairs.push((key.clone(), text.to_string()));
            }
        }
    }
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    pairs.push(("s".into(), secret.into()));
    pairs.push(("gsid".into(), gsid.into()));
    pairs.push(("aid".into(), aid.into()));
    pairs.push(("ul_ctime".into(), now_ms.to_string()));
    pairs.push(("getcookie".into(), "1".into()));
    let body = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(20))
        .build();
    let req = agent
        .post(GETCOOKIE)
        .set("User-Agent", APP_UA)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Cookie", &format!("gsid_CTandWM={gsid}"));
    let (status, text) = read_response(req.send_string(&body))?;
    if status != 200 {
        return Err(format!("getcookie HTTP {status}"));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let cookie = value["cookie"][".weibo.com"].as_str().unwrap_or("").to_string();
    if cookie.is_empty() {
        return Err("getcookie 未返回 .weibo.com cookie".into());
    }
    Ok(cookie)
}

fn parse_mblog(mblog: &Value) -> Option<WeiboPost> {
    let external_id = field_str(mblog, "id");
    if external_id.is_empty() {
        return None;
    }
    let content = strip_html(&field_str(mblog, "text"));
    let title_src = field_str(mblog, "text_raw");
    let title_base = if title_src.is_empty() { content.as_str() } else { title_src.as_str() };
    let (published_at, published_unix) = published(&field_str(mblog, "created_at"));
    let user = &mblog["user"];
    let avatar = field_str(user, "avatar_large");
    let avatar = if avatar.is_empty() { field_str(user, "profile_image_url") } else { avatar };
    Some(WeiboPost {
        external_id: external_id.clone(),
        title: title_base.chars().take(80).collect(),
        content,
        url: format!("https://weibo.com/detail/{external_id}"),
        images: images(mblog),
        published_at,
        published_unix,
        avatar: absolute_url(&avatar),
    })
}

fn keep(published: &str, unix: Option<i64>, watermark: &str, now: i64) -> Keep {
    if !watermark.is_empty() && !published.is_empty() && published < watermark {
        return Keep::Drop;
    }
    if watermark.is_empty() {
        if let Some(ts) = unix {
            if now.saturating_sub(ts) > 36 * 3600 {
                return Keep::Drop;
            }
        }
    }
    if let Some(ts) = unix {
        if now.saturating_sub(ts) > 60 * 60 {
            return Keep::Store;
        }
    }
    Keep::Notify
}

fn images(mblog: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(pics) = mblog.get("pics").and_then(Value::as_array) {
        for pic in pics {
            let url = pic["large"]["url"]
                .as_str()
                .or_else(|| pic["original"]["url"].as_str())
                .or_else(|| pic["url"].as_str())
                .unwrap_or("");
            push_image(&mut out, url);
            if out.len() >= 4 {
                return out;
            }
        }
    }
    let infos = &mblog["pic_infos"];
    if let Some(ids) = mblog.get("pic_ids").and_then(Value::as_array) {
        for id in ids {
            let info = &infos[id.as_str().unwrap_or("")];
            let url = info["original"]["url"]
                .as_str()
                .or_else(|| info["large"]["url"].as_str())
                .or_else(|| info["mw690"]["url"].as_str())
                .unwrap_or("");
            push_image(&mut out, url);
            if out.len() >= 4 {
                break;
            }
        }
    }
    out
}

fn push_image(out: &mut Vec<String>, url: &str) {
    let url = absolute_url(url);
    if !url.is_empty() && !out.iter().any(|item| item == &url) && out.len() < 4 {
        out.push(url);
    }
}

fn absolute_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

fn published(raw: &str) -> (String, Option<i64>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    if let Some(unix) = weibo_unix(raw).or_else(|| clock_unix(raw)) {
        return (crate::xueqiu::published_of(&json!(unix)), Some(unix));
    }
    (raw.to_string(), None)
}

fn weibo_unix(raw: &str) -> Option<i64> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() != 6 {
        return None;
    }
    let month = month_num(parts[1])?;
    let day: u32 = parts[2].parse().ok()?;
    let year: i32 = parts[5].parse().ok()?;
    let time: Vec<&str> = parts[3].split(':').collect();
    if time.len() < 2 {
        return None;
    }
    let hour: u32 = time[0].parse().ok()?;
    let minute: u32 = time[1].parse().ok()?;
    let second: u32 = time.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let offset = zone_minutes(parts[4])?;
    let local = days_from_civil(year, month, day) * 86400
        + hour as i64 * 3600
        + minute as i64 * 60
        + second as i64;
    Some(local - offset * 60)
}

fn clock_unix(raw: &str) -> Option<i64> {
    let (date, time) = raw.split_once(' ')?;
    let mut date = date.split('-');
    let year: i32 = date.next()?.parse().ok()?;
    let month: u32 = date.next()?.parse().ok()?;
    let day: u32 = date.next()?.parse().ok()?;
    let mut time = time.split(':');
    let hour: u32 = time.next()?.parse().ok()?;
    let minute: u32 = time.next()?.parse().ok()?;
    let local = days_from_civil(year, month, day) * 86400 + hour as i64 * 3600 + minute as i64 * 60;
    Some(local - 8 * 3600)
}

fn month_num(name: &str) -> Option<u32> {
    Some(match name {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn zone_minutes(raw: &str) -> Option<i64> {
    let sign = match raw.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return None,
    };
    if raw.len() != 5 {
        return None;
    }
    let hour: i64 = raw[1..3].parse().ok()?;
    let minute: i64 = raw[3..5].parse().ok()?;
    Some(sign * (hour * 60 + minute))
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = (year - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    era as i64 * 146097 + doe as i64 - 719468
}

fn strip_html(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let end = rest.find('>').unwrap_or(rest.len() - 1);
        let tag = &rest[..=end];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("<br") {
            out.push('\n');
        } else if lower.starts_with("<img") {
            if let Some(alt) = attr(tag, "alt") {
                out.push_str(&alt);
            }
        } else if is_block_close(&lower) {
            out.push_str("\n\n");
        }
        rest = if end + 1 <= rest.len() { &rest[end + 1..] } else { "" };
    }
    tidy(&unescape(&out))
}

fn is_block_close(tag: &str) -> bool {
    tag.starts_with("</p")
        || tag.starts_with("</div")
        || tag.starts_with("</li")
        || tag.starts_with("</tr")
        || tag.starts_with("</blockquote")
        || (tag.starts_with("</h") && tag.as_bytes().get(3).is_some_and(|b| b.is_ascii_digit()))
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{name}=");
    let start = lower.find(&key)? + key.len();
    let bytes = tag.as_bytes();
    let quote = *bytes.get(start)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value = &tag[start + 1..];
    let end = value.find(quote as char)?;
    Some(value[..end].to_string())
}

fn unescape(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(';').unwrap_or(0);
        if end == 0 {
            out.push('&');
            rest = &rest[1..];
            continue;
        }
        let token = &rest[1..end];
        let ch = if token == "amp" {
            Some('&')
        } else if token == "lt" {
            Some('<')
        } else if token == "gt" {
            Some('>')
        } else if token == "quot" {
            Some('"')
        } else if token == "apos" || token == "#39" {
            Some('\'')
        } else if token == "nbsp" || token == "#160" {
            Some('\u{00a0}')
        } else if let Some(hex) = token.strip_prefix("#x").or_else(|| token.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = token.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            None
        };
        if let Some(ch) = ch {
            out.push(ch);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn tidy(input: &str) -> String {
    let mut out = String::new();
    let mut newline = false;
    for ch in input.replace('\u{00a0}', " ").chars() {
        if ch == '\n' {
            while out.ends_with(' ') || out.ends_with('\t') {
                out.pop();
            }
            out.push('\n');
            newline = true;
            continue;
        }
        if newline && (ch == ' ' || ch == '\t') {
            continue;
        }
        newline = false;
        out.push(ch);
    }
    let mut collapsed = String::new();
    let mut breaks = 0;
    for ch in out.chars() {
        if ch == '\n' {
            breaks += 1;
            if breaks <= 2 {
                collapsed.push('\n');
            }
        } else {
            breaks = 0;
            collapsed.push(ch);
        }
    }
    collapsed.trim().to_string()
}

fn merge_cookie(old: &str, fresh: &str) -> Option<String> {
    let mut pairs = cookie_pairs(old);
    let updates = cookie_pairs(fresh);
    let mut found_sub = false;
    for (key, value) in updates {
        if !matches!(key.as_str(), "SUB" | "SUBP" | "SCF") {
            continue;
        }
        if key == "SUB" {
            found_sub = true;
        }
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = value;
        } else {
            pairs.push((key, value));
        }
    }
    if !found_sub {
        return None;
    }
    Some(pairs.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
}

fn cookie_pairs(cookie: &str) -> Vec<(String, String)> {
    cookie
        .split(';')
        .filter_map(|seg| {
            let seg = seg.trim();
            let (k, v) = seg.split_once('=')?;
            if k.is_empty() { None } else { Some((k.to_string(), v.to_string())) }
        })
        .collect()
}

fn cookie_value(cookie: &str, name: &str) -> String {
    cookie_pairs(cookie)
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn field_str(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn read_response(result: Result<ureq::Response, ureq::Error>) -> Result<(u16, String), String> {
    match result {
        Ok(resp) => {
            let status = resp.status();
            resp.into_string().map(|text| (status, text)).map_err(|e| e.to_string())
        }
        Err(ureq::Error::Status(status, resp)) => {
            resp.into_string().map(|text| (status, text)).map_err(|e| e.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_images_time_and_cookie_merge() {
        let text = strip_html("你好<br>甲<img alt=\"[笑]\" src=\"x\"> &amp; 乙</p><p>丙");
        assert_eq!(text, "你好\n甲[笑] & 乙\n\n丙");
        let mblog = json!({
            "id": "100",
            "text": "正文<br>第二行",
            "text_raw": "",
            "created_at": "Wed Oct 10 20:19:24 +0800 2012",
            "user": {"avatar_large": "//wx.qlogo.cn/a.jpg"},
            "pic_ids": ["a", "b", "c", "d", "e"],
            "pic_infos": {
                "a": {"original": {"url": "//wx.example/a.jpg"}},
                "b": {"large": {"url": "https://wx.example/b.jpg"}},
                "c": {"mw690": {"url": "https://wx.example/c.jpg"}},
                "d": {"original": {"url": "https://wx.example/d.jpg"}},
                "e": {"original": {"url": "https://wx.example/e.jpg"}}
            }
        });
        let post = parse_mblog(&mblog).unwrap();
        assert_eq!(post.published_at, "2012-10-10 20:19");
        assert_eq!(post.content, "正文\n第二行");
        assert_eq!(post.avatar, "https://wx.qlogo.cn/a.jpg");
        assert_eq!(post.images.len(), 4);
        assert_eq!(post.url, "https://weibo.com/detail/100");
        assert_eq!(published("Thu Jan 01 00:30:00 +0000 1970").0, "1970-01-01 08:30");
        let old_hour = 1_700_000_000;
        assert!(matches!(keep("2023-11-14 22:13", Some(old_hour), "", old_hour + 40 * 3600), Keep::Drop));
        assert!(matches!(keep("2023-11-14 22:13", Some(old_hour), "", old_hour + 120), Keep::Notify));
        assert!(matches!(keep("2023-11-14 22:13", Some(old_hour), "2024-01-01 00:00", old_hour), Keep::Drop));
        let merged = merge_cookie("SCF=old; SUB=old; EXTRA=keep", "SUB=new; SUBP=p; ALF=no").unwrap();
        assert!(merged.contains("SUB=new"));
        assert!(merged.contains("EXTRA=keep"));
        assert!(merged.contains("SUBP=p"));
        assert!(!merged.contains("ALF="));
        assert!(merge_cookie("SUB=old", "SCF=only").is_none());
    }

    #[tokio::test]
    async fn saved_weibo_post_shows_in_feed() {
        let path = std::env::temp_dir().join(format!(
            "vpush-weibo-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        let kol = db.add_kol("weibo", "微博甲", "10001", None, false, false, true).await.unwrap();
        db.subscribe(admin.id, true, kol, "post").await.unwrap();
        let targets = db.weibo_kols().await.unwrap();
        assert_eq!(targets[0].3, true);
        let post = parse_mblog(&json!({
            "id": 7,
            "text": "一条微博",
            "created_at": "2024-03-02 09:30:00"
        })).unwrap();
        db.save_fetched(kol, &post.external_id, &post.title, &post.content, "", "[]", &post.url, &post.published_at)
            .await
            .unwrap();
        let page = db.kol_posts(admin.id, true, kol, 10).await.unwrap().unwrap();
        assert_eq!(page[0]["platform"], "weibo");
        assert_eq!(page[0]["content"], "一条微博");
        let _ = std::fs::remove_file(&path);
    }
}
