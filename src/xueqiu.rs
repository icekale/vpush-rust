//! 雪球时间线：App 隐式账号注册，再把帖子写入 posts。
//! 每轮只拉第一页。更早的页留给后面的补抓。

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::{json, Value};
use sha1::Sha1;

use crate::db::Db;
use crate::xq_crypto::{
    decode_public_key, derive_sm4_key, ecdh_shared_hex, encode_public_key, generate_keypair,
    sm4_decrypt_hex, sm4_encrypt_hex,
};

const API: &str = "https://api.xueqiu.com";
const TIMELINE: &str = "https://api.xueqiu.com/v4/statuses/user_timeline.json";
pub(crate) const APP_UA: &str = "Xueqiu Android 14.96.3";
const CLIENT_ID: &str = "JtXbaMn7eP";
const CLIENT_SECRET: &str = "txsDfr9FphRSPov5oQou74";
const SIGN_SECRET: &str = "2ee0b0d606aa1e845fb9537251db0785";
const APP_VERSION: &str = "14.96.3";
const IDENTITY_KEY: &str = "xueqiu_identity";

#[derive(Clone, Serialize)]
struct Identity {
    access_token: String,
    id_token: String,
    uid: String,
    device_id: String,
    cookie: String,
}

#[derive(Debug)]
enum PullErr {
    Dead,
    Other(String),
}

struct Batch {
    posts: Vec<Fetched>,
    avatar: String,
}

struct Fetched {
    external_id: String,
    title: String,
    content: String,
    post_type: String,
    images: Vec<String>,
    url: String,
    published_at: String,
}

pub fn spawn(db: Db) {
    if !enabled() {
        tracing::info!("雪球抓取已关闭");
        return;
    }
    tokio::spawn(async move {
        let mut streak = HashMap::new();
        loop {
            if let Err(err) = poll(&db, &mut streak).await {
                tracing::warn!("雪球抓取: {err}");
                let _ = db.note_platform("xueqiu", Some(&err)).await;
            } else {
                let _ = db.note_platform("xueqiu", None).await;
            }
            tokio::time::sleep(Duration::from_secs(poll_wait(&db).await)).await;
        }
    });
}

pub(crate) fn fetch_enabled() -> bool {
    enabled()
}

pub(crate) async fn poll_wait(db: &crate::db::Db) -> u64 {
    if let Ok(Some(raw)) = db.setting("config_interval_seconds").await {
        if let Ok(n) = raw.trim().parse::<u64>() {
            if (15..=3600).contains(&n) {
                return n;
            }
        }
    }
    poll_secs()
}

pub async fn app_cookie(db: &Db) -> Result<String, String> {
    Ok(ensure_identity(db).await?.cookie)
}

pub async fn rotate_app_cookie(db: &Db) -> Result<String, String> {
    Ok(rotate_identity(db).await?.cookie)
}

fn enabled() -> bool {
    !matches!(
        std::env::var("XUEQIU_APP_IDENTITY")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("0" | "false" | "no" | "off")
    ) && !matches!(
        std::env::var("VPUSH_FETCH").ok().as_deref().map(str::trim),
        Some("0" | "false" | "no" | "off")
    )
}

