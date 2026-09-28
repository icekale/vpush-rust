use crate::db::{CatalogError, Db, User};
use serde_json::Value;

const PAGE_SIZE: usize = 20;
const SEARCH_LIMIT: usize = 10;
const MAX_INPUT_CHARS: usize = 4096;
const MAX_OUTPUT_CHARS: usize = 4096;
const BIND_TRY_LIMIT: i64 = 5;
const BIND_TRY_WINDOW_SECS: i64 = 600;
const ASK_TRY_LIMIT: i64 = 5;
const ASK_TRY_WINDOW_SECS: i64 = 600;
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
pub struct TelegramCallback {
    pub id: String,
    pub chat_id: String,
    pub message_chat_id: String,
    pub chat_type: TelegramChatType,
    pub message_id: i64,
    pub data: String,
    pub message_text: String,
    pub message_keyboard: Vec<Vec<TelegramButton>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramEditMessage {
    pub chat_id: String,
    pub message_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramButton {
    pub text: String,
    pub callback_data: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramNotification {
    pub chat_id: String,
    pub text: String,
    pub keyboard: Option<Vec<Vec<TelegramButton>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramResponse {
    pub text: String,
    pub keyboard: Option<Vec<Vec<TelegramButton>>>,
    pub notifications: Vec<TelegramNotification>,
    pub answer_callback_query_id: Option<String>,
    pub edit_message: Option<TelegramEditMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramMessage {
    pub chat_id: String,
    pub chat_type: TelegramChatType,
    pub display_name: String,
    pub text: Option<String>,
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
    Ask(String, String, String),
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
        Some(Command::Start(None)) => Ok(response_with_keyboard(
            "欢迎使用 VPush。\n\n".to_owned() + help_text(),
            start_keyboard(),
        )),
        Some(Command::Help) => Ok(response_with_keyboard(help_text(), common_keyboard())),
        Some(Command::List(page)) => list(db, &user, page).await,
        Some(Command::Search(keyword)) => search(db, &user, &keyword).await,
        Some(Command::Subscribe(reference, kind)) => subscribe(db, &user, &reference, &kind).await,
        Some(Command::Unsubscribe(reference)) => unsubscribe(db, &user, &reference).await,
        Some(Command::MySubscriptions) => my_subscriptions(db, &user).await,
        Some(Command::Ask(platform, raw, name)) => {
            ask(db, &user, &platform, &raw, &name, now).await
        }
        None => Ok(response(help_text())),
    };
    let response = match result {
        Ok(response) => response,
        Err(message) => response(message),
    };
    Ok(Some(response))
}

pub async fn handle_callback(
    db: &Db,
    callback: TelegramCallback,
) -> Result<Option<TelegramResponse>, String> {
    if callback.chat_type != TelegramChatType::Private {
        return Ok(None);
    }
    let chat_id = callback.chat_id.trim();
    if chat_id.is_empty()
        || callback.message_chat_id.trim() != chat_id
        || callback.data.trim().is_empty()
        || callback.data.chars().count() > MAX_INPUT_CHARS
    {
        return Ok(None);
    }

    let Some(user) = db
        .user_by_telegram_chat_id(chat_id)
        .await
        .map_err(|err| err.to_string())?
    else {
        return Ok(Some(callback_response(
            &callback,
            response("Telegram 私聊身份无效"),
            false,
        )));
    };
    let result = dispatch_callback(db, &user, &callback).await;
    let (response, edit) = match result {
        Ok((response, edit)) => (response, edit),
        Err(message) => (response(message), false),
    };
    Ok(Some(callback_response(&callback, response, edit)))
}

async fn dispatch_callback(
    db: &Db,
    user: &User,
    callback: &TelegramCallback,
) -> Result<(TelegramResponse, bool), String> {
    let data = callback.data.trim();
    let mut parts = data.split(':');
    let action = parts.next().unwrap_or_default();
    match action {
        "list" => callback_list(db, user, &mut parts).await,
        "mysubs" => callback_mysubs(db, user, &mut parts).await,
        "help" if parts.next().is_none() => Ok((help_text_with_keyboard(), true)),
        "sub" if parts.clone().count() == 1 => {
            callback_subscribe(db, user, parts.next().unwrap_or_default()).await
        }
        "unsub" if parts.clone().count() == 1 => {
            callback_unsub(db, user, callback, parts.next().unwrap_or_default()).await
        }
        "sec" if parts.clone().count() == 1 => {
            callback_secondary(db, user, callback, parts.next().unwrap_or_default()).await
        }
        "secundo" | "unsubundo" if parts.clone().count() == 1 => {
            callback_undo(db, user, callback, action, parts.next().unwrap_or_default()).await
        }
        "approve" if user.is_admin && parts.clone().count() == 1 => {
            callback_approve(db, user, parts.next().unwrap_or_default()).await
        }
        "apcat" if user.is_admin && parts.clone().count() == 2 => {
            callback_approve_category(
                db,
                user,
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            )
            .await
        }
        "reject" if user.is_admin && parts.clone().count() == 1 => {
            callback_reject(db, user, parts.next().unwrap_or_default()).await
        }
        "approve" | "apcat" | "reject" => Err("仅管理员可以处理申请".into()),
        _ => Ok((response("按钮已失效，请重新发送 /list。"), false)),
    }
}

async fn callback_list<'a>(
    db: &Db,
    user: &User,
    parts: &mut impl Iterator<Item = &'a str>,
) -> Result<(TelegramResponse, bool), String> {
    let direction = parts.next().unwrap_or_default();
    let raw_page = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return Ok((response("按钮已失效，请重新发送 /list。"), false));
    }
    let page = raw_page
        .parse::<usize>()
        .ok()
        .filter(|page| *page > 0)
        .ok_or_else(|| "页码无效".to_string())?;
    let target = match direction {
        "prev" => page,
        "next" => page.saturating_add(1),
        _ => return Ok((response("按钮已失效，请重新发送 /list。"), false)),
    };
    Ok((list(db, user, target).await?, true))
}

async fn callback_mysubs<'a>(
    db: &Db,
    user: &User,
    parts: &mut impl Iterator<Item = &'a str>,
) -> Result<(TelegramResponse, bool), String> {
    match parts.next() {
        None => Ok((my_subscriptions(db, user).await?, true)),
        Some("type") => {
            let id = callback_id(parts.next(), parts.next())?;
            let kol = db
                .kol_for(user.id, user.is_admin, id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| "订阅源不存在或不可见".to_string())?;
            if !is_subscribed(&kol) {
                return Err("尚未订阅该订阅源".into());
            }
            let current = kol
                .get("subscribe_type")
                .and_then(Value::as_str)
                .unwrap_or("post");
            let next = next_subscription_type(current).ok_or_else(|| "订阅类型无效".to_string())?;
            db.set_subscription_type(user.id, id, next)
                .await
                .map_err(catalog_error)?;
            Ok((my_subscriptions(db, user).await?, true))
        }
        Some("unsub") => {
            let id = callback_id(parts.next(), parts.next())?;
            if parts.next().is_some() {
                return Ok((response("按钮已失效，请重新发送 /mysubs。"), false));
            }
            callback_unsubscribe_id(db, user, id).await
        }
        _ => Ok((response("按钮已失效，请重新发送 /mysubs。"), false)),
    }
}

fn callback_id(first: Option<&str>, extra: Option<&str>) -> Result<i64, String> {
    if extra.is_some() {
        return Err("订阅源 ID 无效".into());
    }
    first
        .ok_or_else(|| "订阅源 ID 无效".to_string())?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "订阅源 ID 无效".to_string())
}

