use crate::db::{CatalogError, Db, User};
use serde_json::Value;

const PAGE_SIZE: usize = 20;
const SEARCH_LIMIT: usize = 10;
const MAX_INPUT_CHARS: usize = 4096;
const MAX_OUTPUT_CHARS: usize = 4096;
const BIND_TRY_LIMIT: i64 = 5;
const BIND_TRY_WINDOW_SECS: i64 = 600;
const BIND_CODE_ALPHABET: &str = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramChatType {
    Private,
    Group,
    Supergroup,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramMessage {
    pub chat_id: String,
    pub chat_type: TelegramChatType,
    pub display_name: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramButton {
    pub text: String,
    pub callback_data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramResponse {
    pub text: String,
    pub keyboard: Option<Vec<Vec<TelegramButton>>>,
}

#[derive(Debug)]
enum Command {
    Start(Option<String>),
    Help,
    List(usize),
    Search(String),
    Subscribe(String, String),
    Unsubscribe(String),
    MySubscriptions,
    Bind(String),
}

pub async fn handle_message(
    db: &Db,
    message: TelegramMessage,
    now: i64,
) -> Result<Option<TelegramResponse>, String> {
    if message.chat_type != TelegramChatType::Private {
        return Ok(None);
    }
    let chat_id = message.chat_id.trim();
    let Some(text) = message.text.as_deref() else {
        return Ok(None);
    };
    if chat_id.is_empty() || text.trim().is_empty() || text.chars().count() > MAX_INPUT_CHARS {
        return Ok(None);
    }

    let command = parse_command(text);
    let user = db
        .get_or_create_telegram_user(chat_id, &message.display_name, true)
        .await
        .map_err(catalog_error)?
        .ok_or_else(|| "Telegram 私聊身份无效".to_string())?;
    let result = match command {
        Some(Command::Start(Some(code)) | Command::Bind(code)) => {
            bind(db, chat_id, &code, now).await
        }
        Some(Command::Start(None)) => Ok(response("欢迎使用 VPush。\n\n".to_owned() + help_text())),
        Some(Command::Help) => Ok(response(help_text())),
        Some(Command::List(page)) => list(db, &user, page).await,
        Some(Command::Search(keyword)) => search(db, &user, &keyword).await,
        Some(Command::Subscribe(reference, kind)) => subscribe(db, &user, &reference, &kind).await,
        Some(Command::Unsubscribe(reference)) => unsubscribe(db, &user, &reference).await,
        Some(Command::MySubscriptions) => my_subscriptions(db, &user).await,
        None => Ok(response(help_text())),
    };
    let response = match result {
        Ok(response) => response,
        Err(message) => response(message),
    };
    Ok(Some(response))
}

fn parse_command(text: &str) -> Option<Command> {
    let text = text.trim();
    if !text.starts_with('/') {
        return normalize_bind_code(text).map(Command::Bind);
    }
    let mut words = text.split_whitespace();
    let raw_name = words.next()?.trim_start_matches('/');
    let name = raw_name.split('@').next()?.to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    match name.as_str() {
        "start" => {
            if args.len() > 1 {
                return None;
            }
            let deep_link = args.first().and_then(|arg| {
                let (prefix, code) = arg.split_once('_')?;
                prefix.eq_ignore_ascii_case("bind").then_some(code)
            });
            deep_link
                .map(|code| Command::Bind(code.to_owned()))
                .or(Some(Command::Start(None)))
        }
        "help" => (args.is_empty()).then_some(Command::Help),
        "list" => {
            if args.len() > 1 {
                return None;
            }
            let page = args
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|page| *page > 0)
                .unwrap_or(1);
            Some(Command::List(page))
        }
        "search" => {
            if args.len() != 1 {
                return None;
            }
            let keyword = args[0].trim();
            if keyword.is_empty() || keyword.chars().count() > 100 {
                return None;
            }
            Some(Command::Search(keyword.to_owned()))
        }
        "sub" => {
            if !(1..=2).contains(&args.len()) {
                return None;
            }
            let kind = args.get(1).copied().unwrap_or("post").to_ascii_lowercase();
            if !matches!(kind.as_str(), "post" | "reply" | "both") {
                return None;
            }
            Some(Command::Subscribe(args[0].to_owned(), kind))
        }
        "unsub" => (args.len() == 1).then(|| Command::Unsubscribe(args[0].to_owned())),
        "mysubs" => args.is_empty().then_some(Command::MySubscriptions),
        "bind" => (args.len() == 1).then(|| Command::Bind(args[0].to_owned())),
        _ => None,
    }
}

async fn bind(
    db: &Db,
    chat_id: &str,
    raw_code: &str,
    now: i64,
) -> Result<TelegramResponse, String> {
    if !take_bind_attempt(db, chat_id, now).await? {
        return Ok(response("绑定尝试过于频繁，请 10 分钟后重试"));
    }
    let Some(code) = normalize_bind_code(raw_code) else {
        return Ok(response("绑定码无效，请检查后重试"));
    };
    match db
        .consume_bind_code(&code, "telegram_chat_id", chat_id, now)
        .await
        .map_err(catalog_error)?
    {
        Some(_) => Ok(response(
            "绑定成功。当前 Telegram 用户已关联账号。\n发送 /mysubs 查看订阅。",
        )),
        None => Ok(response("绑定码无效或已过期，请检查后重试")),
    }
}

async fn take_bind_attempt(db: &Db, chat_id: &str, now: i64) -> Result<bool, String> {
    let window = now.div_euclid(BIND_TRY_WINDOW_SECS) * BIND_TRY_WINDOW_SECS;
    let key = format!("bind:telegram:{chat_id}");
    let mut tx = db.pool().begin().await.map_err(|err| err.to_string())?;
    let quota: Option<(i64, i64)> =
        sqlx::query_as("SELECT period_start, count FROM bind_quota WHERE key = ?")
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| err.to_string())?;
    let count = match quota {
        Some((start, count)) if start == window => count,
        _ => 0,
    };
    if count >= BIND_TRY_LIMIT {
        tx.commit().await.map_err(|err| err.to_string())?;
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO bind_quota (key, period_start, count) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET period_start = excluded.period_start, count = excluded.count",
    )
    .bind(&key)
    .bind(window)
    .bind(count + 1)
    .execute(&mut *tx)
    .await
    .map_err(|err| err.to_string())?;
    tx.commit().await.map_err(|err| err.to_string())?;
    Ok(true)
}

async fn list(db: &Db, user: &User, page: usize) -> Result<TelegramResponse, String> {
    let items = db
        .catalog(user.id, user.is_admin, "", 0)
        .await
        .map_err(|err| err.to_string())?;
    let start = page.saturating_sub(1).saturating_mul(PAGE_SIZE);
    if start >= items.len() {
        return Ok(response(format!("第 {page} 页没有可见订阅源。")));
    }
    let end = (start + PAGE_SIZE).min(items.len());
    Ok(response(format_catalog(
        &items[start..end],
        Some((page, items.len())),
    )))
}

async fn search(db: &Db, user: &User, keyword: &str) -> Result<TelegramResponse, String> {
    let needle = keyword.to_ascii_lowercase();
    let items = db
        .catalog(user.id, user.is_admin, "", 0)
        .await
        .map_err(|err| err.to_string())?;
    let matches: Vec<Value> = items
        .into_iter()
        .filter(|item| {
            ["name", "external_id", "platform"]
                .iter()
                .filter_map(|key| item.get(key).and_then(Value::as_str))
                .any(|value| value.to_ascii_lowercase().contains(&needle))
        })
        .take(SEARCH_LIMIT)
        .collect();
    if matches.is_empty() {
        return Ok(response(format!("没有找到与“{keyword}”匹配的可见订阅源。")));
    }
    Ok(response(format_catalog(&matches, None)))
}

async fn subscribe(
    db: &Db,
    user: &User,
    reference: &str,
    kind: &str,
) -> Result<TelegramResponse, String> {
    let kol = resolve_reference(db, user, reference).await?;
    let id = kol
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| "订阅源标识无效".to_string())?;
    db.subscribe(user.id, user.is_admin, id, kind)
        .await
        .map_err(catalog_error)?;
    Ok(response(format!(
        "已订阅 {}（{}）。",
        display_name(&kol),
        kind
    )))
}