fn poll_secs() -> u64 {
    std::env::var("VPUSH_POLL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n: &u64| *n >= 15)
        .unwrap_or(180)
}

async fn poll(db: &Db, streak: &mut HashMap<i64, u32>) -> Result<(), String> {
    let kols = db
        .kols_to_fetch("xueqiu")
        .await
        .map_err(|e| e.to_string())?;
    if kols.is_empty() {
        return Ok(());
    }
    let exit = crate::proxy_admin::acquire(db, "xueqiu").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    let mut ident = ensure_identity(db).await?;
    for (id, name, external_id) in kols {
        let pulled = pull(db, &ident, id, &external_id, streak, proxy.clone()).await;
        match pulled {
            Ok(batch) => {
                crate::proxy_admin::note(db, proxy_id, true, "").await;
                let _ = db.note_kol_fetch(id, None).await;
                save(db, id, &name, batch).await?;
            }
            Err(PullErr::Dead) => {
                tracing::warn!("雪球身份失效，重新注册");
                ident = rotate_identity(db).await?;
                if let Ok(batch) = pull(db, &ident, id, &external_id, streak, proxy.clone()).await {
                    let _ = db.note_kol_fetch(id, None).await;
                    save(db, id, &name, batch).await?;
                } else {
                    let _ = db.note_kol_fetch(id, Some("cookie 失效")).await;
                }
            }
            Err(PullErr::Other(msg)) => {
                crate::proxy_admin::note(db, proxy_id, false, &msg).await;
                let _ = db.note_kol_fetch(id, Some(&msg)).await;
                tracing::warn!(kol = id, name, "{msg}");
            }
        }
    }
    Ok(())
}

async fn save(db: &Db, kol_id: i64, kol_name: &str, batch: Batch) -> Result<(), String> {
    if !batch.avatar.is_empty() {
        db.set_avatar(kol_id, &batch.avatar)
            .await
            .map_err(|e| e.to_string())?;
    }
    for post in batch.posts {
        let is_new = !db
            .has_post("xueqiu", &post.external_id)
            .await
            .map_err(|e| e.to_string())?;
        let images = serde_json::to_string(&post.images).unwrap_or_else(|_| "[]".into());
        db.save_fetched(
            kol_id,
            &post.external_id,
            &post.title,
            &post.content,
            &post.post_type,
            &images,
            &post.url,
            &post.published_at,
        )
        .await
        .map_err(|e| e.to_string())?;
        if !is_new {
            continue;
        }
        let push = db
            .should_push(kol_id, &post.post_type)
            .await
            .map_err(|e| e.to_string())?;
        if !push {
            continue;
        }
        crate::push::deliver(
            db,
            kol_id,
            &crate::feishu::Note {
                kol_name,
                platform: "xueqiu",
                external_id: &post.external_id,
                post_type: &post.post_type,
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

async fn pull(
    db: &Db,
    ident: &Identity,
    kol_id: i64,
    external_id: &str,
    streak: &mut HashMap<i64, u32>,
    proxy: Option<String>,
) -> Result<Batch, PullErr> {
    let uid = normalize_id(external_id);
    let cookie = ident.cookie.clone();
    let force = streak.get(&kol_id).copied().unwrap_or(0) >= 10;
    if !force {
        let probe_uid = uid.clone();
        let probe_cookie = cookie.clone();
        let probe_proxy = proxy.clone();
        let data = block(move || timeline(&probe_cookie, &probe_uid, 1, 1, probe_proxy.as_deref()))
            .await?;
        let newest = data["statuses"]
            .as_array()
            .and_then(|rows| rows.first())
            .map(field)
            .unwrap_or_default();
        if !newest.is_empty()
            && db
                .has_post("xueqiu", &newest)
                .await
                .map_err(|e| PullErr::Other(e.to_string()))?
        {
            *streak.entry(kol_id).or_insert(0) += 1;
            return Ok(Batch {
                posts: Vec::new(),
                avatar: String::new(),
            });
        }
    }
    streak.insert(kol_id, 0);
    let data = block(move || timeline(&cookie, &uid, 1, 20, proxy.as_deref())).await?;
    Ok(parse_timeline(&data))
}

async fn block<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, PullErr> + Send + 'static,
) -> Result<T, PullErr> {
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(err) => Err(PullErr::Other(err.to_string())),
    }
}

async fn ensure_identity(db: &Db) -> Result<Identity, String> {
    if let Some(raw) = db.setting(IDENTITY_KEY).await.map_err(|e| e.to_string())? {
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            if let Some(ident) = identity_from(&value, "") {
                return Ok(ident);
            }
        }
    }
    let device = device_id();
    let ident = block(move || register(&device).map_err(PullErr::Other))
        .await
        .map_err(|err| match err {
            PullErr::Other(msg) => msg,
            PullErr::Dead => "雪球注册被拒绝".into(),
        })?;
    store_identity(db, &ident).await?;
    Ok(ident)
}

async fn rotate_identity(db: &Db) -> Result<Identity, String> {
    if !rotate_due() {
        return Err("雪球身份刚换过，暂不重复注册".into());
    }
    let device = new_device();
    let ident = block(move || register(&device).map_err(PullErr::Other))
        .await
        .map_err(|err| match err {
            PullErr::Other(msg) => msg,
            PullErr::Dead => "雪球注册被拒绝".into(),
        })?;
    mark_rotated();
    store_identity(db, &ident).await?;
    Ok(ident)
}

fn rotate_clock() -> &'static std::sync::Mutex<u64> {
    static LAST: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    &LAST
}

fn rotate_due() -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let last = rotate_clock().lock().unwrap_or_else(|err| err.into_inner());
    now.saturating_sub(*last) >= 600
}