async fn callback_subscribe(
    db: &Db,
    user: &User,
    raw_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let id = callback_id(Some(raw_id), None)?;
    let kol = db
        .kol_for(user.id, user.is_admin, id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "订阅源不存在或不可见".to_string())?;
    db.subscribe(user.id, user.is_admin, id, "post")
        .await
        .map_err(catalog_error)?;
    Ok((
        response(format!("已订阅 {}（post）。", display_name(&kol))),
        true,
    ))
}

const UNDO_TTL_SECS: u64 = 30;

struct UndoAction {
    at: std::time::SystemTime,
    kind: &'static str,
    kol_id: i64,
    subscribe_type: String,
    favorite: bool,
    secondary: bool,
    text: String,
    keyboard: Vec<Vec<TelegramButton>>,
}

fn undo_key(chat_id: &str, message_id: i64) -> String {
    format!("{chat_id}:{message_id}")
}

fn undo_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, UndoAction>> {
    static STORE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, UndoAction>>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn remember_undo(callback: &TelegramCallback, action: UndoAction) {
    let mut store = undo_store().lock().unwrap_or_else(|err| err.into_inner());
    store.insert(undo_key(&callback.chat_id, callback.message_id), action);
}

fn take_undo(callback: &TelegramCallback, kol_id: i64, kind: &str) -> Result<UndoAction, String> {
    let mut store = undo_store().lock().unwrap_or_else(|err| err.into_inner());
    let key = undo_key(&callback.chat_id, callback.message_id);
    let Some(action) = store.remove(&key) else {
        return Err("撤销超时或操作已失效（30 秒内可撤销）".into());
    };
    let fresh = action
        .at
        .elapsed()
        .is_ok_and(|age| age.as_secs() <= UNDO_TTL_SECS);
    if !fresh || action.kol_id != kol_id || action.kind != kind {
        return Err("撤销超时或操作已失效（30 秒内可撤销）".into());
    }
    Ok(action)
}

async fn callback_unsub(
    db: &Db,
    user: &User,
    callback: &TelegramCallback,
    raw_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let id = callback_id(Some(raw_id), None)?;
    let kol = db
        .kol_for(user.id, user.is_admin, id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "订阅源不存在或不可见".to_string())?;
    if !is_subscribed(&kol) {
        return Err("尚未订阅该订阅源".into());
    }
    let kind = kol
        .get("subscribe_type")
        .and_then(Value::as_str)
        .unwrap_or("post")
        .to_owned();
    let favorite = kol
        .get("favorite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let secondary = kol
        .get("secondary")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    db.unsubscribe(user.id, id)
        .await
        .map_err(|err| err.to_string())?;
    let name = display_name(&kol);
    remember_undo(
        callback,
        UndoAction {
            at: std::time::SystemTime::now(),
            kind: "unsub",
            kol_id: id,
            subscribe_type: kind,
            favorite,
            secondary,
            text: callback.message_text.clone(),
            keyboard: callback.message_keyboard.clone(),
        },
    );
    let text = if callback.message_text.is_empty() {
        format!("已取消订阅 {name}。")
    } else {
        callback.message_text.clone()
    };
    Ok((
        response_with_keyboard(
            text,
            Some(vec![vec![button(
                &format!("撤销退订「{name}」"),
                format!("unsubundo:{id}"),
            )]]),
        ),
        true,
    ))
}

async fn callback_secondary(
    db: &Db,
    user: &User,
    callback: &TelegramCallback,
    raw_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let id = callback_id(Some(raw_id), None)?;
    let kol = db
        .kol_for(user.id, user.is_admin, id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "订阅源不存在或不可见".to_string())?;
    let name = display_name(&kol);
    if !is_subscribed(&kol) {
        return Ok((response(format!("未订阅「{name}」，无法设置次要")), true));
    }
    let was_secondary = kol
        .get("secondary")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    db.set_secondary(user.id, id, !was_secondary)
        .await
        .map_err(catalog_error)?;
    remember_undo(
        callback,
        UndoAction {
            at: std::time::SystemTime::now(),
            kind: "sec",
            kol_id: id,
            subscribe_type: String::new(),
            favorite: false,
            secondary: was_secondary,
            text: callback.message_text.clone(),
            keyboard: callback.message_keyboard.clone(),
        },
    );
    let text = if was_secondary {
        format!("已恢复实时推送：「{name}」")
    } else {
        format!("已设为次要：「{name}」新帖合并推送")
    };
    Ok((
        response_with_keyboard(
            text,
            Some(vec![vec![button("撤销", format!("secundo:{id}"))]]),
        ),
        true,
    ))
}

async fn callback_undo(
    db: &Db,
    user: &User,
    callback: &TelegramCallback,
    action: &str,
    raw_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let id = callback_id(Some(raw_id), None)?;
    let kind = if action == "unsubundo" {
        "unsub"
    } else {
        "sec"
    };
    let saved = match take_undo(callback, id, kind) {
        Ok(saved) => saved,
        Err(message) => return Ok((response(message), true)),
    };
    if kind == "unsub" {
        db.subscribe(user.id, user.is_admin, id, &saved.subscribe_type)
            .await
            .map_err(catalog_error)?;
        db.set_favorite(user.id, id, saved.favorite)
            .await
            .map_err(catalog_error)?;
        db.set_secondary(user.id, id, saved.secondary)
            .await
            .map_err(catalog_error)?;
    } else {
        db.set_secondary(user.id, id, saved.secondary)
            .await
            .map_err(catalog_error)?;
    }
    let text = if saved.text.is_empty() {
        "已撤销".to_owned()
    } else {
        saved.text
    };
    Ok((
        response_with_keyboard(text, Some(saved.keyboard).filter(|rows| !rows.is_empty())),
        true,
    ))
}

fn button(text: &str, callback_data: String) -> TelegramButton {
    TelegramButton {
        text: text.to_owned(),
        callback_data,
        url: String::new(),
    }
}

#[cfg(test)]
fn expire_undo(chat_id: &str, message_id: i64) {
    let mut store = undo_store().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(action) = store.get_mut(&undo_key(chat_id, message_id)) {
        action.at = std::time::SystemTime::UNIX_EPOCH;
    }
}

async fn callback_unsubscribe_id(
    db: &Db,
    user: &User,
    id: i64,
) -> Result<(TelegramResponse, bool), String> {
    let kol = db
        .kol_for(user.id, user.is_admin, id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "订阅源不存在或不可见".to_string())?;
    if !is_subscribed(&kol) {
        return Err("尚未订阅该订阅源".into());
    }
    db.unsubscribe(user.id, id)
        .await
        .map_err(|err| err.to_string())?;
    Ok((
        response(format!("已取消订阅 {}。", display_name(&kol))),
        true,
    ))
}

async fn callback_approve(
    db: &Db,
    _user: &User,
    raw_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let request_id = parse_request_id(raw_id)?;
    if !db
        .kol_request_pending(request_id)
        .await
        .map_err(|err| err.to_string())?
    {
        return Err("申请不存在或已处理".into());
    }
    let categories = db.categories().await.map_err(|err| err.to_string())?;
    let mut rows = Vec::new();
    for category in categories {
        let Some(category_id) = category.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let name = category
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("分类");
        let mut row = Vec::new();
        push_button(&mut row, name, format!("apcat:{request_id}:{category_id}"));
        if !row.is_empty() {
            rows.push(row);
        }
    }
    let mut no_category = Vec::new();
    push_button(&mut no_category, "不分类", format!("apcat:{request_id}:0"));
    rows.push(no_category);
    Ok((
        response_with_keyboard("请选择分类后批准申请：", nonempty_keyboard(rows)),
        true,
    ))
}

async fn callback_approve_category(
    db: &Db,
    user: &User,
    raw_request_id: &str,
    raw_category_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let request_id = parse_request_id(raw_request_id)?;
    let category_id = raw_category_id
        .parse::<i64>()
        .ok()
        .filter(|id| *id >= 0)
        .ok_or_else(|| "分类 ID 无效".to_string())?;
    let effect = db
        .approve_kol_request_as(request_id, Some(category_id), user.id)
        .await
        .map_err(catalog_error)?;
    let mut result = response(format!(
        "已批准申请 #{request_id}，{}已加入目录并自动订阅。",
        effect.name
    ));
    add_applicant_notification(&mut result, &effect, true);
    Ok((result, true))
}

async fn callback_reject(
    db: &Db,
    user: &User,
    raw_id: &str,
) -> Result<(TelegramResponse, bool), String> {
    let request_id = parse_request_id(raw_id)?;
    let effect = db
        .reject_kol_request_as(request_id, user.id)
        .await
        .map_err(catalog_error)?;
    let mut result = response(format!("已拒绝申请 #{request_id}。"));
    add_applicant_notification(&mut result, &effect, false);
    Ok((result, true))
}

fn parse_request_id(raw: &str) -> Result<i64, String> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "申请 ID 无效".into())
}