async fn unsubscribe(db: &Db, user: &User, reference: &str) -> Result<TelegramResponse, String> {
    let kol = resolve_reference(db, user, reference).await?;
    let id = kol
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| "订阅源标识无效".to_string())?;
    db.unsubscribe(user.id, id)
        .await
        .map_err(|err| err.to_string())?;
    Ok(response(format!("已取消订阅 {}。", display_name(&kol))))
}

async fn my_subscriptions(db: &Db, user: &User) -> Result<TelegramResponse, String> {
    let items = db
        .my_subscriptions(user.id)
        .await
        .map_err(|err| err.to_string())?;
    if items.is_empty() {
        return Ok(response("还没有订阅。发送 /list 查看可用订阅源。"));
    }
    Ok(response(format_catalog(&items, None)))
}

async fn resolve_reference(db: &Db, user: &User, reference: &str) -> Result<Value, String> {
    let reference = reference.trim();
    if let Ok(id) = reference.parse::<i64>() {
        if id <= 0 {
            return Err("订阅源 ID 无效".into());
        }
        return db
            .kol_for(user.id, user.is_admin, id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "订阅源不存在或不可见".into());
    }
    let platform = if reference.contains("xueqiu.com") {
        "xueqiu"
    } else if reference.contains("weibo.com") {
        "weibo"
    } else {
        return Err("只支持订阅源 ID、雪球主页 URL 或微博主页 URL".into());
    };
    let external_id = crate::db::normalize_external_id(platform, reference);
    if external_id.is_empty() {
        return Err("订阅源 URL 无效".into());
    }
    let items = db
        .catalog(user.id, user.is_admin, "", 0)
        .await
        .map_err(|err| err.to_string())?;
    items
        .into_iter()
        .find(|item| {
            item.get("platform").and_then(Value::as_str) == Some(platform)
                && item.get("external_id").and_then(Value::as_str) == Some(external_id.as_str())
        })
        .ok_or_else(|| "订阅源不存在或不可见".into())
}

