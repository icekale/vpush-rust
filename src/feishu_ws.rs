use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::db::Db;
use crate::feishu_personal::{self, accept_bind};

static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn listeners_enabled() -> bool {
    std::env::var("FEISHU_PERSONAL_LISTENER").ok().as_deref() != Some("0")
}

pub fn ensure_listener(db: Db, session_id: String) {
    if !listeners_enabled() {
        return;
    }
    let mut guard = RUNNING.lock().expect("feishu listeners");
    let running = guard.get_or_insert_with(HashSet::new);
    if !running.insert(session_id.clone()) {
        return;
    }
    drop(guard);
    tokio::spawn(async move {
        if let Err(err) = run(&db, &session_id).await {
            tracing::warn!("飞书个人机器人长连接结束 {session_id}: {err}");
        }
        if let Some(running) = RUNNING.lock().expect("feishu listeners").as_mut() {
            running.remove(&session_id);
        }
    });
}

pub fn spawn_shared(db: Db) {
    if std::env::var("FEISHU_SHARED_LISTENER").ok().as_deref() == Some("0") {
        return;
    }
    let app_id = std::env::var("FEISHU_APP_ID").unwrap_or_default();
    let secret = std::env::var("FEISHU_APP_SECRET").unwrap_or_default();
    if app_id.trim().is_empty() || secret.trim().is_empty() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = run_shared(&db, &app_id, &secret).await {
                tracing::warn!("飞书共享机器人将重试: {err}");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn run_shared(db: &Db, app_id: &str, secret: &str) -> Result<(), String> {
    let endpoint = tokio::task::spawn_blocking({
        let app_id = app_id.to_string();
        let secret = secret.to_string();
        move || fetch_endpoint("https://open.feishu.cn", &app_id, &secret)
    })
    .await
    .map_err(|err| err.to_string())??;
    let (mut socket, _) = tokio_tungstenite::connect_async(&endpoint.url)
        .await
        .map_err(|err| err.to_string())?;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    socket
        .send(Message::Binary(
            encode_frame(&ping_frame(endpoint.service)).into(),
        ))
        .await
        .map_err(|err| err.to_string())?;
    loop {
        let message = tokio::time::timeout(Duration::from_secs(20), socket.next()).await;
        let message = match message {
            Err(_) => {
                socket
                    .send(Message::Binary(
                        encode_frame(&ping_frame(endpoint.service)).into(),
                    ))
                    .await
                    .map_err(|err| err.to_string())?;
                continue;
            }
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(err))) => return Err(err.to_string()),
            Ok(None) => return Err("连接已关闭".into()),
        };
        let bytes = match message {
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Close(_) => return Err("连接已关闭".into()),
            _ => continue,
        };
        let (reply, outgoing) = handle_shared_frame(db, &bytes, now_secs()).await?;
        if let Some((chat_id, text)) = outgoing {
            crate::feishu::send_shared_text(&chat_id, &text).await?;
        }
        if let Some(reply) = reply {
            socket
                .send(Message::Binary(reply.into()))
                .await
                .map_err(|err| err.to_string())?;
        }
    }
}

pub async fn resume(db: Db) {
    if !listeners_enabled() {
        return;
    }
    let Ok(rows) = db.feishu_sessions_by_status("awaiting_bind").await else {
        return;
    };
    for session in rows {
        ensure_listener(db.clone(), session.session_id);
    }
}