fn add_applicant_notification(
    response: &mut TelegramResponse,
    effect: &crate::db::KolRequestEffect,
    approved: bool,
) {
    if effect.applicant_chat_id.trim().is_empty() {
        return;
    }
    let text = if approved {
        format!(
            "申请已通过：{}（{}:{}），已自动订阅。",
            effect.name, effect.platform, effect.external_id
        )
    } else {
        format!(
            "申请未通过：{}（{}:{}）。",
            effect.name, effect.platform, effect.external_id
        )
    };
    response.notifications.push(TelegramNotification {
        chat_id: effect.applicant_chat_id.clone(),
        text: bounded(text),
        keyboard: None,
    });
}

fn response_with_notifications(
    mut response: TelegramResponse,
    notifications: Vec<TelegramNotification>,
) -> TelegramResponse {
    response.notifications = notifications;
    response
}

fn ask_keyboard(request_id: i64) -> Option<Vec<Vec<TelegramButton>>> {
    let mut row = Vec::new();
    push_button(&mut row, "批准并选择分类", format!("approve:{request_id}"));
    push_button(&mut row, "拒绝", format!("reject:{request_id}"));
    nonempty_keyboard(vec![row])
}

async fn ask(
    db: &Db,
    user: &User,
    platform: &str,
    raw: &str,
    name: &str,
    now: i64,
) -> Result<TelegramResponse, String> {
    if !take_quota(
        db,
        &format!("ask:telegram:{}", user.id),
        now,
        ASK_TRY_LIMIT,
        ASK_TRY_WINDOW_SECS,
    )
    .await?
    {
        return Ok(response("申请提交过于频繁，请 10 分钟后重试"));
    }
    let request_id = db
        .add_kol_request_without_category(platform, raw, user.id, name)
        .await
        .map_err(catalog_error)?;
    let request = db
        .list_kol_requests("", user.id)
        .await
        .map_err(|err| err.to_string())?
        .into_iter()
        .find(|item| item["id"].as_i64() == Some(request_id))
        .ok_or_else(|| "申请已创建但读取失败".to_string())?;
    let requester = request["requester"].as_str().unwrap_or("Telegram 用户");
    let display = request["name"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(raw);
    let text = format!(
        "收到添加申请 #{request_id}：{}（{}:{}），申请人：{}。",
        display,
        platform,
        request["external_id"].as_str().unwrap_or(raw),
        requester
    );
    let notifications = db
        .telegram_admin_chat_ids()
        .await
        .map_err(|err| err.to_string())?
        .into_iter()
        .map(|chat_id| TelegramNotification {
            chat_id,
            text: text.clone(),
            keyboard: ask_keyboard(request_id),
        })
        .collect();
    Ok(response_with_notifications(
        response(format!("申请已提交 #{request_id}，等待管理员处理。")),
        notifications,
    ))
}

fn is_subscribed(kol: &Value) -> bool {
    kol.get("subscribed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn next_subscription_type(kind: &str) -> Option<&'static str> {
    match kind {
        "post" => Some("reply"),
        "reply" => Some("both"),
        "both" => Some("post"),
        _ => None,
    }
}

fn callback_response(
    callback: &TelegramCallback,
    mut response: TelegramResponse,
    edit: bool,
) -> TelegramResponse {
    response.answer_callback_query_id = Some(callback.id.clone());
    if edit {
        response.edit_message = Some(TelegramEditMessage {
            chat_id: callback.message_chat_id.trim().to_owned(),
            message_id: callback.message_id,
        });
    }
    response
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
            let keyword = args.join(" ").trim().to_owned();
            if keyword.is_empty() || keyword.chars().count() > 100 {
                return None;
            }
            Some(Command::Search(keyword))
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
        "ask" => parse_ask(&args),
        "bind" => (args.len() == 1).then(|| Command::Bind(args[0].to_owned())),
        _ => None,
    }
}

fn parse_ask(args: &[&str]) -> Option<Command> {
    if args.is_empty() {
        return None;
    }
    let (platform, raw, name_start) = match args[0].to_ascii_lowercase().as_str() {
        "xueqiu" | "雪球" => ("xueqiu", args.get(1).copied()?, 2),
        "weibo" | "微博" => ("weibo", args.get(1).copied()?, 2),
        _ if args[0].contains("xueqiu.com") => ("xueqiu", args[0], 1),
        _ if args[0].contains("weibo.com") || args[0].contains("weibo.cn") => ("weibo", args[0], 1),
        _ => return None,
    };
    if raw.is_empty() || args.len() > name_start + 4 {
        return None;
    }
    let name = args[name_start..].join(" ");
    Some(Command::Ask(platform.to_owned(), raw.to_owned(), name))
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

async fn take_quota(
    db: &Db,
    key: &str,
    now: i64,
    limit: i64,
    window_secs: i64,
) -> Result<bool, String> {
    let window = now.div_euclid(window_secs) * window_secs;
    let mut tx = db.pool().begin().await.map_err(|err| err.to_string())?;
    let quota: Option<(i64, i64)> =
        sqlx::query_as("SELECT period_start, count FROM bind_quota WHERE key = ?")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| err.to_string())?;
    let count = match quota {
        Some((start, count)) if start == window => count,
        _ => 0,
    };
    if count >= limit {
        tx.commit().await.map_err(|err| err.to_string())?;
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO bind_quota (key, period_start, count) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET period_start = excluded.period_start, count = excluded.count",
    )
    .bind(key)
    .bind(window)
    .bind(count + 1)
    .execute(&mut *tx)
    .await
    .map_err(|err| err.to_string())?;
    tx.commit().await.map_err(|err| err.to_string())?;
    Ok(true)
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
    let requested_offset = list_offset(page);
    let (total_count, mut items) = db
        .catalog_page(user.id, user.is_admin, PAGE_SIZE, requested_offset)
        .await
        .map_err(|err| err.to_string())?;
    let total = usize::try_from(total_count.max(0)).unwrap_or(usize::MAX);
    if total == 0 {
        return Ok(response("没有可见订阅源。"));
    }
    let pages = total.div_ceil(PAGE_SIZE);
    let requested_page = page;
    let page = page.clamp(1, pages);
    if requested_page != page {
        let offset = list_offset(page);
        let (_, refreshed) = db
            .catalog_page(user.id, user.is_admin, PAGE_SIZE, offset)
            .await
            .map_err(|err| err.to_string())?;
        items = refreshed;
    }
    if items.is_empty() {
        return Ok(response("没有可见订阅源。"));
    }
    Ok(response_with_keyboard(
        format_catalog(&items, Some((page, total))),
        list_keyboard(&items, page, total),
    ))
}

fn list_offset(page: usize) -> i64 {
    page.checked_sub(1)
        .and_then(|page| page.checked_mul(PAGE_SIZE))
        .and_then(|offset| i64::try_from(offset).ok())
        .unwrap_or(i64::MAX)
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
    Ok(response_with_keyboard(
        format_catalog(&matches, None),
        search_keyboard(&matches),
    ))
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
        .visible_my_subscriptions(user.id, user.is_admin)
        .await
        .map_err(|err| err.to_string())?;
    if items.is_empty() {
        return Ok(response_with_keyboard(
            "还没有订阅。发送 /list 查看可用订阅源。",
            common_keyboard(),
        ));
    }
    Ok(response_with_keyboard(
        format_catalog(&items, None),
        my_subscriptions_keyboard(&items),
    ))
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

fn list_keyboard(items: &[Value], page: usize, total: usize) -> Option<Vec<Vec<TelegramButton>>> {
    let mut rows = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_i64).filter(|id| *id > 0) else {
            continue;
        };
        let mut row = Vec::new();
        push_button(&mut row, "订阅", format!("sub:{id}"));
        push_button(&mut row, "取消订阅", format!("unsub:{id}"));
        if !row.is_empty() {
            rows.push(row);
        }
    }
    let pages = total.div_ceil(PAGE_SIZE);
    let mut navigation = Vec::new();
    if page > 1 {
        push_button(&mut navigation, "上一页", format!("list:prev:{}", page - 1));
    }
    if page < pages {
        push_button(&mut navigation, "下一页", format!("list:next:{page}"));
    }
    if !navigation.is_empty() {
        rows.push(navigation);
    }
    append_common_buttons(&mut rows);
    nonempty_keyboard(rows)
}

