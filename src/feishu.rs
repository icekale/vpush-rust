//! 飞书群机器人。新帖写成一张交互卡片，发到 FEISHU_WEBHOOK_URL 或 settings.feishu_webhook。

use serde_json::{json, Value};

use crate::db::Db;

pub struct Note<'a> {
    pub kol_name: &'a str,
    pub platform: &'a str,
    pub external_id: &'a str,
    pub post_type: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub url: &'a str,
    pub published_at: &'a str,
}

pub async fn notify(db: &Db, note: Note<'_>) -> Result<(), String> {
    let Some(url) = webhook(db).await? else {
        return Ok(());
    };
    if !allowed(&url) {
        return Err("飞书 webhook 必须是 https://open.feishu.cn 或 open.larksuite.com".into());
    }
    let body = serde_json::to_string(&card(&note)).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || post(&url, &body))
        .await
        .map_err(|e| e.to_string())?
}

pub fn card(note: &Note<'_>) -> Value {
    let platform = if note.platform == "xueqiu" {
        "雪球"
    } else {
        note.platform
    };
    let mut title = format!("📌 {} · {platform}", note.kol_name);
    if note.post_type == "reply" {
        title.push_str(" · 回复");
    }
    let content = {
        let text = if note.content.is_empty() {
            note.title
        } else {
            note.content
        };
        let text = if text.is_empty() {
            "（无正文）"
        } else {
            text
        };
        truncate(text, 2000)
    };
    let mut elements = vec![
        json!({"tag": "div", "text": {"tag": "lark_md", "content": content}}),
        json!({"tag": "hr"}),
        json!({"tag": "div", "text": {"tag": "lark_md", "content": format!("🕐 {}", note.published_at)}}),
    ];
    if note.url.starts_with("https://") || note.url.starts_with("http://") {
        elements.push(json!({
            "tag": "button",
            "text": {"tag": "plain_text", "content": "查看原文"},
            "type": "primary",
            "behaviors": [{"type": "open_url", "default_url": note.url}]
        }));
    }
    json!({
        "msg_type": "interactive",
        "card": {
            "schema": "2.0",
            "header": {"title": {"tag": "plain_text", "content": title}, "template": "blue"},
            "body": {"elements": elements}
        }
    })
}

async fn webhook(db: &Db) -> Result<Option<String>, String> {
    if let Ok(value) = std::env::var("FEISHU_WEBHOOK_URL") {
        let value = value.trim();
        if !value.is_empty() {
            return Ok(Some(value.to_string()));
        }
    }
    let saved = db
        .setting("feishu_webhook")
        .await
        .map_err(|e| e.to_string())?;
    Ok(saved.filter(|value| !value.trim().is_empty()))
}

fn allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', ':']).next().unwrap_or("");
    host == "open.feishu.cn" || host == "open.larksuite.com"
}

fn post(url: &str, body: &str) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build();
    let (status, text) = match agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_string(body)
    {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            (status, text)
        }
        Err(ureq::Error::Status(status, resp)) => (status, resp.into_string().unwrap_or_default()),
        Err(err) => return Err(err.to_string()),
    };
    if status != 200 {
        return Err(format!("飞书 HTTP {status}: {text}"));
    }
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let Some(code) = payload["code"].as_i64() {
        if code != 0 {
            return Err(format!("飞书返回错误: {text}"));
        }
    }
    Ok(())
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

pub fn definitive(err: &str) -> bool {
    const CODES: [&str; 4] = ["code=230101", "code=91002", "code=20001", "code=99991670"];
    CODES.iter().any(|code| err.contains(code))
        || [
            "权限",
            "未开通",
            "应用不存在",
            "用户不存在",
            "app_secret",
            "凭据",
        ]
        .iter()
        .any(|word| err.contains(word))
}