async fn run(db: &Db, session_id: &str) -> Result<(), String> {
    loop {
        let Some(session) = db
            .feishu_session(session_id)
            .await
            .map_err(|err| err.to_string())?
        else {
            return Ok(());
        };
        if session.status != "awaiting_bind" {
            return Ok(());
        }
        let now = now_secs();
        if session.session_expires_at < now {
            db.set_feishu_status(session_id, "expired", "")
                .await
                .map_err(|err| err.to_string())?;
            return Ok(());
        }
        let Some(key) = std::env::var("FEISHU_CREDENTIAL_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Err("未配置 FEISHU_CREDENTIAL_KEY".into());
        };
        let secret =
            feishu_personal::open_app_secret(&key, &session.candidate_app_secret_ciphertext)?;
        let brand = session.candidate_tenant_brand.clone();
        let app_id = session.candidate_app_id.clone();
        let domain = if brand == "lark" {
            "https://open.larksuite.com"
        } else {
            "https://open.feishu.cn"
        };
        let endpoint = tokio::task::spawn_blocking({
            let app_id = app_id.to_string();
            let secret = secret.to_string();
            let domain = domain.to_string();
            move || fetch_endpoint(&domain, &app_id, &secret)
        })
        .await
        .map_err(|err| err.to_string())??;
        match serve(db, session_id, &endpoint, &app_id, &secret, &brand).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                tracing::warn!("飞书长连接将重试 {session_id}: {err}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn serve(
    db: &Db,
    session_id: &str,
    endpoint: &Endpoint,
    app_id: &str,
    secret: &str,
    brand: &str,
) -> Result<(), String> {
    let (mut socket, _) = tokio_tungstenite::connect_async(&endpoint.url)
        .await
        .map_err(|err| err.to_string())?;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    socket
        .send(Message::Binary(
            encode_frame(&ping_frame(endpoint.service)).into(),
        ))
        .await
        .map_err(|err| err.to_string())?;
    loop {
        let Some(session) = db
            .feishu_session(session_id)
            .await
            .map_err(|err| err.to_string())?
        else {
            return Ok(());
        };
        if session.status != "awaiting_bind" {
            return Ok(());
        }
        let message = tokio::time::timeout(Duration::from_secs(20), socket.next()).await;
        let message = match message {
            Err(_) => {
                socket
                    .send(Message::Binary(
                        encode_frame(&ping_frame(endpoint.service)).into(),
                    ))
                    .await
                    .map_err(|err| err.to_string())?;
                continue;
            }
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(err))) => return Err(err.to_string()),
            Ok(None) => return Err("连接已关闭".into()),
        };
        let bytes = match message {
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Close(_) => return Err("连接已关闭".into()),
            _ => continue,
        };
        let app_id = app_id.to_string();
        let secret = secret.to_string();
        let brand = brand.to_string();
        let reply = handle_frame(
            db,
            session_id,
            &bytes,
            now_secs(),
            move |_, _, chat, host| send_notice(&app_id, &secret, chat, host, &brand),
        )
        .await
        .map_err(|err| err.to_string())?;
        if let Some(reply) = reply {
            socket
                .send(Message::Binary(reply.into()))
                .await
                .map_err(|err| err.to_string())?;
        }
    }
}