fn mark_rotated() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    *rotate_clock().lock().unwrap_or_else(|err| err.into_inner()) = now;
}

fn new_device() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).ok();
    format!(
        "1ONEPLUS{}",
        hex::encode(Md5::digest(hex::encode(buf).as_bytes()))
    )
}

fn device_id() -> String {
    if let Ok(value) = std::env::var("XUEQIU_DEVICE_ID") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    format!("1ONEPLUS{}", hex::encode(Md5::digest(b"vpush-xueqiu-app")))
}

fn empty_sign() -> String {
    let dig = Sha1::digest(format!("_secretkey={SIGN_SECRET}").as_bytes());
    hex::encode(dig)[34..].to_string()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn register(device: &str) -> Result<Identity, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(25))
        .build();
    let (secret, public) = generate_keypair();
    let stamp = format!("{device}.0.0.{}", now_ms());
    let (status, text) = send(
        &agent,
        "POST",
        &format!("{API}/ee2e/public_key.json"),
        &[("_t", stamp.as_str()), ("_s", &empty_sign())],
        device,
        Some(&json!({ "public_key": encode_public_key(&public) }).to_string()),
        false,
    )?;
    if status != 200 {
        return Err(format!("密钥协商 HTTP {status}"));
    }
    let payload: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let server_pub = payload["data"]["public_key"]
        .as_str()
        .ok_or_else(|| format!("密钥协商失败: {text}"))?;
    let shared = ecdh_shared_hex(&secret, &decode_public_key(server_pub)?)?;
    let sm4_key = derive_sm4_key(&shared);
    let form = format!(
        "client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}&sid={device}&timestamp={}&type=1&version={APP_VERSION}&nonce_str={}",
        now_ms(),
        uuid()
    );
    let cipher = sm4_encrypt_hex(&sm4_key, form.as_bytes())?;
    let stamp = format!("{device}.0.0.{}", now_ms());
    let (status, text) = send(
        &agent,
        "POST",
        &format!("{API}/uc_passport/provider/oauth/app_anonymous_id"),
        &[("_t", stamp.as_str()), ("_s", &empty_sign())],
        device,
        Some(&cipher),
        true,
    )?;
    if status != 200 {
        return Err(format!("隐式账号 HTTP {status}: {text}"));
    }
    let payload: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if payload["result_code"].as_i64().unwrap_or(-1) != 0 {
        return Err(format!("隐式账号注册失败: {text}"));
    }
    let data = payload["data"].as_str().ok_or("注册响应没有 data")?;
    let plain = sm4_decrypt_hex(&sm4_key, data)?;
    let result: Value = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    identity_from(&result, device).ok_or_else(|| "注册响应缺少 token".into())
}

fn identity_from(value: &Value, device: &str) -> Option<Identity> {
    let access_token = field_str(value, "access_token");
    let id_token = field_str(value, "id_token");
    let uid = field_str(value, "uid");
    if access_token.is_empty() || uid.is_empty() {
        return None;
    }
    let device_id = {
        let saved = field_str(value, "device_id");
        if saved.is_empty() {
            device.to_string()
        } else {
            saved
        }
    };
    let cookie = format!("xq_a_token={access_token};xq_id_token={id_token};u={uid}");
    Some(Identity {
        access_token,
        id_token,
        uid,
        device_id,
        cookie,
    })
}