fn search_keyboard(items: &[Value]) -> Option<Vec<Vec<TelegramButton>>> {
    let mut rows = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_i64).filter(|id| *id > 0) else {
            continue;
        };
        let mut row = Vec::new();
        push_button(&mut row, "订阅", format!("sub:{id}"));
        push_button(&mut row, "取消订阅", format!("unsub:{id}"));
        if !row.is_empty() {
            rows.push(row);
        }
    }
    append_common_buttons(&mut rows);
    nonempty_keyboard(rows)
}

fn my_subscriptions_keyboard(items: &[Value]) -> Option<Vec<Vec<TelegramButton>>> {
    let mut rows = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_i64).filter(|id| *id > 0) else {
            continue;
        };
        let mut row = Vec::new();
        push_button(&mut row, "切换类型", format!("mysubs:type:{id}"));
        push_button(&mut row, "取消订阅", format!("mysubs:unsub:{id}"));
        if !row.is_empty() {
            rows.push(row);
        }
    }
    append_common_buttons(&mut rows);
    nonempty_keyboard(rows)
}

fn start_keyboard() -> Option<Vec<Vec<TelegramButton>>> {
    let mut rows = Vec::new();
    let mut first = Vec::new();
    push_button(&mut first, "查看订阅源", "list:1".to_owned());
    if !first.is_empty() {
        rows.push(first);
    }
    append_common_buttons(&mut rows);
    nonempty_keyboard(rows)
}

