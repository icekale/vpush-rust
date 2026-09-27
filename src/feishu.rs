//! 飞书群机器人。新帖写成一张交互卡片，发到 FEISHU_WEBHOOK_URL 或 settings.feishu_webhook。

use serde_json::{json, Value};

use crate::db::Db;

pub struct Note<'a> {
    pub kol_name: &'a str,
    pub platform: &'a str,
    pub post_type: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub url: &'a str,
    pub published_at: &'a str,
}

pub async fn configured(db: &Db) -> Result<bool, String> {
    Ok(webhook(db).await?.is_some_and(|url| allowed(&url)))
}

pub async fn send_text(db: &Db, text: &str) -> Result<(), String> {
    let Some(url) = webhook(db).await? else {
        return Err("未配置飞书 webhook".into());
    };
    if !allowed(&url) {
        return Err("飞书 webhook 必须是 https://open.feishu.cn 或 open.larksuite.com".into());
    }
    let body = json!({"msg_type": "text", "content": {"text": truncate(text, 2000)}}).to_string();
    tokio::task::spawn_blocking(move || post(&url, &body))
        .await
        .map_err(|e| e.to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_has_title_body_and_link() {
        let note = Note {
            kol_name: "甲",
            platform: "xueqiu",
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