fn send(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    query: &[(&str, &str)],
    device: &str,
    body: Option<&str>,
    encrypted: bool,
) -> Result<(u16, String), String> {
    let mut req = if method == "POST" {
        agent.post(url)
    } else {
        agent.get(url)
    };
    for (key, value) in query {
        req = req.query(key, value);
    }
    req = req
        .set("User-Agent", APP_UA)
        .set("Accept-Language", "en-US,en;q=0.8,zh-CN;q=0.6,zh;q=0.4")
        .set("X-Device-ID", device)
        .set("X-Device-Model-Name", "OnePlus_PJD110")
        .set("X-Device-OS", "Android 16")
        .set("Cookie", "xq_a_token=;xq_id_token=;u=0;session_id=;xid=0");
    let result = if let Some(body) = body {
        let req = if encrypted {
            req.set("isenc", "1").set(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            )
        } else {
            req.set("Content-Type", "application/json;charset=UTF-8")
        };
        req.send_string(body)
    } else {
        req.call()
    };
    read_response(result)
}

pub(crate) enum Keepalive {
    Alive,
    Dead(String),
    Transient,
}

pub(crate) fn probe_keepalive(cookie: &str, uid: &str) -> Keepalive {
    match timeline(cookie, uid, 1, 1, None) {
        Ok(_) => Keepalive::Alive,
        Err(PullErr::Dead) => Keepalive::Dead("cookie 无效或已过期（保活探测）".into()),
        Err(PullErr::Other(_)) => Keepalive::Transient,
    }
}

fn timeline(
    cookie: &str,
    uid: &str,
    page: i64,
    count: i64,
    proxy: Option<&str>,
) -> Result<Value, PullErr> {
    let agent =
        crate::proxy_admin::http_agent(proxy, Duration::from_secs(15), Duration::from_secs(20))
            .map_err(PullErr::Other)?;
    let req = agent
        .get(TIMELINE)
        .query("user_id", uid)
        .query("page", &page.to_string())
        .query("count", &count.to_string())
        .set("User-Agent", APP_UA)
        .set("Accept", "application/json, text/plain, */*")
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .set("Origin", "https://xueqiu.com")
        .set("X-Requested-With", "XMLHttpRequest")
        .set("Referer", &format!("https://xueqiu.com/u/{uid}"))
        .set("Cookie", cookie);
    let (status, text) = read_response(req.call()).map_err(PullErr::Other)?;
    if text.contains("aliyun_waf") || text.contains("acw_sc__v2") {
        return Err(PullErr::Dead);
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| PullErr::Other(e.to_string()))?;
    if status == 401 || status == 403 || session_dead(&value) {
        return Err(PullErr::Dead);
    }
    if status != 200 {
        return Err(PullErr::Other(format!("时间线 HTTP {status}")));
    }
    Ok(value)
}