pub async fn deliver_user(db: &Db, user_id: i64, note: &Note<'_>) -> Result<(), String> {
    if let Some(route) = db
        .active_feishu_route(user_id)
        .await
        .map_err(|err| err.to_string())?
    {
        let key = crate::feishu_personal::credential_key().unwrap_or_default();
        match crate::feishu_personal::open_app_secret(&key, &route.app_secret_ciphertext) {
            Ok(secret) => {
                match send_card(
                    &route.app_id,
                    &secret,
                    &route.tenant_brand,
                    &route.chat_id,
                    "",
                    note,
                )
                .await
                {
                    Ok(()) => return Ok(()),
                    Err(err) if definitive(&err) => {
                        db.degrade_feishu_bot(user_id, &err)
                            .await
                            .map_err(|err| err.to_string())?;
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(_) => tracing::warn!(user_id, "飞书个人凭据无法解密，改用共享应用"),
        }
    }
    send_shared(db, user_id, Message::Card(note)).await
}

pub async fn deliver_text(db: &Db, user_id: i64, text: &str) -> Result<(), String> {
    if let Some(route) = db
        .active_feishu_route(user_id)
        .await
        .map_err(|err| err.to_string())?
    {
        let key = crate::feishu_personal::credential_key().unwrap_or_default();
        if let Ok(secret) =
            crate::feishu_personal::open_app_secret(&key, &route.app_secret_ciphertext)
        {
            match send_text_im(
                &route.app_id,
                &secret,
                &route.tenant_brand,
                &route.chat_id,
                "",
                text,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(err) if definitive(&err) => {
                    db.degrade_feishu_bot(user_id, &err)
                        .await
                        .map_err(|err| err.to_string())?;
                }
                Err(err) => return Err(err),
            }
        }
    }
    send_shared(db, user_id, Message::Text(text)).await
}

enum Message<'a> {
    Card(&'a Note<'a>),
    Text(&'a str),
}

async fn send_shared(db: &Db, user_id: i64, message: Message<'_>) -> Result<(), String> {
    let Some((app_id, secret)) = shared_app() else {
        return Err("飞书未绑定".into());
    };
    let Some(user) = db
        .user_by_id(user_id)
        .await
        .map_err(|err| err.to_string())?
    else {
        return Err("用户不存在".into());
    };
    let (chat_id, open_id) = if !user.feishu_chat_id.is_empty() {
        (user.feishu_chat_id.as_str(), "")
    } else if !user.feishu_open_id.is_empty() {
        ("", user.feishu_open_id.as_str())
    } else {
        return Err("飞书未绑定".into());
    };
    match message {
        Message::Card(note) => send_card(&app_id, &secret, "feishu", chat_id, open_id, note).await,
        Message::Text(text) => {
            send_text_im(&app_id, &secret, "feishu", chat_id, open_id, text).await
        }
    }
}

pub async fn send_shared_text(chat_id: &str, text: &str) -> Result<(), String> {
    let Some((app_id, secret)) = shared_app() else {
        return Err("未配置飞书应用".into());
    };
    send_text_im(&app_id, &secret, "feishu", chat_id, "", text).await
}

fn shared_app() -> Option<(String, String)> {
    let app_id = std::env::var("FEISHU_APP_ID").unwrap_or_default();
    let secret = std::env::var("FEISHU_APP_SECRET").unwrap_or_default();
    if app_id.trim().is_empty() || secret.trim().is_empty() {
        None
    } else {
        Some((app_id, secret))
    }
}

fn host(brand: &str) -> &'static str {
    if brand == "lark" {
        "https://open.larksuite.com"
    } else {
        "https://open.feishu.cn"
    }
}

async fn send_card(
    app_id: &str,
    secret: &str,
    brand: &str,
    chat_id: &str,
    open_id: &str,
    note: &Note<'_>,
) -> Result<(), String> {
    let content = serde_json::to_string(&card(note)["card"]).map_err(|err| err.to_string())?;
    send_im(
        app_id,
        secret,
        brand,
        chat_id,
        open_id,
        "interactive",
        &content,
    )
    .await
}

async fn send_text_im(
    app_id: &str,
    secret: &str,
    brand: &str,
    chat_id: &str,
    open_id: &str,
    text: &str,
) -> Result<(), String> {
    let content = serde_json::to_string(&json!({"text": truncate(text, 2000)}))
        .map_err(|err| err.to_string())?;
    send_im(app_id, secret, brand, chat_id, open_id, "text", &content).await
}

async fn send_im(
    app_id: &str,
    secret: &str,
    brand: &str,
    chat_id: &str,
    open_id: &str,
    msg_type: &str,
    content: &str,
) -> Result<(), String> {
    let (id_type, receive_id) = if !chat_id.is_empty() {
        ("chat_id", chat_id)
    } else if !open_id.is_empty() {
        ("open_id", open_id)
    } else {
        return Err("飞书未绑定".into());
    };
    let base = host(brand).to_string();
    let app_id = app_id.to_string();
    let secret = secret.to_string();
    let body = json!({
        "receive_id": receive_id,
        "msg_type": msg_type,
        "content": content,
    })
    .to_string();
    let id_type = id_type.to_string();
    tokio::task::spawn_blocking(move || {
        let token = tenant_token(&base, &app_id, &secret)?;
        let url = format!("{base}/open-apis/im/v1/messages?receive_id_type={id_type}");
        let (status, text) = http_post(&url, &body, Some(&token))?;
        if status != 200 {
            return Err(api_error(status, &text));
        }
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if payload["code"].as_i64().unwrap_or(0) != 0 {
            return Err(api_error(status, &text));
        }
        Ok(())
    })
    .await
    .map_err(|err| err.to_string())?
}

fn tenant_token(host: &str, app_id: &str, secret: &str) -> Result<String, String> {
    let key = format!("{host}\n{app_id}");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|item| item.as_secs() as i64)
        .unwrap_or(0);
    let cache = token_cache();
    if let Some((token, expiry)) = cache.lock().expect("feishu tokens").get(&key) {
        if *expiry > now + 60 {
            return Ok(token.clone());
        }
    }
    let (status, text) = http_post(
        &format!("{host}/open-apis/auth/v3/tenant_access_token/internal"),
        &json!({"app_id": app_id, "app_secret": secret}).to_string(),
        None,
    )?;
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status != 200 || payload["code"].as_i64().unwrap_or(-1) != 0 {
        return Err(api_error(status, &text));
    }
    let token = payload["tenant_access_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        return Err("飞书获取 token 失败(code=20001): app_secret".into());
    }
    let expiry = now + payload["expire"].as_i64().unwrap_or(7200);
    cache
        .lock()
        .expect("feishu tokens")
        .insert(key, (token.clone(), expiry));
    Ok(token)
}

fn token_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, i64)>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (String, i64)>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn http_post(url: &str, body: &str, bearer: Option<&str>) -> Result<(u16, String), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build();
    let mut request = agent.post(url).set("Content-Type", "application/json");
    if let Some(token) = bearer {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    match request.send_string(body) {
        Ok(response) => Ok((
            response.status(),
            response.into_string().unwrap_or_default(),
        )),
        Err(ureq::Error::Status(status, response)) => {
            Ok((status, response.into_string().unwrap_or_default()))
        }
        Err(err) => Err(err.to_string()),
    }
}