fn format_catalog(items: &[Value], page: Option<(usize, usize)>) -> String {
    let mut lines = Vec::with_capacity(items.len() + 1);
    if let Some((page, total)) = page {
        let pages = total.div_ceil(PAGE_SIZE);
        lines.push(format!("订阅源（第 {page}/{pages} 页，共 {total} 个）："));
    }
    for item in items {
        let id = item.get("id").and_then(Value::as_i64).unwrap_or(0);
        let name = item.get("name").and_then(Value::as_str).unwrap_or("未知");
        let platform = item
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let kind = item
            .get("subscribe_type")
            .and_then(Value::as_str)
            .filter(|kind| !kind.is_empty())
            .map(|kind| format!(" [{kind}]"))
            .unwrap_or_default();
        lines.push(format!("{id}. {name} ({platform}){kind}"));
    }
    bounded(lines.join("\n"))
}

fn display_name(item: &Value) -> &str {
    item.get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("订阅源")
}

fn normalize_bind_code(raw: &str) -> Option<String> {
    let normalized: String = raw
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_uppercase)
        .collect();
    if normalized.chars().count() != 8
        || !normalized
            .chars()
            .all(|character| BIND_CODE_ALPHABET.contains(character))
    {
        return None;
    }
    Some(normalized)
}

fn response(text: impl Into<String>) -> TelegramResponse {
    TelegramResponse {
        text: bounded(text.into()),
        keyboard: None,
    }
}

fn bounded(text: String) -> String {
    text.chars().take(MAX_OUTPUT_CHARS).collect()
}

fn help_text() -> &'static str {
    "/start  开始使用或处理绑定链接\n/help  查看帮助\n/list [页码]  查看可见订阅源\n/search 关键词  搜索订阅源\n/sub ID或URL [post|reply|both]  订阅\n/unsub ID或URL  取消订阅\n/mysubs  查看我的订阅\n/bind 绑定码  绑定网页账号"
}

