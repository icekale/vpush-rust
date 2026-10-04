//! 腾讯 IMA 的登录、知识库列表和 PDF 下载。

use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use base64::Engine;
use rand::rngs::OsRng;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Oaep, RsaPublicKey};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashSet;

const PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAx9h6SY1LO88wRVKdOC5U\n\
tjYTXfMpUqCK22FemW9ba4812nVjF4Va+guoHXdBePkhsQmz94PeqqZiSN/YekiV\n\
CU6HbWivdhKcs6LcYMT+sw4cwtMZ1NOJEkY1ujOWFnmNmpK243wwlf+RwE/L+xOJ\n\
8vy/EBuYDv06M+BbccO832bOpWsOFYPPP5KtOmOaqXq6Fgu1vOrXmQIo0q8WmO09\n\
PvjHLwIruqthV2dBcVI1qMEKejM1SKwzCWb78t+fUsr3OjDqApWma3h10hGKcin4\n\
NIGdfITwmiBmS+R1Mr8P/ssNq0ptvr9+VqUvsJD7ASVCPo9EG658fZYGil6oH5JN\n\
OQIDAQAB\n\
-----END PUBLIC KEY-----\n";

pub struct File {
    pub media_id: String,
    pub name: String,
    pub day: String,
    pub sort_date: String,
    pub size: i64,
    pub text: String,
}

pub trait Transport: Send + Sync {
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> Result<Value, String>;

    async fn post_text(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> Result<String, String> {
        let _ = (url, headers, body);
        Err("IMA 正文请求未实现".into())
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let _ = url;
        Err("IMA 文件下载未实现".into())
    }
}

pub struct Live {
    proxy: Option<String>,
}

impl Live {
    pub fn proxied(proxy: Option<String>) -> Self {
        Self { proxy }
    }
}

impl Transport for Live {
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> Result<Value, String> {
        let url = url.to_string();
        let body = body.to_string();
        let proxy = self.proxy.clone();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        tokio::task::spawn_blocking(move || live_post(&url, &headers, &body, proxy.as_deref()))
            .await
            .map_err(|_| "IMA 请求中断".to_string())?
    }