pub async fn handle_frame<F>(
    db: &Db,
    session_id: &str,
    bytes: &[u8],
    now: i64,
    send_test: F,
) -> Result<Option<Vec<u8>>, String>
where
    F: FnOnce(&str, &str, &str, &str) -> Result<(), String>,
{
    let mut frame = decode_frame(bytes)?;
    if frame.method != 1 || header(&frame, "type") != "event" {
        return Ok(None);
    }
    if let Ok(payload) = serde_json::from_slice::<Value>(&frame.payload) {
        if payload
            .pointer("/header/event_type")
            .and_then(Value::as_str)
            == Some("im.message.receive_v1")
        {
            let message_type = payload
                .pointer("/event/message/message_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            if message_type == "text" {
                let content = payload
                    .pointer("/event/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = serde_json::from_str::<Value>(content)
                    .ok()
                    .and_then(|item| item.get("text").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                let text = text
                    .split_whitespace()
                    .filter(|part| !part.starts_with('@'))
                    .collect::<Vec<_>>()
                    .join(" ");
                let chat_id = payload
                    .pointer("/event/message/chat_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let sender = payload
                    .pointer("/event/sender/sender_id/open_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(code) = feishu_personal::parse_bind_code(&text) {
                    accept_bind(db, session_id, code, sender, chat_id, now, send_test)
                        .await
                        .map_err(|err| format!("{err:?}"))?;
                }
            }
        }
    }
    frame.headers.push(Header {
        key: "biz_rt".into(),
        value: "1".into(),
    });
    frame.payload = br#"{"code":200}"#.to_vec();
    Ok(Some(encode_frame(&frame)))
}

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub async fn handle_shared_frame(
    db: &Db,
    bytes: &[u8],
    now: i64,
) -> Result<(Option<Vec<u8>>, Option<(String, String)>), String> {
    let mut frame = decode_frame(bytes)?;
    if frame.method != 1 || header(&frame, "type") != "event" {
        return Ok((None, None));
    }
    let payload: Value = serde_json::from_slice(&frame.payload).unwrap_or(Value::Null);
    let outgoing = match payload
        .pointer("/header/event_type")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "im.message.receive_v1" => shared_message(db, &payload, now).await?,
        "card.action.trigger" => shared_card(db, &payload, now).await?,
        _ => None,
    };
    frame.headers.push(Header {
        key: "biz_rt".into(),
        value: "1".into(),
    });
    frame.payload = br#"{"code":200}"#.to_vec();
    Ok((Some(encode_frame(&frame)), outgoing))
}

async fn shared_message(
    db: &Db,
    payload: &Value,
    now: i64,
) -> Result<Option<(String, String)>, String> {
    let message_id = payload
        .pointer("/event/message/message_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !remember_message(message_id) {
        return Ok(None);
    }
    if payload
        .pointer("/event/message/chat_type")
        .and_then(Value::as_str)
        != Some("p2p")
        || payload
            .pointer("/event/message/message_type")
            .and_then(Value::as_str)
            != Some("text")
    {
        return Ok(None);
    }
    let chat_id = payload
        .pointer("/event/message/chat_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let open_id = payload
        .pointer("/event/sender/sender_id/open_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = payload
        .pointer("/event/message/content")
        .and_then(Value::as_str)
        .unwrap_or("");
    let text = serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|item| item.get("text").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let text = text
        .split_whitespace()
        .filter(|part| !part.starts_with('@'))
        .collect::<Vec<_>>()
        .join(" ");
    if chat_id.is_empty() || open_id.is_empty() || text.is_empty() {
        return Ok(None);
    }
    if let Some(raw_code) = crate::telegram_bot::bind_request(&text) {
        let Some(code) = crate::telegram_bot::normalize_bind_code(&raw_code) else {
            return Ok(Some((chat_id.to_owned(), "绑定码无效或已过期。".into())));
        };
        db.release_feishu_placeholder(open_id)
            .await
            .map_err(|err| err.to_string())?;
        let reply = match db
            .consume_bind_code(&code, "feishu_open_id", open_id, now)
            .await
        {
            Ok(Some(user_id)) => {
                db.set_user_text(user_id, "feishu_chat_id", chat_id)
                    .await
                    .map_err(|err| err.to_string())?;
                "绑定成功。发送 /mysubs 查看订阅。".to_owned()
            }
            Ok(None) => "绑定码无效或已过期。".to_owned(),
            Err(err) => catalog_message(err),
        };
        return Ok(Some((chat_id.to_owned(), reply)));
    }
    let user = db
        .upsert_feishu_identity(open_id, chat_id, "")
        .await
        .map_err(catalog_message)?;
    let reply = crate::telegram_bot::reply_for(db, &user, &text, now).await;
    Ok(Some((chat_id.to_owned(), reply.text)))
}

async fn shared_card(
    db: &Db,
    payload: &Value,
    now: i64,
) -> Result<Option<(String, String)>, String> {
    let open_id = payload
        .pointer("/event/operator/open_id")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .pointer("/event/operator/operator_id/open_id")
                .and_then(Value::as_str)
        })
        .unwrap_or("");
    let Some(user) = db
        .user_by_feishu_open_id(open_id)
        .await
        .map_err(|err| err.to_string())?
    else {
        return Ok(None);
    };
    if user.feishu_chat_id.is_empty() {
        return Ok(None);
    }
    let Some(command) = card_command(payload) else {
        return Ok(None);
    };
    let reply = crate::telegram_bot::reply_for(db, &user, &command, now).await;
    Ok(Some((user.feishu_chat_id, reply.text)))
}

fn card_command(payload: &Value) -> Option<String> {
    let value = payload.pointer("/event/action/value")?;
    let action = value.get("action").and_then(Value::as_str)?;
    let kol = value
        .get("kol_id")
        .and_then(Value::as_i64)
        .map(|id| id.to_string())
        .or_else(|| {
            value
                .get("kol_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    match action {
        "sub" => Some(format!("/sub {}", kol?)),
        "unsub" => Some(format!("/unsub {}", kol?)),
        "list" => Some(format!(
            "/list {}",
            value.get("page").and_then(Value::as_i64).unwrap_or(1)
        )),
        _ => None,
    }
}

fn remember_message(id: &str) -> bool {
    if id.is_empty() {
        return true;
    }
    let mut seen = SEEN.lock().expect("feishu seen");
    if seen.iter().any(|item| item == id) {
        return false;
    }
    seen.push(id.to_owned());
    if seen.len() > 200 {
        seen.remove(0);
    }
    true
}

fn catalog_message(err: crate::db::CatalogError) -> String {
    use crate::db::CatalogError;
    match err {
        CatalogError::Missing(message)
        | CatalogError::Bad(message)
        | CatalogError::Limited(message)
        | CatalogError::Conflict(message) => message.to_owned(),
        CatalogError::Invalid(message) => message,
        CatalogError::Db(err) => err.to_string(),
    }
}

struct Endpoint {
    url: String,
    service: i32,
}

fn fetch_endpoint(domain: &str, app_id: &str, secret: &str) -> Result<Endpoint, String> {
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build()
        .post(&format!("{domain}/callback/ws/endpoint"))
        .set("locale", "zh")
        .set("Content-Type", "application/json")
        .send_string(&json!({"AppID": app_id, "AppSecret": secret}).to_string())
        .map_err(|err| err.to_string())?;
    let text = response.into_string().map_err(|err| err.to_string())?;
    let body: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    endpoint_from(&body)
}

#[allow(dead_code)]
pub fn event_frame(payload: &[u8]) -> Vec<u8> {
    encode_frame(&Frame {
        seq_id: 9,
        log_id: 8,
        service: 7,
        method: 1,
        headers: vec![Header {
            key: "type".into(),
            value: "event".into(),
        }],
        payload: payload.to_vec(),
    })
}

fn endpoint_from(body: &Value) -> Result<Endpoint, String> {
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        return Err(body
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("飞书长连接地址获取失败")
            .to_string());
    }
    let url = body
        .pointer("/data/URL")
        .or_else(|| body.pointer("/data/url"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !url.starts_with("wss://") {
        return Err("飞书长连接地址无效".into());
    }
    let service = url
        .split('?')
        .nth(1)
        .unwrap_or("")
        .split('&')
        .find_map(|item| {
            let (key, value) = item.split_once('=')?;
            if key == "service_id" {
                value.parse::<i32>().ok()
            } else {
                None
            }
        })
        .ok_or("飞书长连接缺少 service_id")?;
    Ok(Endpoint {
        url: url.to_string(),
        service,
    })
}

fn send_notice(
    app_id: &str,
    secret: &str,
    chat_id: &str,
    host: &str,
    brand: &str,
) -> Result<(), String> {
    let host = if brand == "lark" {
        "open.larksuite.com"
    } else {
        host
    };
    let token_text = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build()
        .post(&format!(
            "https://{host}/open-apis/auth/v3/tenant_access_token/internal"
        ))
        .set("Content-Type", "application/json")
        .send_string(&json!({"app_id": app_id, "app_secret": secret}).to_string())
        .map_err(|err| err.to_string())?
        .into_string()
        .map_err(|err| err.to_string())?;
    let token_body: Value = serde_json::from_str(&token_text).map_err(|err| err.to_string())?;
    let token = token_body
        .get("tenant_access_token")
        .and_then(Value::as_str)
        .unwrap_or("");
    if token.is_empty() {
        return Err(token_body
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("获取飞书令牌失败")
            .into());
    }
    let content = json!({"text": "VPush 已绑定这个机器人，之后会用它给你发推送。"}).to_string();
    let sent_text = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .redirects(0)
        .build()
        .post(&format!(
            "https://{host}/open-apis/im/v1/messages?receive_id_type=chat_id"
        ))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(
            &json!({"receive_id": chat_id, "msg_type": "text", "content": content}).to_string(),
        )
        .map_err(|err| err.to_string())?
        .into_string()
        .map_err(|err| err.to_string())?;
    let sent: Value = serde_json::from_str(&sent_text).map_err(|err| err.to_string())?;
    if sent.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
        return Err(sent
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("测试消息发送失败")
            .into());
    }
    Ok(())
}

#[derive(Clone)]
struct Header {
    key: String,
    value: String,
}

struct Frame {
    seq_id: u64,
    log_id: u64,
    service: i32,
    method: i32,
    headers: Vec<Header>,
    payload: Vec<u8>,
}

fn ping_frame(service: i32) -> Frame {
    Frame {
        seq_id: 0,
        log_id: 0,
        service,
        method: 0,
        headers: vec![Header {
            key: "type".into(),
            value: "ping".into(),
        }],
        payload: Vec::new(),
    }
}

fn header<'a>(frame: &'a Frame, key: &str) -> &'a str {
    frame
        .headers
        .iter()
        .find(|item| item.key == key)
        .map(|item| item.value.as_str())
        .unwrap_or("")
}

fn decode_frame(data: &[u8]) -> Result<Frame, String> {
    let mut frame = Frame {
        seq_id: 0,
        log_id: 0,
        service: 0,
        method: 0,
        headers: Vec::new(),
        payload: Vec::new(),
    };
    let mut index = 0;
    while index < data.len() {
        let key = read_varint(data, &mut index).ok_or("飞书帧无效")?;
        let field = key >> 3;
        let wire = key & 7;
        match (field, wire) {
            (1, 0) => frame.seq_id = read_varint(data, &mut index).ok_or("飞书帧无效")?,
            (2, 0) => frame.log_id = read_varint(data, &mut index).ok_or("飞书帧无效")?,
            (3, 0) => frame.service = read_varint(data, &mut index).ok_or("飞书帧无效")? as i32,
            (4, 0) => frame.method = read_varint(data, &mut index).ok_or("飞书帧无效")? as i32,
            (5, 2) => frame
                .headers
                .push(decode_header(&read_bytes(data, &mut index)?)?),
            (8, 2) => frame.payload = read_bytes(data, &mut index)?,
            (_, 0) => {
                read_varint(data, &mut index).ok_or("飞书帧无效")?;
            }
            (_, 2) => {
                read_bytes(data, &mut index)?;
            }
            (_, 5) => index += 4,
            (_, 1) => index += 8,
            _ => return Err("飞书帧无效".into()),
        }
    }
    Ok(frame)
}

fn decode_header(data: &[u8]) -> Result<Header, String> {
    let mut key = String::new();
    let mut value = String::new();
    let mut index = 0;
    while index < data.len() {
        let tag = read_varint(data, &mut index).ok_or("飞书帧无效")?;
        match (tag >> 3, tag & 7) {
            (1, 2) => {
                key = String::from_utf8(read_bytes(data, &mut index)?)
                    .map_err(|_| "飞书帧无效".to_string())?
            }
            (2, 2) => {
                value = String::from_utf8(read_bytes(data, &mut index)?)
                    .map_err(|_| "飞书帧无效".to_string())?
            }
            (_, 0) => {
                read_varint(data, &mut index).ok_or("飞书帧无效")?;
            }
            (_, 2) => {
                read_bytes(data, &mut index)?;
            }
            _ => return Err("飞书帧无效".into()),
        }
    }
    Ok(Header { key, value })
}

fn encode_frame(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint_field(&mut out, 1, frame.seq_id);
    write_varint_field(&mut out, 2, frame.log_id);
    write_varint_field(&mut out, 3, frame.service as u64);
    write_varint_field(&mut out, 4, frame.method as u64);
    for header in &frame.headers {
        let bytes = encode_header(header);
        write_key(&mut out, 5, 2);
        write_varint(&mut out, bytes.len() as u64);
        out.extend(bytes);
    }
    if !frame.payload.is_empty() {
        write_key(&mut out, 8, 2);
        write_varint(&mut out, frame.payload.len() as u64);
        out.extend(&frame.payload);
    }
    out
}

fn encode_header(header: &Header) -> Vec<u8> {
    let mut out = Vec::new();
    write_key(&mut out, 1, 2);
    write_varint(&mut out, header.key.len() as u64);
    out.extend(header.key.as_bytes());
    write_key(&mut out, 2, 2);
    write_varint(&mut out, header.value.len() as u64);
    out.extend(header.value.as_bytes());
    out
}

fn write_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    write_key(out, field, 0);
    write_varint(out, value);
}

fn write_key(out: &mut Vec<u8>, field: u64, wire: u64) {
    write_varint(out, (field << 3) | wire);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint(data: &[u8], index: &mut usize) -> Option<u64> {
    let mut out = 0u64;
    let mut shift = 0;
    while *index < data.len() && shift < 64 {
        let byte = data[*index];
        *index += 1;
        out |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(out);
        }
        shift += 7;
    }
    None
}

fn read_bytes(data: &[u8], index: &mut usize) -> Result<Vec<u8>, String> {
    let len = read_varint(data, index).ok_or("飞书帧无效")? as usize;
    if *index + len > data.len() {
        return Err("飞书帧无效".into());
    }
    let bytes = data[*index..*index + len].to_vec();
    *index += len;
    Ok(bytes)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|item| item.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_frame_roundtrips_and_endpoint_needs_service_id() {
        let frame = decode_frame(&encode_frame(&ping_frame(7))).unwrap();
        assert_eq!(frame.method, 0);
        assert_eq!(frame.service, 7);
        assert_eq!(header(&frame, "type"), "ping");
        let body = serde_json::json!({"code": 0, "data": {"URL": "wss://open.feishu.cn/ws?service_id=12"}});
        let endpoint = endpoint_from(&body).unwrap();
        assert_eq!(endpoint.service, 12);
        assert!(
            endpoint_from(&serde_json::json!({"code": 0, "data": {"URL": "https://evil"}}))
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_private_message_replies_and_bind_moves_to_web_user() {
        let db = Db::open(std::path::Path::new(":memory:")).await.unwrap();
        let frame = text_event("om_help", "ou_help", "oc_help", "p2p", "/help");
        let (_, outgoing) = handle_shared_frame(&db, &frame, 1_000).await.unwrap();
        assert!(outgoing.unwrap().1.contains("帮助"));
        let user = db.user_by_feishu_open_id("ou_help").await.unwrap().unwrap();
        assert_eq!(user.feishu_chat_id, "oc_help");
        let (_, duplicate) = handle_shared_frame(&db, &frame, 1_000).await.unwrap();
        assert!(duplicate.is_none());

        let group = text_event("om_group", "ou_group", "oc_group", "group", "/help");
        let (_, group_reply) = handle_shared_frame(&db, &group, 1_000).await.unwrap();
        assert!(group_reply.is_none());
        assert!(db
            .user_by_feishu_open_id("ou_group")
            .await
            .unwrap()
            .is_none());

        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('web', 'hash')")
            .execute(db.pool())
            .await
            .unwrap();
        let web_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'web'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let (code, _) = db.issue_bind_code(web_id, 1_000).await.unwrap();
        let hello = text_event("om_hello", "ou_bind", "oc_bind", "p2p", "你好");
        handle_shared_frame(&db, &hello, 1_000).await.unwrap();
        let bind = text_event(
            "om_bind",
            "ou_bind",
            "oc_bind",
            "p2p",
            &format!("/bind {code}"),
        );
        let (_, bound) = handle_shared_frame(&db, &bind, 1_000).await.unwrap();
        assert!(bound.unwrap().1.contains("绑定成功"));
        let web = db.user_by_id(web_id).await.unwrap().unwrap();
        assert_eq!(web.feishu_open_id, "ou_bind");
        assert_eq!(web.feishu_chat_id, "oc_bind");
    }

    fn text_event(id: &str, open_id: &str, chat_id: &str, chat_type: &str, text: &str) -> Vec<u8> {
        event_frame(
            &serde_json::to_vec(&json!({
                "header": {"event_type": "im.message.receive_v1"},
                "event": {
                    "sender": {"sender_id": {"open_id": open_id}},
                    "message": {
                        "message_id": id,
                        "chat_id": chat_id,
                        "chat_type": chat_type,
                        "message_type": "text",
                        "content": serde_json::to_string(&json!({"text": text})).unwrap()
                    }
                }
            }))
            .unwrap(),
        )
    }
}
