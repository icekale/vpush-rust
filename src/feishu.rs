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

pub struct CardContext<'a> {
    pub category: &'a str,
    pub tags: &'a Value,
    pub detail: &'a Value,
    pub favorite: bool,
    pub keyword: bool,
}

fn empty_value() -> &'static Value {
    static EMPTY: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| Value::Null);
    &EMPTY
}

pub fn card(note: &Note<'_>) -> Value {
    card_with(
        note,
        &CardContext {
            category: "",
            tags: empty_value(),
            detail: empty_value(),
            favorite: false,
            keyword: false,
        },
    )
}

pub fn card_with(note: &Note<'_>, ctx: &CardContext<'_>) -> Value {
    if note.platform == "combination" && ctx.detail.is_object() {
        return combination_card(note, ctx.detail);
    }
    let platform = platform_label(note.platform);
    let mut title = format!("📌 {} · {platform}", note.kol_name);
    if note.post_type == "reply" {
        title.push_str(" · 回复");
    }
    let content = truncate(body_text(note.content, note.title), 2000);
    let mut meta = Vec::new();
    let badges = badges(ctx.favorite, ctx.keyword);
    if !badges.is_empty() {
        meta.push(badges);
    }
    if !ctx.category.is_empty() {
        meta.push(format!("🗂 {}", ctx.category));
    }
    if let Some(tags) = ctx.tags.as_array() {
        let tags = tags
            .iter()
            .filter_map(Value::as_str)
            .filter(|tag| !tag.is_empty())
            .take(12)
            .collect::<Vec<_>>()
            .join(" ");
        if !tags.is_empty() {
            meta.push(format!("🏷 {tags}"));
        }
    }
    meta.push(format!("🕐 {}", note.published_at));
    let mut elements = vec![
        json!({"tag": "div", "text": {"tag": "lark_md", "content": content}}),
        json!({"tag": "hr"}),
        json!({"tag": "div", "text": {"tag": "lark_md", "content": meta.join("\n")}}),
    ];
    if show_original(note.platform, note.url) {
        elements.push(link_button("查看原文", note.url, "primary"));
    }
    interactive(
        &title,
        &summary(&[&title, &preview(note.content, note.title)]),
        "blue",
        elements,
    )
}

fn combination_card(note: &Note<'_>, detail: &Value) -> Value {
    let title = format!("📌 {} · 雪球组合 · 调仓", note.kol_name);
    let mut elements = Vec::new();
    let mut summary_extra = String::new();
    if let Some(stats) = detail["stats"].as_array() {
        let line = stats
            .iter()
            .filter_map(|pair| {
                let pair = pair.as_array()?;
                Some(format!(
                    "**{}** {}",
                    pair.first()?.as_str().unwrap_or(""),
                    pair.get(1)?.as_str().unwrap_or("")
                ))
            })
            .collect::<Vec<_>>()
            .join("　");
        if !line.is_empty() {
            summary_extra = stats
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some(format!(
                        "{} {}",
                        pair.first()?.as_str().unwrap_or(""),
                        pair.get(1)?.as_str().unwrap_or("")
                    ))
                })
                .collect::<Vec<_>>()
                .join("　");
            elements.push(json!({"tag": "div", "text": {"tag": "lark_md", "content": line}}));
            elements.push(json!({"tag": "hr"}));
        }
    }
    if let Some(actions) = detail["actions"].as_array() {
        for action in actions.iter().take(12).filter(|action| action.is_object()) {
            let kind = action["type"].as_str().unwrap_or("调整");
            let icon = match kind {
                "清仓" => "🗑",
                "新建" => "🆕",
                "增持" => "➕",
                "减持" => "➖",
                _ => "•",
            };
            let stock = action["stock"].as_str().unwrap_or("");
            let symbol = action["symbol"].as_str().unwrap_or("");
            let name = if symbol.is_empty() {
                stock.to_string()
            } else {
                format!("{stock}（{symbol}）")
            };
            if summary_extra.is_empty() {
                summary_extra = if stock.is_empty() {
                    kind.to_string()
                } else {
                    format!("{kind} {stock}")
                };
            }
            let mut text = format!(
                "{icon} **{kind}** {name}\n{} → {}",
                action["prev"].as_str().unwrap_or("0.0%"),
                action["target"].as_str().unwrap_or("0.0%")
            );
            let price = scalar(&action["price"]);
            if !price.is_empty() {
                text.push_str(&format!("\n成交价 {price}"));
            }
            elements.push(json!({"tag": "div", "text": {"tag": "lark_md", "content": text}}));
        }
    }
    if let Some(cash) = detail["cash"].as_str().filter(|cash| !cash.is_empty()) {
        elements.push(json!({"tag": "div", "text": {"tag": "lark_md", "content": format!("💵 现金 **{cash}**")}}));
    }
    if let Some(holdings) = detail["holdings"].as_array() {
        let lines = holdings
            .iter()
            .filter(|holding| {
                holding["name"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty())
                    && !holding["weight"].is_null()
            })
            .take(15)
            .map(|holding| {
                format!(
                    "{}（{}） {}%",
                    holding["name"].as_str().unwrap_or(""),
                    holding["symbol"].as_str().unwrap_or(""),
                    scalar(&holding["weight"])
                )
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            elements.push(json!({
                "tag": "div",
                "text": {"tag": "lark_md", "content": format!("**现有持仓**\n{}", lines.join("\n"))}
            }));
        }
    }
    elements.push(json!({"tag": "hr"}));
    elements.push(json!({"tag": "div", "text": {"tag": "lark_md", "content": format!("🕐 {}", note.published_at)}}));
    if show_original(note.platform, note.url) {
        elements.push(link_button("查看原文", note.url, "primary"));
    }
    interactive(
        &title,
        &summary(&[&title, &summary_extra]),
        "blue",
        elements,
    )
}