fn api_error(status: u16, text: &str) -> String {
    let payload: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if let Some(code) = payload["code"].as_i64() {
        return format!(
            "飞书发送失败(code={code}): {}",
            payload["msg"].as_str().unwrap_or("")
        );
    }
    format!("飞书 HTTP {status}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_definite_feishu_errors_can_fall_back() {
        assert!(definitive("飞书发送失败(code=230101): 用户不存在"));
        assert!(definitive("飞书发送失败(code=91002): 无权限"));
        assert!(!definitive("飞书 HTTP 500"));
        assert!(!definitive("error sending request"));
    }

    #[test]
    fn card_has_title_body_and_link() {
        let note = Note {
            kol_name: "甲",
            platform: "xueqiu",
            external_id: "fixture",
            post_type: "reply",
            title: "",
            content: "回复一下",
            url: "https://xueqiu.com/1/2",
            published_at: "2026-09-26 12:00",
        };
        let card = card(&note);
        assert_eq!(card["msg_type"], "interactive");
        assert_eq!(
            card["card"]["header"]["title"]["content"],
            "📌 甲 · 雪球 · 回复"
        );
        assert_eq!(
            card["card"]["body"]["elements"][0]["text"]["content"],
            "回复一下"
        );
        assert_eq!(
            card["card"]["body"]["elements"][3]["behaviors"][0]["default_url"],
            note.url
        );
        assert!(allowed("https://open.feishu.cn/open-apis/bot/v2/hook/abc"));
        assert!(!allowed("http://open.feishu.cn/hook"));
        assert!(!allowed("https://example.com/hook"));
    }
}