fn common_keyboard() -> Option<Vec<Vec<TelegramButton>>> {
    let mut rows = Vec::new();
    append_common_buttons(&mut rows);
    nonempty_keyboard(rows)
}

fn help_text_with_keyboard() -> TelegramResponse {
    response_with_keyboard(help_text(), common_keyboard())
}

fn append_common_buttons(rows: &mut Vec<Vec<TelegramButton>>) {
    let mut row = Vec::new();
    push_button(&mut row, "订阅源", "list:1".to_owned());
    push_button(&mut row, "我的订阅", "mysubs".to_owned());
    push_button(&mut row, "帮助", "help".to_owned());
    if !row.is_empty() {
        rows.push(row);
    }
}

fn push_button(row: &mut Vec<TelegramButton>, text: &str, callback_data: String) {
    if callback_data.len() <= 64 {
        row.push(TelegramButton {
            text: text.to_owned(),
            callback_data,
            url: String::new(),
        });
    }
}

fn nonempty_keyboard(rows: Vec<Vec<TelegramButton>>) -> Option<Vec<Vec<TelegramButton>>> {
    let rows: Vec<Vec<TelegramButton>> = rows.into_iter().filter(|row| !row.is_empty()).collect();
    (!rows.is_empty()).then_some(rows)
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
    response_with_keyboard(text, None)
}

fn response_with_keyboard(
    text: impl Into<String>,
    keyboard: Option<Vec<Vec<TelegramButton>>>,
) -> TelegramResponse {
    TelegramResponse {
        text: bounded(text.into()),
        keyboard,
        notifications: Vec::new(),
        answer_callback_query_id: None,
        edit_message: None,
    }
}

fn bounded(text: String) -> String {
    text.chars().take(MAX_OUTPUT_CHARS).collect()
}