fn interactive(title: &str, summary: &str, template: &str, elements: Vec<Value>) -> Value {
    json!({
        "msg_type": "interactive",
        "card": {
            "schema": "2.0",
            "config": {"width_mode": "fill", "summary": {"content": summary}},
            "header": {"title": {"tag": "plain_text", "content": title}, "template": template},
            "body": {"elements": elements}
        }
    })
}

fn link_button(text: &str, url: &str, kind: &str) -> Value {
    json!({
        "tag": "button",
        "text": {"tag": "plain_text", "content": text},
        "type": kind,
        "behaviors": [{"type": "open_url", "default_url": url}]
    })
}

fn platform_label(platform: &str) -> &str {
    match platform {
        "xueqiu" => "雪球",
        "combination" => "雪球组合",
        "weibo" => "微博",
        "twitter" => "X",
        "zsxq" => "知识星球",
        "truth" => "Truth Social",
        other => other,
    }
}

fn show_original(platform: &str, url: &str) -> bool {
    platform != "zsxq" && (url.starts_with("https://") || url.starts_with("http://"))
}

fn body_text<'a>(content: &'a str, title: &'a str) -> &'a str {
    if !content.is_empty() {
        content
    } else if !title.is_empty() {
        title
    } else {
        "（无正文）"
    }
}

fn preview(content: &str, title: &str) -> String {
    let flat = body_text(content, title)
        .chars()
        .map(|ch| if ch == '\n' { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate(&flat, 60)
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn badges(favorite: bool, keyword: bool) -> String {
    match (favorite, keyword) {
        (true, true) => "🔔 特别关注 · 🔑 命中关键词".into(),
        (true, false) => "🔔 特别关注".into(),
        (false, true) => "🔑 命中关键词".into(),
        (false, false) => String::new(),
    }
}

fn summary(parts: &[&str]) -> String {
    let text = parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("：");
    let text: String = text
        .chars()
        .filter(|ch| *ch != '\u{1FAE9}' && *ch != '\u{1FAEA}')
        .collect();
    truncate(&text, 100)
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
        Err(_) => return Err("飞书请求失败".into()),
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

pub async fn deliver_user(
    db: &Db,
    user_id: i64,
    note: &Note<'_>,
    ctx: &CardContext<'_>,
) -> Result<(), String> {
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
                    ctx,
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
    send_shared(db, user_id, Message::Card(note, ctx)).await
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
    Card(&'a Note<'a>, &'a CardContext<'a>),
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
        Message::Card(note, ctx) => {
            send_card(&app_id, &secret, "feishu", chat_id, open_id, note, ctx).await
        }
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
    ctx: &CardContext<'_>,
) -> Result<(), String> {
    let content =
        serde_json::to_string(&card_with(note, ctx)["card"]).map_err(|err| err.to_string())?;
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

    #[test]
    fn card_includes_badges_and_hides_zsxq_original() {
        let note = Note {
            kol_name: "甲",
            platform: "zsxq",
            external_id: "1",
            post_type: "post",
            title: "标题",
            content: "正文",
            url: "https://wx.zsxq.com/1",
            published_at: "2026-09-28 12:00",
        };
        let tags = json!(["半导体"]);
        let card = card_with(
            &note,
            &CardContext {
                category: "行业",
                tags: &tags,
                detail: empty_value(),
                favorite: true,
                keyword: true,
            },
        );
        let meta = card["card"]["body"]["elements"][2]["text"]["content"]
            .as_str()
            .unwrap();
        assert!(meta.contains("特别关注"));
        assert!(meta.contains("命中关键词"));
        assert!(meta.contains("行业"));
        assert!(meta.contains("半导体"));
        assert!(card["card"]["body"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["tag"] != "button"));
        assert!(card["card"]["config"]["summary"]["content"]
            .as_str()
            .unwrap()
            .contains("知识星球"));
    }

    #[test]
    fn combination_card_lists_rebalance_without_raw_json() {
        let note = Note {
            kol_name: "组合甲",
            platform: "combination",
            external_id: "1",
            post_type: "rebalance",
            title: "调仓",
            content: "{\"raw\":true}",
            url: "https://xueqiu.com/P/ZH1",
            published_at: "2026-09-28 12:00",
        };
        let detail = json!({
            "stats": [["总收益", "12.0%"]],
            "actions": [{"type": "增持", "stock": "贵州茅台", "symbol": "SH600519", "prev": "1.0%", "target": "2.0%", "price": "1800"}],
            "cash": "5.0%",
            "holdings": [{"name": "贵州茅台", "symbol": "SH600519", "weight": 2}]
        });
        let card = card_with(
            &note,
            &CardContext {
                category: "",
                tags: empty_value(),
                detail: &detail,
                favorite: false,
                keyword: false,
            },
        );
        let body = card["card"]["body"]["elements"].to_string();
        assert!(body.contains("增持"));
        assert!(body.contains("贵州茅台"));
        assert!(body.contains("现金"));
        assert!(body.contains("现有持仓"));
        assert!(!body.contains("raw"));
    }

    #[test]
    fn webhook_transport_error_omits_secret_url() {
        let secret = "unit-test-feishu-hook";
        let err = post(
            &format!("http://127.0.0.1:1/open-apis/bot/v2/hook/{secret}"),
            "{}",
        )
        .unwrap_err();
        assert!(!err.contains(secret), "{err}");
        assert!(!err.contains("http"), "{err}");
    }
}