fn read_response(result: Result<ureq::Response, ureq::Error>) -> Result<(u16, String), String> {
    match result {
        Ok(resp) => {
            let status = resp.status();
            resp.into_string()
                .map(|text| (status, text))
                .map_err(|e| e.to_string())
        }
        Err(ureq::Error::Status(status, resp)) => resp
            .into_string()
            .map(|text| (status, text))
            .map_err(|e| e.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

fn session_dead(value: &Value) -> bool {
    let code = field_str(value, "error_code");
    if code == "10022" || code == "400016" {
        return true;
    }
    let text = format!(
        "{} {} {}",
        field_str(value, "error_description"),
        field_str(value, "msg"),
        field_str(value, "message")
    )
    .to_lowercase();
    [
        "重新登录",
        "请登录",
        "登录帐号",
        "登录账号",
        "login",
        "cookie",
    ]
    .iter()
    .any(|m| text.contains(m))
}

fn parse_timeline(data: &Value) -> Batch {
    let rows = data["statuses"].as_array().cloned().unwrap_or_default();
    let avatar = rows.first().map(avatar_of).unwrap_or_default();
    let posts = rows.iter().filter_map(parse_status).collect();
    Batch { posts, avatar }
}

fn parse_status(status: &Value) -> Option<Fetched> {
    let post_type = classify(status)?.to_string();
    let external_id = field(status);
    if external_id.is_empty() {
        return None;
    }
    let target = status["target"].as_str().unwrap_or("");
    let url = if target.starts_with('/') {
        format!("https://xueqiu.com{target}")
    } else {
        target.to_string()
    };
    Some(Fetched {
        external_id,
        title: status["title"].as_str().unwrap_or("").to_string(),
        content: body_of(status),
        post_type,
        images: images_of(status),
        url,
        published_at: published_of(&status["created_at"]),
    })
}

fn classify(status: &Value) -> Option<&'static str> {
    let desc = status["description"].as_str().unwrap_or("").trim_start();
    if desc.starts_with("回复") && has_comment(status) {
        return Some("reply");
    }
    if !status["retweeted_status"].is_null() && status.get("retweeted_status").is_some() {
        return None;
    }
    Some("post")
}

fn has_comment(status: &Value) -> bool {
    match &status["commentId"] {
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(s) => !s.is_empty() && s != "0",
        _ => false,
    }
}

fn body_of(status: &Value) -> String {
    let content = strip_html(status["description"].as_str().unwrap_or(""));
    if content.ends_with('…') || content.ends_with("...") {
        let full = strip_html(status["text"].as_str().unwrap_or(""));
        if full.len() > content.len() {
            return full;
        }
    }
    content
}

fn images_of(status: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["original_pictures", "pics"] {
        if let Some(pics) = status[key].as_array() {
            for pic in pics {
                push_image(&mut out, pic["url"].as_str().unwrap_or(""));
                if out.len() >= 4 {
                    return out;
                }
            }
        }
    }
    for raw in status["pic"].as_str().unwrap_or("").split(',') {
        push_image(&mut out, raw);
        if out.len() >= 4 {
            break;
        }
    }
    out
}

fn push_image(out: &mut Vec<String>, raw: &str) {
    let mut url = raw.trim().to_string();
    if url.starts_with("//") {
        url = format!("https:{url}");
    }
    if let Some((base, _)) = url.split_once('!') {
        url = base.to_string();
    }
    if !url.is_empty() && !out.iter().any(|item| item == &url) {
        out.push(url);
    }
}

fn avatar_of(status: &Value) -> String {
    let user = &status["user"];
    let domain = user["photo_domain"].as_str().unwrap_or("");
    let variants: Vec<&str> = user["profile_image_url"]
        .as_str()
        .unwrap_or("")
        .split(',')
        .collect();
    let first = variants
        .get(1)
        .copied()
        .filter(|s| !s.is_empty())
        .or(variants.first().copied())
        .unwrap_or("");
    if first.is_empty() {
        return String::new();
    }
    if domain.starts_with("//") {
        format!("https:{domain}{first}")
    } else if domain.starts_with("http") {
        format!("{domain}{first}")
    } else {
        String::new()
    }
}

pub fn normalize_id(external_id: &str) -> String {
    let value = external_id.trim();
    if let Some(uid) = value.split("xueqiu.com/u/").nth(1) {
        let uid = uid.split(['?', '/', '#']).next().unwrap_or("");
        if uid.chars().all(|c| c.is_ascii_digit()) && !uid.is_empty() {
            return uid.to_string();
        }
    }
    value.to_string()
}

fn field(status: &Value) -> String {
    field_str(status, "id")
}

fn field_str(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn published_of(value: &Value) -> String {
    match value {
        Value::Number(n) => format_unix(n.as_i64().unwrap_or(0)),
        Value::String(s) => {
            if let Ok(n) = s.parse::<i64>() {
                format_unix(n)
            } else {
                s.clone()
            }
        }
        _ => String::new(),
    }
}

fn format_unix(raw: i64) -> String {
    let secs = if raw > 1_000_000_000_000 {
        raw / 1000
    } else {
        raw
    } + 8 * 3600;
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400) as u32;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60
    )
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

pub fn strip_html(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tag = &rest[start..];
        let end = tag.find('>').unwrap_or(tag.len() - 1);
        let raw = &tag[..=end.min(tag.len() - 1)];
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("<img") {
            if let Some(alt) = attr_alt(&lower, raw) {
                out.push_str(alt);
            }
        } else if lower.starts_with("<br") {
            out.push('\n');
        } else if lower.starts_with("</p")
            || lower.starts_with("</div")
            || lower.starts_with("</li")
            || lower.starts_with("</tr")
            || lower.starts_with("</blockquote")
            || (lower.starts_with("</h")
                && lower.as_bytes().get(3).is_some_and(|b| b.is_ascii_digit()))
        {
            out.push_str("\n\n");
        }
        rest = if tag.contains('>') {
            &tag[end + 1..]
        } else {
            ""
        };
    }
    out.push_str(rest);
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace('\u{00a0}', " ");
    let mut collapsed = String::new();
    let mut newlines = 0;
    for line in out.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            newlines += 1;
            continue;
        }
        if !collapsed.is_empty() {
            collapsed.push_str(if newlines >= 2 { "\n\n" } else { "\n" });
        }
        collapsed.push_str(line);
        newlines = 0;
    }
    collapsed
}