    async fn post_text(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> Result<String, String> {
        let url = url.to_string();
        let body = body.to_string();
        let proxy = self.proxy.clone();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        tokio::task::spawn_blocking(move || live_text(&url, &headers, &body, proxy.as_deref()))
            .await
            .map_err(|_| "IMA 请求中断".to_string())?
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let url = url.to_string();
        let proxy = self.proxy.clone();
        tokio::task::spawn_blocking(move || live_bytes(&url, proxy.as_deref()))
            .await
            .map_err(|_| "IMA 请求中断".to_string())?
    }
}

pub struct Session {
    token: String,
    uid: String,
}

pub async fn refresh(
    http: &impl Transport,
    base: &str,
    uid: &str,
    refresh_token: &str,
) -> Result<Session, String> {
    let data = http
        .post(
            &format!("{base}/oversea/auth_login/refresh"),
            &[("Content-Type", "application/json".into())],
            &json!({"user_id": uid, "refresh_token": refresh_token}).to_string(),
        )
        .await?;
    let token = data["token"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("IMA 登录没有返回 token")?;
    Ok(Session {
        token: token.to_string(),
        uid: uid.to_string(),
    })
}

pub async fn list_pdfs(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    folders: &[String],
) -> Result<Vec<File>, String> {
    let mut files = Vec::new();
    for folder in folders {
        let mut cursor = String::new();
        for _ in 0..20 {
            let body = json!({
                "knowledge_base_id": knowledge_base_id,
                "folder_id": folder,
                "cursor": cursor,
                "limit": 50,
                "need_file_size": true,
                "version": "1"
            });
            let data = http
                .post(
                    &format!("{base}/knowledge_tab_reader/get_knowledge_list"),
                    &headers(session),
                    &body.to_string(),
                )
                .await?;
            let items = data["knowledge_list"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for item in items {
                if let Some(file) = pdf_file(&item) {
                    files.push(file);
                }
            }
            let next = data["cursor"].as_str().unwrap_or("").trim();
            if data["is_end"].as_bool().unwrap_or(true) || next.is_empty() || next == cursor {
                break;
            }
            cursor = next.to_string();
        }
    }
    Ok(files)
}

/// 递归列出根目录下所有 PDF。原来只调 list_pdfs 列挂载根目录的直接文件，
/// 上游三万个研报就只能看到两个 —— 子目录得自己走下去。
pub async fn list_pdfs_deep(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    roots: &[String],
) -> Result<Vec<File>, String> {
    let mut files = list_pdfs(http, base, session, knowledge_base_id, roots).await?;
    let mut seen: HashSet<String> = roots.iter().cloned().collect();
    let mut level = child_folders(http, base, session, knowledge_base_id, roots, None).await?;
    // seen 同时挡环形 parent，所以不需要深度上限
    while !level.is_empty() {
        let results = stream::iter(level.into_iter().map(|(id, day)| async move {
            scan_folder(
                http,
                base,
                session,
                knowledge_base_id,
                id,
                day,
            )
            .await
        }))
        .buffer_unordered(8)
        .try_collect::<Vec<_>>()
        .await?;
        let mut next = Vec::new();
        for (mut folder_files, children) in results {
            files.append(&mut folder_files);
            next.extend(children);
        }
        level = next
            .into_iter()
            .filter(|(id, _)| seen.insert(id.clone()))
            .collect();
    }
    Ok(files)
}

async fn scan_folder(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    id: String,
    day: Option<String>,
) -> Result<(Vec<File>, Vec<(String, Option<String>)>), String> {
    let ids = [id.clone()];
    let (files, children) = tokio::join!(
        list_pdfs(http, base, session, knowledge_base_id, &ids),
        child_folders(http, base, session, knowledge_base_id, &ids, day.as_deref()),
    );
    let mut files = files?;
    if let Some(day) = day.as_deref() {
        for file in &mut files {
            stamp_day(file, day);
        }
    }
    Ok((files, children?))
}

/// 列一层子目录，并带上从祖先继承下来的 4 位月日（0929 这类目录名）。
async fn child_folders(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    parents: &[String],
    inherited: Option<&str>,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut found = Vec::new();
    for parent in parents {
        for item in list_folders(http, base, session, knowledge_base_id, parent).await? {
            let Some(id) = item["id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            let day = item["name"]
                .as_str()
                .and_then(mmdd)
                .map(str::to_string)
                .or_else(|| inherited.map(str::to_string));
            found.push((id.to_string(), day));
        }
    }
    Ok(found)
}

/// 4 位纯数字目录名（0929）= 那一天的归档目录。
fn mmdd(name: &str) -> Option<&str> {
    (name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_digit())).then_some(name)
}

/// 目录给出的月日优先，create_time 只负责年份。
/// 上游 create_time 经常是 0，以前这种行会写成 day='unknown'/sort_date='' 而沉在列表最底。
fn stamp_day(file: &mut File, day: &str) {
    let year = file
        .sort_date
        .get(..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_string)
        .unwrap_or_else(current_year);
    file.day = day.to_string();
    file.sort_date = format!("{year}-{}-{}", &day[..2], &day[2..]);
}

/// 当前年份（北京时区，与上面 MMDD 同一套换算）。
fn current_year() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    civil(ms)
        .map(|(year, _, _)| format!("{year:04}"))
        .unwrap_or_else(|| "1970".to_string())
}

/// 排序键是否落在最近 `days` 天内（北京时区，与 MMDD 同一套换算）。
/// 用来判断「新研报」：老研报只建档、不下载。
pub fn fresh_sort_date(sort_date: &str, days: i64) -> bool {
    let Ok(since_epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return false;
    };
    let ms = since_epoch.as_millis() as i64 - days * 86_400_000;
    let Some((year, month, day)) = civil(ms) else {
        return false;
    };
    // 都是 YYYY-MM-DD，字典序就是时间序
    sort_date >= format!("{year:04}-{month:02}-{day:02}").as_str()
}

pub async fn list_folders(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    folder: &str,
) -> Result<Vec<Value>, String> {
    let mut items = Vec::new();
    let mut seen_ids = Vec::new();
    let mut seen_cursors = Vec::new();
    let mut cursor = String::new();
    for _ in 0..20 {
        if seen_cursors.iter().any(|item: &String| item == &cursor) {
            return Err("IMA 列表分页重复".into());
        }
        seen_cursors.push(cursor.clone());
        let mut body =
            json!({"knowledge_base_id": knowledge_base_id, "folder_id": folder, "limit": "50"});
        if !cursor.is_empty() {
            body["cursor"] = json!(cursor);
        }
        let data = match http
            .post(
                &format!("{base}/knowledge_tab_reader/get_knowledge_list"),
                &headers(session),
                &body.to_string(),
            )
            .await
        {
            Ok(data) => data,
            Err(err) if !items.is_empty() => {
                let _ = err;
                return Ok(items);
            }
            Err(err) => return Err(err),
        };
        let page = data["knowledge_list"]
            .as_array()
            .cloned()
            .or_else(|| data["data"]["knowledge_list"].as_array().cloned())
            .ok_or("IMA 列表返回无效")?;
        let mut folders = 0;
        for item in page {
            let Some(folder_item) = folder_item(&item, folder) else {
                continue;
            };
            let id = folder_item["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() || seen_ids.iter().any(|item: &String| item == &id) {
                continue;
            }
            seen_ids.push(id);
            items.push(folder_item);
            folders += 1;
        }
        if folders == 0 {
            return Ok(items);
        }
        let next = data["next_cursor"]
            .as_str()
            .or_else(|| data["data"]["next_cursor"].as_str())
            .unwrap_or("")
            .trim();
        if next.is_empty() {
            return Ok(items);
        }
        cursor = next.to_string();
    }
    Ok(items)
}

fn folder_item(item: &Value, parent_id: &str) -> Option<Value> {
    if !is_folder(item) {
        return None;
    }
    let id = folder_id(item)?;
    let mut out = json!({"id": id, "name": folder_name(item, &id), "parent_id": parent_id, "has_children": Value::Null});
    if let Some(count) = count_of(
        item,
        &[
            "folder_number",
            "sub_folder_count",
            "children_count",
            "child_count",
        ],
    ) {
        out["has_children"] = json!(count > 0);
    }
    if let Some(count) = count_of(item, &["folder_number", "sub_folder_count"]) {
        out["folder_count"] = json!(count);
    }
    if let Some(count) = count_of(item, &["file_number", "file_count"]) {
        out["file_count"] = json!(count);
    }
    Some(out)
}

fn is_folder(item: &Value) -> bool {
    let info_id = folder_text(&item["folder_info"]["folder_id"]);
    if info_id.is_some() {
        return true;
    }
    let media_type = item["media_type"].as_i64().or_else(|| {
        item["media_type"]
            .as_str()
            .and_then(|text| text.parse().ok())
    });
    folder_id(item).is_some()
        && (media_type == Some(99)
            || item["media_id"]
                .as_str()
                .is_some_and(|text| text.starts_with("folder_"))
            || item.get("media_id").is_none_or(Value::is_null))
}

fn folder_id(item: &Value) -> Option<String> {
    folder_text(&item["folder_info"]["folder_id"])
        .or_else(|| folder_text(&item["folder_id"]))
        .or_else(|| {
            item["media_id"]
                .as_str()
                .filter(|text| text.starts_with("folder_"))
                .and_then(folder_text_str)
        })
}

fn folder_name(item: &Value, id: &str) -> String {
    item["folder_info"]["name"]
        .as_str()
        .or_else(|| item["name"].as_str())
        .or_else(|| item["title"].as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(id)
        .chars()
        .take(200)
        .collect()
}

fn count_of(item: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(count) = number_at(&item[key]).or_else(|| number_at(&item["folder_info"][key]))
        {
            return Some(count.max(0));
        }
    }
    None
}

fn number_at(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn folder_text(value: &Value) -> Option<String> {
    value.as_str().and_then(folder_text_str)
}

fn folder_text_str(text: &str) -> Option<String> {
    let text = text.trim();
    if (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':'))
    {
        Some(text.to_string())
    } else {
        None
    }
}

fn pdf_file(item: &Value) -> Option<File> {
    let media_id = item["media_id"]
        .as_str()
        .or_else(|| item["source_path"].as_str())?
        .trim();
    if media_id.is_empty()
        || item["media_type"].as_i64() == Some(99)
        || item["media_type"].as_str() == Some("99")
    {
        return None;
    }
    let name = item["title"]
        .as_str()
        .or_else(|| item["name"].as_str())
        .unwrap_or("")
        .trim();
    if !name.to_ascii_lowercase().ends_with(".pdf") {
        return None;
    }
    let size = item["file_size"]
        .as_i64()
        .or_else(|| item["file_size"].as_str().and_then(|v| v.parse().ok()))
        .unwrap_or(0);
    let (day, sort_date) = dates(item["create_time"].as_i64().unwrap_or(0));
    Some(File {
        media_id: media_id.to_string(),
        name: name.to_string(),
        day,
        sort_date,
        size,
        text: item["abstract"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(2000)
            .collect(),
    })
}

fn dates(ms: i64) -> (String, String) {
    let Some((y, m, d)) = civil(ms) else {
        return ("unknown".into(), String::new());
    };
    (format!("{m:02}{d:02}"), format!("{y:04}-{m:02}-{d:02}"))
}

fn civil(ms: i64) -> Option<(i32, u32, u32)> {
    if ms <= 0 {
        return None;
    }
    let days = (ms / 1000 + 8 * 3600).div_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    Some((y as i32, m as u32, d as u32))
}

fn headers(session: &Session) -> Vec<(&str, String)> {
    let guid = std::env::var("IMA_GUID").unwrap_or_else(|_| "7497986728819336".into());
    let ver = std::env::var("IMA_APP_VER").unwrap_or_else(|_| "2.6.7.0515".into());
    let q36 =
        std::env::var("IMA_Q36").unwrap_or_else(|_| "07cf2a0a18c8863cfbfa8957100013719518".into());
    let iua = std::env::var("IMA_IUA").unwrap_or_else(|_| format!("PR=IMA&PP=com.tencent.ima&PPVN={ver}&PL=ADR&DN=OnePlus+PJD110&MO=PJD110&RL=1440*3168&OS=16"));
    let cookie = format!("IMA-GUID={guid};APP-VERSION={ver};IMA-Q36={q36};IMA-IUA={iua};UID-TYPE=2;IMA-UID={};IMA-TOKEN={};IMA-TOKEN-TYPE=IDC_TOKEN_IMATOKEN_BIND_SOCIAL;CLIENT-TYPE=256001", session.uid, session.token);
    vec![
        ("Content-Type", "application/json".into()),
        ("User-Agent", "okhttp/4.12.0".into()),
        ("from_browser_ima", "1".into()),
        ("x-ima-cookie", cookie),
        ("x-ima-bkn", bkn(&session.token).to_string()),
        ("referer", "https://ima.qq.com".into()),
        ("origin", "https://ima.qq.com".into()),
    ]
}

fn bkn(token: &str) -> u32 {
    let mut value: u32 = 5381;
    for byte in token.chars() {
        value = value.wrapping_mul(33).wrapping_add(byte as u32);
    }
    value & 0x7FFF_FFFF
}

fn live_post(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    proxy: Option<&str>,
) -> Result<Value, String> {
    let mut req = crate::proxy_admin::http_agent(
        proxy,
        std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(30),
    )?
    .post(url);
    for (key, value) in headers {
        req = req.set(key, value);
    }
    let response = req
        .send_string(body)
        .map_err(|_| "IMA 网络请求失败".to_string())?;
    let text = response
        .into_string()
        .map_err(|_| "IMA 返回的不是 JSON".to_string())?;
    serde_json::from_str(&text).map_err(|_| "IMA 返回的不是 JSON".to_string())
}

pub async fn fetch_pdf(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    media_id: &str,
) -> Result<Vec<u8>, String> {
    let key = RsaPublicKey::from_public_key_pem(PUB_PEM).map_err(|_| "IMA 公钥无效".to_string())?;
    fetch_pdf_with(http, base, session, knowledge_base_id, media_id, &key).await
}

pub async fn fetch_pdf_with(
    http: &impl Transport,
    base: &str,
    session: &Session,
    knowledge_base_id: &str,
    media_id: &str,
    key: &RsaPublicKey,
) -> Result<Vec<u8>, String> {
    let plain =
        json!({"media_id": media_id, "source_knowledge_base_id": knowledge_base_id}).to_string();
    let (aes, body, wrapped) = encrypt_with(key, plain.as_bytes())?;
    let mut headers = headers(session);
    headers.push(("x-ima-cm", "1".into()));
    headers.push(("x-ima-ckey", wrapped));
    let raw = http
        .post_text(&format!("{base}/s/file_manager/get_media"), &headers, &body)
        .await?;
    let decoded = decrypt_body(&raw, &aes)?;
    let result: Value =
        serde_json::from_slice(&decoded).map_err(|_| "IMA 正文返回无法解密".to_string())?;
    if result["code"].as_i64() != Some(0) {
        return Err("IMA 没有返回下载地址".into());
    }
    let url = result["jump_url_info"]["url"]
        .as_str()
        .or_else(|| result["jump_url"].as_str())
        .unwrap_or("");
    if !url.starts_with("https://") {
        return Err("IMA 没有返回下载地址".into());
    }
    let bytes = http.get_bytes(url).await?;
    if bytes.len() > 50 * 1024 * 1024 || !bytes.starts_with(b"%PDF") {
        return Err("IMA 返回的不是 PDF".into());
    }
    Ok(bytes)
}

fn encrypt_with(key: &RsaPublicKey, plain: &[u8]) -> Result<(Vec<u8>, String, String), String> {
    let mut aes = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::RngCore::fill_bytes(&mut OsRng, &mut aes);
    rand::RngCore::fill_bytes(&mut OsRng, &mut nonce);
    let cipher = Aes128Gcm::new_from_slice(&aes).map_err(|_| "加密失败".to_string())?;
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| "加密失败".to_string())?;
    let mut packed = nonce.to_vec();
    packed.extend(encrypted);
    let wrapped = key
        .encrypt(&mut OsRng, Oaep::new::<Sha256>(), &aes)
        .map_err(|_| "加密失败".to_string())?;
    Ok((
        aes.to_vec(),
        STANDARD.encode(packed),
        STANDARD.encode(wrapped),
    ))
}

fn decrypt_body(body: &str, key: &[u8]) -> Result<Vec<u8>, String> {
    let raw = STANDARD
        .decode(body.trim())
        .map_err(|_| "IMA 正文返回无法解密".to_string())?;
    if raw.len() < 12 + 16 {
        return Err("IMA 正文返回无法解密".into());
    }
    let cipher = Aes128Gcm::new_from_slice(key).map_err(|_| "IMA 正文返回无法解密".to_string())?;
    cipher
        .decrypt(Nonce::from_slice(&raw[..12]), &raw[12..])
        .map_err(|_| "IMA 正文返回无法解密".to_string())
}

fn live_text(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    proxy: Option<&str>,
) -> Result<String, String> {
    let mut req = crate::proxy_admin::http_agent(
        proxy,
        std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(30),
    )?
    .post(url);
    for (key, value) in headers {
        req = req.set(key, value);
    }
    req.send_string(body)
        .map_err(|_| "IMA 网络请求失败".to_string())?
        .into_string()
        .map_err(|_| "IMA 正文返回无法解密".to_string())
}

fn live_bytes(url: &str, proxy: Option<&str>) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("IMA 没有返回下载地址".into());
    }
    let response = crate::proxy_admin::http_agent(
        proxy,
        std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(60),
    )?
    .get(url)
    .call()
    .map_err(|_| "IMA 文件下载失败".to_string())?;
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if bytes.len() > 50 * 1024 * 1024 {
            return Err("IMA 文件过大".into());
        }
        let read = std::io::Read::read(&mut reader, &mut chunk)
            .map_err(|_| "IMA 文件下载失败".to_string())?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(bytes)
}

const STANDARD: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub fn base() -> String {
    std::env::var("IMA_BASE").unwrap_or_else(|_| "https://ima.qq.com/cgi-bin".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;

    struct Script {
        private: RsaPrivateKey,
    }

    impl Transport for Script {
        async fn post(&self, url: &str, _: &[(&str, String)], _: &str) -> Result<Value, String> {
            let _ = url;
            Ok(Value::Null)
        }

        async fn post_text(
            &self,
            _: &str,
            headers: &[(&str, String)],
            body: &str,
        ) -> Result<String, String> {
            let wrapped = headers
                .iter()
                .find(|(key, _)| *key == "x-ima-ckey")
                .map(|(_, value)| value.clone())
                .unwrap();
            let aes = self
                .private
                .decrypt(Oaep::new::<Sha256>(), &STANDARD.decode(wrapped).unwrap())
                .unwrap();
            let plain = decrypt_body(body, &aes).unwrap();
            assert!(plain.windows(b"m1".len()).any(|item| item == b"m1"));
            let reply = br#"{"code":0,"jump_url_info":{"url":"https://files.test/a.pdf"}}"#;
            let cipher = Aes128Gcm::new_from_slice(&aes).unwrap();
            let mut nonce = [0u8; 12];
            rand::RngCore::fill_bytes(&mut OsRng, &mut nonce);
            let encrypted = cipher
                .encrypt(Nonce::from_slice(&nonce), reply.as_ref())
                .unwrap();
            let mut raw = nonce.to_vec();
            raw.extend(encrypted);
            Ok(STANDARD.encode(raw))
        }

        async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
            assert_eq!(url, "https://files.test/a.pdf");
            Ok(b"%PDF-1.4\n%".to_vec())
        }
    }

    #[tokio::test]
    async fn downloads_pdf_through_encrypted_media_url() {
        let private = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let http = Script {
            private: private.clone(),
        };
        let bytes = fetch_pdf_with(
            &http,
            "https://ima.test",
            &Session {
                token: "t".into(),
                uid: "u".into(),
            },
            "kb",
            "m1",
            &RsaPublicKey::from(&private),
        )
        .await
        .unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }

    struct FolderPages {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Transport for FolderPages {
        async fn post(&self, url: &str, _: &[(&str, String)], _: &str) -> Result<Value, String> {
            assert!(url.ends_with("/get_knowledge_list"));
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                return Ok(json!({"knowledge_list": [
                    {"folder_info": {"folder_id": "folder_a", "name": "2026年8月", "folder_number": 1, "file_number": "2"}},
                    {"media_id": "m1", "title": "报告.pdf", "media_type": 1}
                ], "next_cursor": "c2"}));
            }
            Ok(
                json!({"knowledge_list": [{"media_id": "m2", "title": "另一份.pdf"}], "next_cursor": "c3"}),
            )
        }
    }

    #[tokio::test]
    async fn lists_only_folders_and_stops_when_a_page_has_none() {
        let http = FolderPages {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let items = list_folders(
            &http,
            "https://ima.test",
            &Session {
                token: "t".into(),
                uid: "u".into(),
            },
            "kb",
            "root",
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "folder_a");
        assert_eq!(items[0]["name"], "2026年8月");
        assert_eq!(items[0]["parent_id"], "root");
        assert_eq!(items[0]["has_children"], true);
        assert_eq!(items[0]["folder_count"], 1);
        assert_eq!(items[0]["file_count"], 2);
        assert_eq!(http.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn fresh_sort_date_keeps_only_recent_days() {
        let days_ago = |offset: i64| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            let (year, month, day) = civil(now - offset * 86_400_000).unwrap();
            format!("{year:04}-{month:02}-{day:02}")
        };
        assert!(fresh_sort_date(&days_ago(0), 3));
        assert!(fresh_sort_date(&days_ago(2), 3));
        assert!(!fresh_sort_date(&days_ago(4), 3));
        assert!(!fresh_sort_date("2026-09-27", 3));
        // 没有日期的行（day='unknown'）不能进下载队列
        assert!(!fresh_sort_date("", 3));
    }

    struct Tree;

    impl Transport for Tree {
        async fn post(&self, url: &str, _: &[(&str, String)], body: &str) -> Result<Value, String> {
            assert!(url.ends_with("/get_knowledge_list"));
            let folder = serde_json::from_str::<Value>(body).unwrap()["folder_id"]
                .as_str()
                .unwrap()
                .to_string();
            let list = match folder.as_str() {
                "root" => json!([
                    {"media_id": "f-root", "title": "根目录报告.pdf", "media_type": 1},
                    {"folder_info": {"folder_id": "A", "name": "2026年9月"}}
                ]),
                "A" => json!([
                    {"media_id": "f-a", "title": "报告A.pdf", "media_type": 1},
                    {"folder_info": {"folder_id": "B", "name": "0929"}}
                ]),
                // B 里放一个指回 root 的目录项：环形 parent 不能把递归绕死
                "B" => json!([
                    {"media_id": "f-b", "title": "报告B.pdf", "media_type": 1},
                    {"folder_info": {"folder_id": "root", "name": "根"}}
                ]),
                _ => json!([]),
            };
            Ok(json!({"knowledge_list": list}))
        }
    }

    #[tokio::test]
    async fn lists_pdfs_in_subfolders_and_survives_a_cycle() {
        let session = Session {
            token: "t".into(),
            uid: "u".into(),
        };
        let files = list_pdfs_deep(
            &Tree,
            "https://ima.test",
            &session,
            "kb",
            &["root".to_string()],
        )
        .await
        .unwrap();
        let mut ids: Vec<String> = files.iter().map(|file| file.media_id.clone()).collect();
        ids.sort();
        assert_eq!(ids, ["f-a", "f-b", "f-root"]);
        // 0929 目录下的文档用目录名当日期，create_time 缺失也不再沉底
        let by_id = |wanted: &str| files.iter().find(|file| file.media_id == wanted).unwrap();
        assert_eq!(by_id("f-b").day, "0929");
        assert!(by_id("f-b").sort_date.ends_with("-09-29"), "{}", by_id("f-b").sort_date);
        assert_eq!(by_id("f-a").day, "unknown");
    }
}