fn help_text() -> &'static str {
    "/start  开始使用或处理绑定链接\n/help  查看帮助\n/list [页码]  查看可见订阅源\n/search 关键词  搜索订阅源\n/sub ID或URL [post|reply|both]  订阅\n/unsub ID或URL  取消订阅\n/mysubs  查看我的订阅\n/ask 平台 ID或主页URL [名称]  申请添加订阅源\n/bind 绑定码  绑定网页账号"
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
    fn callback(data: &str) -> TelegramCallback {
        TelegramCallback {
            id: "callback-1".into(),
            chat_id: "42".into(),
            message_chat_id: "42".into(),
            chat_type: TelegramChatType::Private,
            message_id: 17,
            data: data.into(),
            message_text: String::new(),
            message_keyboard: Vec::new(),
        }
    }

    fn callback_for(data: &str, chat_id: &str, message_chat_id: &str) -> TelegramCallback {
        TelegramCallback {
            id: "callback-2".into(),
            chat_id: chat_id.into(),
            message_chat_id: message_chat_id.into(),
            chat_type: TelegramChatType::Private,
            message_id: 18,
            data: data.into(),
            message_text: String::new(),
            message_keyboard: Vec::new(),
        }
    }

    #[tokio::test]
    async fn secondary_and_unsubscribe_buttons_undo_within_thirty_seconds() {
        let db = db().await;
        let id = db
            .add_kol("weibo", "Alpha", "alpha", None, false, false, false)
            .await
            .unwrap();
        handle_message(&db, private("/help"), 1_000).await.unwrap();
        db.subscribe(1, false, id, "both").await.unwrap();
        db.set_favorite(1, id, true).await.unwrap();
        let mut callback = callback(&format!("sec:{id}"));
        callback.message_id = 900;
        callback.message_text = "通知正文".into();
        callback.message_keyboard = vec![vec![button("原文", String::new())]];
        callback.message_keyboard[0][0].url = "https://example.com/1".into();
        callback.message_keyboard[0][0].callback_data.clear();

        let toggled = handle_callback(&db, callback.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(toggled.text.contains("已设为次要"));
        assert!(db.my_subscriptions(1).await.unwrap()[0]["secondary"]
            .as_bool()
            .unwrap());

        callback.data = format!("secundo:{id}");
        let restored = handle_callback(&db, callback.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.text, "通知正文");
        assert_eq!(
            restored.keyboard.unwrap()[0][0].url,
            "https://example.com/1"
        );
        assert!(!db.my_subscriptions(1).await.unwrap()[0]["secondary"]
            .as_bool()
            .unwrap());

        callback.data = format!("unsub:{id}");
        let removed = handle_callback(&db, callback.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(removed.text, "通知正文");
        assert!(db.my_subscriptions(1).await.unwrap().is_empty());
        callback.data = format!("unsubundo:{id}");
        handle_callback(&db, callback.clone())
            .await
            .unwrap()
            .unwrap();
        let sub = &db.my_subscriptions(1).await.unwrap()[0];
        assert_eq!(sub["subscribe_type"], "both");
        assert!(sub["favorite"].as_bool().unwrap());

        callback.data = format!("unsub:{id}");
        handle_callback(&db, callback.clone())
            .await
            .unwrap()
            .unwrap();
        expire_undo(&callback.chat_id, callback.message_id);
        callback.data = format!("unsubundo:{id}");
        let expired = handle_callback(&db, callback).await.unwrap().unwrap();
        assert!(expired.text.contains("撤销超时"));
        assert!(db.my_subscriptions(1).await.unwrap().is_empty());
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
    async fn callbacks_mutate_only_authorized_private_actor_and_return_transport_targets() {
        let db = db().await;
        let id = db
            .add_kol(
                "weibo",
                "callback-source",
                "callback-1",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        handle_message(&db, private("/help"), 1_000).await.unwrap();

        let subscribed = handle_callback(&db, callback(&format!("sub:{id}")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            subscribed.answer_callback_query_id.as_deref(),
            Some("callback-1")
        );
        assert_eq!(
            subscribed.edit_message,
            Some(TelegramEditMessage {
                chat_id: "42".into(),
                message_id: 17,
            })
        );
        assert_eq!(db.my_subscriptions(1).await.unwrap().len(), 1);

        let changed = handle_callback(&db, callback(&format!("mysubs:type:{id}")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            changed.answer_callback_query_id.as_deref(),
            Some("callback-1")
        );
        assert_eq!(
            db.my_subscriptions(1).await.unwrap()[0]["subscribe_type"],
            "reply"
        );
        handle_callback(&db, callback(&format!("mysubs:type:{id}")))
            .await
            .unwrap();
        assert_eq!(
            db.my_subscriptions(1).await.unwrap()[0]["subscribe_type"],
            "both"
        );
        handle_callback(&db, callback(&format!("mysubs:type:{id}")))
            .await
            .unwrap();
        assert_eq!(
            db.my_subscriptions(1).await.unwrap()[0]["subscribe_type"],
            "post"
        );

        handle_message(
            &db,
            TelegramMessage {
                chat_id: "43".into(),
                ..private("/help")
            },
            1_000,
        )
        .await
        .unwrap();
        assert!(
            handle_callback(&db, callback_for(&format!("unsub:{id}"), "43", "42"))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(db.my_subscriptions(1).await.unwrap().len(), 1);

        let removed = handle_callback(&db, callback(&format!("mysubs:unsub:{id}")))
            .await
            .unwrap()
            .unwrap();
        assert!(removed.text.contains("已取消订阅"));
        assert!(db.my_subscriptions(1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn callbacks_reject_groups_acl_hidden_ids_and_random_data_without_mutation() {
        let db = db().await;
        let public_id = db
            .add_kol(
                "weibo",
                "public-callback",
                "public-1",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        let private_id = db
            .add_kol(
                "weibo",
                "private-callback",
                "private-1",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET is_private = 1 WHERE id = ?")
            .bind(private_id)
            .execute(db.pool())
            .await
            .unwrap();
        handle_message(&db, private("/help"), 1_000).await.unwrap();

        let mut group = callback(&format!("sub:{public_id}"));
        group.chat_type = TelegramChatType::Group;
        assert!(handle_callback(&db, group).await.unwrap().is_none());
        assert!(handle_callback(&db, callback(&format!("sub:{private_id}")))
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("不可见"));
        assert!(handle_callback(&db, callback("sub:0"))
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("无效"));
        assert!(handle_callback(&db, callback("sub:9223372036854775808"))
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("无效"));
        let random = handle_callback(&db, callback("random:999"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            random.answer_callback_query_id.as_deref(),
            Some("callback-1")
        );
        assert!(random.edit_message.is_none());
        assert!(db.my_subscriptions(1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn paged_catalog_bounds_rows_and_clamps_extreme_list_pages() {
        let db = db().await;
        for index in 0..45 {
            db.add_kol(
                "weibo",
                &format!("bounded source {index}"),
                &format!("bounded-{index}"),
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        }
        let private_id = db
            .add_kol(
                "weibo",
                "unauthorized private source",
                "unauthorized-private",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET is_private = 1 WHERE id = ?")
            .bind(private_id)
            .execute(db.pool())
            .await
            .unwrap();
        handle_message(&db, private("/help"), 1_000).await.unwrap();

        let (total, first_page) = db.catalog_page(1, false, 100, 0).await.unwrap();
        assert_eq!(total, 45);
        assert_eq!(first_page.len(), PAGE_SIZE);
        assert!(first_page
            .iter()
            .all(|item| item["id"].as_i64() != Some(private_id)));

        let extreme = handle_message(&db, private(&format!("/list {}", usize::MAX)), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(extreme.text.contains("第 3/3 页"));
        assert!(extreme.text.lines().skip(1).count().le(&PAGE_SIZE));
        assert!(!extreme.text.contains("unauthorized private source"));
    }

    #[tokio::test]
    async fn telegram_mysubs_filters_revoked_acl_and_disabled_sources_without_deleting() {
        let db = db().await;
        let revoked_id = db
            .add_kol(
                "weibo",
                "revoked private source",
                "revoked-private",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET is_private = 1 WHERE id = ?")
            .bind(revoked_id)
            .execute(db.pool())
            .await
            .unwrap();
        let disabled_id = db
            .add_kol(
                "weibo",
                "disabled source",
                "disabled-source",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        handle_message(&db, private("/help"), 1_000).await.unwrap();
        sqlx::query("INSERT INTO kol_acl (kol_id, user_id) VALUES (?, ?)")
            .bind(revoked_id)
            .bind(1_i64)
            .execute(db.pool())
            .await
            .unwrap();
        db.subscribe(1, false, revoked_id, "post").await.unwrap();
        db.subscribe(1, false, disabled_id, "reply").await.unwrap();
        sqlx::query("DELETE FROM kol_acl WHERE kol_id = ? AND user_id = ?")
            .bind(revoked_id)
            .bind(1_i64)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET enabled = 0 WHERE id = ?")
            .bind(disabled_id)
            .execute(db.pool())
            .await
            .unwrap();

        let response = handle_message(&db, private("/mysubs"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(response.text.contains("还没有订阅"));
        assert!(!response.text.contains("revoked private source"));
        assert!(!response.text.contains("disabled source"));
        assert!(response.keyboard.unwrap().iter().flatten().all(|button| {
            button.callback_data != format!("mysubs:type:{revoked_id}")
                && button.callback_data != format!("mysubs:unsub:{revoked_id}")
                && button.callback_data != format!("mysubs:type:{disabled_id}")
                && button.callback_data != format!("mysubs:unsub:{disabled_id}")
        }));
        assert_eq!(db.my_subscriptions(1).await.unwrap().len(), 2);

        let rejected = handle_callback(&db, callback(&format!("mysubs:type:{revoked_id}")))
            .await
            .unwrap()
            .unwrap();
        assert!(rejected.text.contains("不可见") || rejected.text.contains("尚未订阅"));
        let subscriptions = db.my_subscriptions(1).await.unwrap();
        assert_eq!(
            subscriptions
                .iter()
                .find(|item| item["id"].as_i64() == Some(revoked_id))
                .expect("revoked subscription")["subscribe_type"],
            "post"
        );
        assert!(
            handle_callback(&db, callback(&format!("mysubs:unsub:{disabled_id}")))
                .await
                .unwrap()
                .unwrap()
                .text
                .contains("不可见")
        );
        assert_eq!(db.my_subscriptions(1).await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn callback_pagination_boundaries_and_multiword_search_are_supported() {
        let db = db().await;
        for index in 0..21 {
            db.add_kol(
                "weibo",
                &format!("page source {index}"),
                &format!("page-{index}"),
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        }
        db.add_kol(
            "weibo",
            "multi word target",
            "multi-word",
            None,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        handle_message(&db, private("/help"), 1_000).await.unwrap();

        let page_two = handle_callback(&db, callback("list:next:1"))
            .await
            .unwrap()
            .unwrap();
        assert!(page_two.text.contains("第 2/2 页"));
        let last_page = handle_callback(&db, callback("list:next:2"))
            .await
            .unwrap()
            .unwrap();
        assert!(last_page.text.contains("第 2/2 页"));
        let first_page = handle_callback(&db, callback("list:prev:1"))
            .await
            .unwrap()
            .unwrap();
        assert!(first_page.text.contains("第 1/2 页"));
        assert!(handle_callback(&db, callback("list:prev:0"))
            .await
            .unwrap()
            .unwrap()
            .text
            .contains("无效"));

        let search = handle_message(&db, private("/search multi word"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(search.text.contains("multi word target"));
        assert!(search
            .keyboard
            .unwrap()
            .iter()
            .flatten()
            .any(|button| button.callback_data == "sub:22" || button.callback_data == "sub:23"));
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
        let buttons: Vec<String> = result
            .keyboard
            .unwrap()
            .into_iter()
            .flatten()
            .map(|button| button.callback_data)
            .collect();
        assert!(buttons.iter().any(|data| data == "list:1"));
        assert!(buttons.iter().any(|data| data == "mysubs"));
        assert!(buttons.iter().any(|data| data == "help"));
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

    #[tokio::test]
    async fn ask_and_admin_approval_cover_ids_urls_acl_categories_notifications_and_audit() {
        let db = db().await;
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        db.set_user_text(admin.id, "telegram_chat_id", "admin-chat")
            .await
            .unwrap();
        let category = db.add_category("宏观").await.unwrap();

        let xueqiu = handle_message(&db, private("/ask xueqiu 4514680565 Alpha"), 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(xueqiu.text.contains("申请已提交"));
        assert_eq!(xueqiu.notifications.len(), 1);
        assert_eq!(xueqiu.notifications[0].chat_id, "admin-chat");
        assert!(xueqiu.notifications[0]
            .keyboard
            .as_ref()
            .unwrap()
            .iter()
            .flatten()
            .any(|button| button.callback_data.starts_with("approve:")));

        let applicant = db.user_by_telegram_chat_id("42").await.unwrap().unwrap();
        let requests = db.list_kol_requests("pending", applicant.id).await.unwrap();
        assert_eq!(requests.len(), 1);
        let request_id = requests[0]["id"].as_i64().unwrap();
        assert_eq!(requests[0]["external_id"], "4514680565");
        assert_eq!(requests[0]["requester"], applicant.username);

        let weibo = handle_message(
            &db,
            private("/ask https://weibo.com/u/99887766 微博源"),
            1_001,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(weibo.text.contains("申请已提交"));
        let all_pending = db.list_kol_requests("pending", applicant.id).await.unwrap();
        assert_eq!(all_pending.len(), 2);
        assert_eq!(
            all_pending
                .iter()
                .find(|request| request["platform"] == "weibo")
                .unwrap()["external_id"],
            "99887766"
        );

        let duplicate = handle_message(
            &db,
            private("/ask xueqiu https://xueqiu.com/u/4514680565"),
            1_002,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(duplicate.text.contains("处理中"));
        for now in 1_003..1_006 {
            let _ = handle_message(&db, private("/ask xueqiu 4514680565"), now)
                .await
                .unwrap()
                .unwrap();
        }
        let limited = handle_message(&db, private("/ask xueqiu 4514680565"), 1_006)
            .await
            .unwrap()
            .unwrap();
        assert!(limited.text.contains("频繁"));

        let unauthorized = handle_callback(
            &db,
            callback_for(&format!("approve:{request_id}"), "42", "42"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(unauthorized.text.contains("管理员"));
        assert!(db.kol_request_pending(request_id).await.unwrap());

        let choose_category = handle_callback(
            &db,
            callback_for(&format!("approve:{request_id}"), "admin-chat", "admin-chat"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(choose_category.text.contains("请选择分类"));
        assert!(choose_category
            .keyboard
            .as_ref()
            .unwrap()
            .iter()
            .flatten()
            .any(|button| button.callback_data == format!("apcat:{request_id}:{category}")));
        assert!(choose_category
            .keyboard
            .as_ref()
            .unwrap()
            .iter()
            .flatten()
            .any(|button| button.callback_data == format!("apcat:{request_id}:0")));

        let approved = handle_callback(
            &db,
            callback_for(
                &format!("apcat:{request_id}:{category}"),
                "admin-chat",
                "admin-chat",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(approved.text.contains("已批准"));
        assert_eq!(approved.notifications.len(), 1);
        assert_eq!(approved.notifications[0].chat_id, "42");
        assert!(approved.notifications[0].text.contains("自动订阅"));
        assert!(!db.kol_request_pending(request_id).await.unwrap());
        let subscriptions = db.my_subscriptions(applicant.id).await.unwrap();
        assert_eq!(subscriptions.len(), 1);
        let approved_category: Option<i64> = sqlx::query_scalar(
            "SELECT category_id FROM kols WHERE id = (SELECT kol_id FROM subscriptions WHERE user_id = ?)",
        )
        .bind(applicant.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(approved_category, Some(category));

        let repeated = handle_callback(
            &db,
            callback_for(&format!("apcat:{request_id}:0"), "admin-chat", "admin-chat"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(repeated.text.contains("已处理"));
        assert_eq!(db.my_subscriptions(applicant.id).await.unwrap().len(), 1);
        let logs = db.list_admin_logs(20).await.unwrap();
        assert!(logs
            .iter()
            .any(|log| { log["user_id"] == admin.id && log["action"] == "approve_kol_request" }));

        let reject_applicant = handle_message(
            &db,
            TelegramMessage {
                chat_id: "43".into(),
                text: Some("/ask weibo 123456".into()),
                ..private("/help")
            },
            2_000,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(reject_applicant.text.contains("申请已提交"));
        let reject_user = db.user_by_telegram_chat_id("43").await.unwrap().unwrap();
        let rejected_id = db
            .list_kol_requests("pending", reject_user.id)
            .await
            .unwrap()[0]["id"]
            .as_i64()
            .unwrap();
        let rejected = handle_callback(
            &db,
            callback_for(&format!("reject:{rejected_id}"), "admin-chat", "admin-chat"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(rejected.text.contains("已拒绝"));
        assert_eq!(rejected.notifications[0].chat_id, "43");
        assert_eq!(
            db.list_kol_requests("rejected", reject_user.id)
                .await
                .unwrap()[0]["id"],
            rejected_id
        );
        let rejected_again = handle_callback(
            &db,
            callback_for(&format!("reject:{rejected_id}"), "admin-chat", "admin-chat"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(rejected_again.text.contains("已处理"));
    }

    #[tokio::test]
    async fn duplicate_telegram_chat_owners_fail_closed_for_admin_callbacks() {
        let db = db().await;
        db.ensure_admin("hash").await.unwrap();
        let applicant = db
            .get_or_create_telegram_user("applicant", "Applicant", true)
            .await
            .unwrap()
            .unwrap();
        let request = db
            .add_kol_request_without_category("xueqiu", "777", applicant.id, "")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin, telegram_chat_id)
             VALUES ('duplicate-admin', 'x', 1, 'duplicate-chat'),
                    ('duplicate-reader', 'x', 0, 'duplicate-chat')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        assert!(db
            .user_by_telegram_chat_id("duplicate-chat")
            .await
            .unwrap()
            .is_none());
        assert!(db
            .get_or_create_telegram_user("duplicate-chat", "Duplicate", true)
            .await
            .is_err());
        let callback = handle_callback(
            &db,
            callback_for(
                &format!("approve:{request}"),
                "duplicate-chat",
                "duplicate-chat",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(callback.text.contains("身份无效"));
        assert!(db.kol_request_pending(request).await.unwrap());
        assert!(db
            .list_kol_requests("pending", applicant.id)
            .await
            .unwrap()
            .iter()
            .any(|item| item["id"] == request));
    }
}