fn attr_alt<'a>(lower: &str, raw: &'a str) -> Option<&'a str> {
    let idx = lower.find("alt=")?;
    let bytes = raw.as_bytes();
    let mut i = idx + 4;
    if i >= bytes.len() {
        return None;
    }
    let quote = bytes[i];
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    i += 1;
    let start = i;
    while i < bytes.len() && bytes[i] != quote {
        i += 1;
    }
    raw.get(start..i)
}

fn uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).ok();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

async fn store_identity(db: &Db, ident: &Identity) -> Result<(), String> {
    let raw = serde_json::to_string(ident).map_err(|e| e.to_string())?;
    db.set_setting(IDENTITY_KEY, &raw)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_posts_replies_and_skips_reposts() {
        let data = json!({
            "statuses": [
                {
                    "id": 11,
                    "description": "正文<br>第二行…",
                    "text": "正文<br>第二行完整",
                    "title": "标题",
                    "target": "/123/11",
                    "created_at": 1710000000000i64,
                    "pic": "//xqimg.imedao.com/a.jpg!thumb.jpg,//xqimg.imedao.com/b.jpg!thumb.jpg",
                    "user": {"photo_domain": "//xavatar.imedao.com/", "profile_image_url": "a,b"}
                },
                {"id": 12, "description": "转发", "retweeted_status": {"id": 1}},
                {"id": 13, "description": "回复<a href=\"/n/foo\">@foo</a>: 好", "commentId": 9, "created_at": "1710000000"}
            ]
        });
        let batch = parse_timeline(&data);
        assert_eq!(batch.posts.len(), 2);
        assert_eq!(batch.posts[0].post_type, "post");
        assert_eq!(batch.posts[0].content, "正文\n第二行完整");
        assert_eq!(batch.posts[0].url, "https://xueqiu.com/123/11");
        assert_eq!(
            batch.posts[0].images,
            vec![
                "https://xqimg.imedao.com/a.jpg".to_string(),
                "https://xqimg.imedao.com/b.jpg".to_string()
            ]
        );
        assert_eq!(batch.posts[0].published_at, "2024-03-10 00:00");
        assert_eq!(batch.avatar, "https://xavatar.imedao.com/b");
        assert_eq!(batch.posts[1].post_type, "reply");
        assert_eq!(batch.posts[1].content, "回复@foo: 好");
        assert_eq!(empty_sign(), "9667ca");
        assert_eq!(
            normalize_id("https://xueqiu.com/u/4514680565"),
            "4514680565"
        );
    }
}