fn catalog_error(error: CatalogError) -> String {
    match error {
        CatalogError::Missing(message)
        | CatalogError::Bad(message)
        | CatalogError::Limited(message)
        | CatalogError::Conflict(message) => message.to_string(),
        CatalogError::Invalid(message) => message,
        CatalogError::Db(error) => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    async fn db() -> Db {
        Db::open(Path::new(":memory:")).await.unwrap()
    }

    fn private(text: &str) -> TelegramMessage {
        TelegramMessage {
            chat_id: "42".into(),
            chat_type: TelegramChatType::Private,
            display_name: "Ada".into(),
            text: Some(text.into()),
        }
    }

    #[test]
    fn parser_accepts_commands_and_normalizes_pasted_codes() {
        assert!(matches!(parse_command("/help"), Some(Command::Help)));
        assert!(matches!(parse_command("/list 2"), Some(Command::List(2))));
        assert!(parse_command("ab cd efg").is_none());
        assert!(parse_command("ab cd ef").is_none());
        assert!(
            matches!(parse_command("abcd2345"), Some(Command::Bind(code)) if code == "ABCD2345")
        );
        assert!(
            matches!(parse_command("/start bind_abcd2345"), Some(Command::Bind(code)) if code == "abcd2345")
        );
    }

    #[tokio::test]
    async fn private_identity_is_created_but_group_and_malformed_updates_are_ignored() {
        let db = db().await;
        let mut group = private("/help");
        group.chat_type = TelegramChatType::Group;
        assert!(handle_message(&db, group, 1_000).await.unwrap().is_none());
        assert!(db.user_by_telegram_chat_id("42").await.unwrap().is_none());

        let malformed = TelegramMessage {
            chat_id: "42".into(),
            chat_type: TelegramChatType::Private,
            display_name: "Ada".into(),
            text: None,
        };
        assert!(handle_message(&db, malformed, 1_000)
            .await
            .unwrap()
            .is_none());
        assert!(db.user_by_telegram_chat_id("42").await.unwrap().is_none());

        let result = handle_message(&db, private("/help"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(result.text.contains("/list"));
        assert!(result.keyboard.is_none());
        assert!(db.user_by_telegram_chat_id("42").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn list_search_subscribe_unsubscribe_and_invalid_refs_are_bounded() {
        let db = db().await;
        for index in 0..25 {
            db.add_kol(
                "weibo",
                &format!("source-{index}"),
                &format!("{index}"),
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        }
        let xueqiu_id = db
            .add_kol("xueqiu", "雪球源", "123456", None, false, false, false)
            .await
            .unwrap();
        let user = handle_message(&db, private("/help"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(user.text.contains("/list"));

        let page = handle_message(&db, private("/list 2"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(page.text.contains("第 2/2 页"));

        let search = handle_message(&db, private("/search source"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(search.text.lines().count() <= SEARCH_LIMIT + 1);

        let subscribed = handle_message(&db, private(&format!("/sub {xueqiu_id} both")), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(subscribed.text.contains("已订阅"));
        let by_url = handle_message(
            &db,
            private("/sub https://xueqiu.com/u/123456 reply"),
            1_000,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(by_url.text.contains("已订阅"));
        let subscriptions = handle_message(&db, private("/mysubs"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(subscriptions.text.contains("[reply]"));

        let removed = handle_message(&db, private(&format!("/unsub {xueqiu_id}")), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(removed.text.contains("已取消订阅"));
        let invalid = handle_message(&db, private("/sub 999999"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(invalid.text.contains("不存在") || invalid.text.contains("不可见"));
        assert!(handle_message(&db, private("/sub 1 invalid"), 1_000)
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("/sub"));

        sqlx::query("UPDATE kols SET name = ? WHERE id = ?")
            .bind("x".repeat(5_000))
            .bind(xueqiu_id)
            .execute(db.pool())
            .await
            .unwrap();
        let bounded_response = handle_message(&db, private("/list"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(bounded_response.text.chars().count() <= MAX_OUTPUT_CHARS);
    }

    #[tokio::test]
    async fn catalog_and_subscribe_reject_acl_hidden_sources() {
        let db = db().await;
        let hidden_id = db
            .add_kol("weibo", "private-source", "9988", None, false, false, false)
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET is_private = 1 WHERE id = ?")
            .bind(hidden_id)
            .execute(db.pool())
            .await
            .unwrap();
        let user = db
            .get_or_create_telegram_user("hidden-test", "Reader", true)
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .kol_for(user.id, user.is_admin, hidden_id)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .subscribe(user.id, user.is_admin, hidden_id, "post")
            .await
            .is_err());
        let response = handle_message(
            &db,
            {
                TelegramMessage {
                    chat_id: "hidden-test".into(),
                    ..private(&format!("/sub {hidden_id}"))
                }
            },
            1_000,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.text.contains("不可见") || response.text.contains("不存在"));
    }

    #[tokio::test]
    async fn bind_codes_are_single_use_and_persistently_throttled() {
        let db = db().await;
        let owner = db
            .get_or_create_telegram_user("owner", "Owner", true)
            .await
            .unwrap()
            .unwrap();
        let (code, _) = db.issue_bind_code(owner.id, 1_000).await.unwrap();
        let mut wrong = private("/bind badcode");
        for now in 1_000..1_005 {
            wrong.text = Some(format!("/bind wrong{now}"));
            let result = handle_message(&db, wrong.clone(), now)
                .await
                .unwrap()
                .unwrap();
            assert!(result.text.contains("无效") || result.text.contains("频繁"));
        }
        let limited = handle_message(
            &db,
            TelegramMessage {
                text: Some(format!("/bind {code}")),
                ..private("/help")
            },
            1_005,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(limited.text.contains("频繁"));

        let success = handle_message(
            &db,
            TelegramMessage {
                chat_id: "new-chat".into(),
                text: Some(format!("/bind {}", code.to_ascii_lowercase())),
                ..private("/help")
            },
            1_600,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(success.text.contains("成功"));
        let reused = handle_message(
            &db,
            TelegramMessage {
                chat_id: "third-chat".into(),
                text: Some(format!("/bind {code}")),
                ..private("/help")
            },
            1_601,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(reused.text.contains("无效") || reused.text.contains("过期"));
    }
}
