use std::future::Future;

use serde_json::{json, Value};

use crate::db::Db;
use crate::push::parse_telegram_response;
use crate::telegram_bot::{
    handle_callback, handle_message, TelegramButton, TelegramCallback, TelegramChatType,
    TelegramMessage, TelegramResponse,
};

const MAX_TEXT_CHARS: usize = 4096;
const MAX_CALLBACK_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotUpdate {
    Message {
        update_id: i64,
        message: TelegramMessage,
    },
    Callback {
        update_id: i64,
        callback: TelegramCallback,
    },
}

pub fn parse_update(value: &Value) -> Result<Option<BotUpdate>, String> {
    let Some(update_id) = value.get("update_id").and_then(Value::as_i64) else {
        return Ok(None);
    };
    if update_id < 0 {
        return Ok(None);
    }

    if let Some(message) = value.get("message") {
        return Ok(parse_message(update_id, message));
    }
    if let Some(callback) = value.get("callback_query") {
        return Ok(parse_callback(update_id, callback));
    }
    Ok(None)
}

fn parse_message(update_id: i64, value: &Value) -> Option<BotUpdate> {
    let chat = value.get("chat")?;
    if chat.get("type").and_then(Value::as_str) != Some("private") {
        return None;
    }
    let chat_id = id_string(chat.get("id")?)?;
    let text = value.get("text").and_then(Value::as_str)?;
    let display_name = private_display_name(chat).unwrap_or_else(|| "Telegram 用户".to_owned());
    Some(BotUpdate::Message {
        update_id,
        message: TelegramMessage {
            chat_id,
            chat_type: TelegramChatType::Private,
            display_name,
            text: Some(text.to_owned()),
        },
    })
}

fn parse_callback(update_id: i64, value: &Value) -> Option<BotUpdate> {
    let callback_id = value.get("id").and_then(Value::as_str)?.trim();
    let data = value.get("data").and_then(Value::as_str)?.to_owned();
    let actor_id = id_string(value.get("from")?.get("id")?)?;
    let message = value.get("message")?;
    let chat = message.get("chat")?;
    let chat_type = chat_type(chat.get("type")?.as_str()?)?;
    if chat_type != TelegramChatType::Private || callback_id.is_empty() {
        return None;
    }
    let message_chat_id = id_string(chat.get("id")?)?;
    let message_id = message.get("message_id").and_then(Value::as_i64)?;
    Some(BotUpdate::Callback {
        update_id,
        callback: TelegramCallback {
            id: callback_id.to_owned(),
            chat_id: actor_id,
            message_chat_id,
            chat_type,
            message_id,
            data,
        },
    })
}

fn id_string(value: &Value) -> Option<String> {
    if let Some(id) = value.as_i64() {
        return (id >= 0).then(|| id.to_string());
    }
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
}

fn chat_type(value: &str) -> Option<TelegramChatType> {
    match value {
        "private" => Some(TelegramChatType::Private),
        "group" => Some(TelegramChatType::Group),
        "supergroup" => Some(TelegramChatType::Supergroup),
        "channel" => Some(TelegramChatType::Channel),
        _ => None,
    }
}

fn private_display_name(chat: &Value) -> Option<String> {
    let first = chat.get("first_name").and_then(Value::as_str).unwrap_or("");
    let last = chat.get("last_name").and_then(Value::as_str).unwrap_or("");
    let username = chat.get("username").and_then(Value::as_str).unwrap_or("");
    let name = format!("{first} {last}").trim().to_owned();
    if !name.is_empty() {
        Some(name)
    } else if !username.is_empty() {
        Some(username.to_owned())
    } else {
        None
    }
}

pub fn send_message_payload(chat_id: &str, response: &TelegramResponse) -> Value {
    let mut body = json!({
        "chat_id": chat_id,
        "text": bounded_text(&response.text),
        "disable_web_page_preview": true,
    });
    add_keyboard(&mut body, response.keyboard.as_ref());
    body
}

pub fn edit_message_payload(chat_id: &str, message_id: i64, response: &TelegramResponse) -> Value {
    let mut body = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": bounded_text(&response.text),
    });
    add_keyboard(&mut body, response.keyboard.as_ref());
    body
}

pub fn answer_callback_payload(callback_id: &str) -> Value {
    json!({ "callback_query_id": callback_id })
}

fn add_keyboard(body: &mut Value, keyboard: Option<&Vec<Vec<TelegramButton>>>) {
    let Some(keyboard) = keyboard else {
        return;
    };
    let rows: Vec<Vec<Value>> = keyboard
        .iter()
        .map(|row| {
            row.iter()
                .filter(|button| button.callback_data.len() <= MAX_CALLBACK_BYTES)
                .map(|button| {
                    json!({
                        "text": button.text,
                        "callback_data": button.callback_data,
                    })
                })
                .collect()
        })
        .filter(|row: &Vec<Value>| !row.is_empty())
        .collect();
    if !rows.is_empty() {
        body["reply_markup"] = json!({ "inline_keyboard": rows });
    }
}

fn bounded_text(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

pub async fn dispatch<F, Fut>(
    db: &Db,
    update: BotUpdate,
    now: i64,
    mut transport: F,
) -> Result<(), String>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<(u16, String), String>>,
{
    match update {
        BotUpdate::Message { message, .. } => {
            if let Some(response) = handle_message(db, message.clone(), now).await? {
                send_response(&mut transport, &message.chat_id, &response).await?;
            }
        }
        BotUpdate::Callback { callback, .. } => {
            call(
                &mut transport,
                "answerCallbackQuery",
                answer_callback_payload(&callback.id),
            )
            .await?;
            if let Some(response) = handle_callback(db, callback.clone()).await? {
                if let Some(target) = response.edit_message.as_ref() {
                    call(
                        &mut transport,
                        "editMessageText",
                        edit_message_payload(&target.chat_id, target.message_id, &response),
                    )
                    .await?;
                    send_notifications(&mut transport, &response).await?;
                } else {
                    send_response(&mut transport, &callback.message_chat_id, &response).await?;
                };
            }
        }
    }
    Ok(())
}

async fn send_response<F, Fut>(
    transport: &mut F,
    chat_id: &str,
    response: &TelegramResponse,
) -> Result<(), String>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<(u16, String), String>>,
{
    call(
        transport,
        "sendMessage",
        send_message_payload(chat_id, response),
    )
    .await?;
    send_notifications(transport, response).await
}

async fn send_notifications<F, Fut>(
    transport: &mut F,
    response: &TelegramResponse,
) -> Result<(), String>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<(u16, String), String>>,
{
    for notification in &response.notifications {
        let notification_response = TelegramResponse {
            text: notification.text.clone(),
            keyboard: notification.keyboard.clone(),
            notifications: Vec::new(),
            answer_callback_query_id: None,
            edit_message: None,
        };
        call(
            transport,
            "sendMessage",
            send_message_payload(&notification.chat_id, &notification_response),
        )
        .await?;
    }
    Ok(())
}

async fn call<F, Fut>(transport: &mut F, method: &str, body: Value) -> Result<(), String>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<(u16, String), String>>,
{
    let (status, response) = transport(method.to_owned(), body).await?;
    parse_telegram_response(status, &response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    fn message_update() -> Value {
        json!({
            "update_id": 17,
            "message": {
                "message_id": 3,
                "from": {"id": 999, "first_name": "Untrusted"},
                "chat": {"id": 42, "type": "private", "first_name": "Ada", "last_name": "Lovelace"},
                "text": "/help"
            }
        })
    }

    fn callback_update(data: &str, actor: i64, chat: i64) -> Value {
        json!({
            "update_id": 18,
            "callback_query": {
                "id": "callback-1",
                "from": {"id": actor},
                "message": {"message_id": 7, "chat": {"id": chat, "type": "private"}, "text": "old"},
                "data": data
            }
        })
    }

    #[test]
    fn parses_private_text_without_trusting_nested_sender_and_ignores_groups() {
        let parsed = parse_update(&message_update()).unwrap().unwrap();
        assert_eq!(
            parsed,
            BotUpdate::Message {
                update_id: 17,
                message: TelegramMessage {
                    chat_id: "42".into(),
                    chat_type: TelegramChatType::Private,
                    display_name: "Ada Lovelace".into(),
                    text: Some("/help".into()),
                },
            }
        );
        let group = json!({"update_id": 19, "message": {"chat": {"id": -4, "type": "group"}, "text": "/help"}});
        assert_eq!(parse_update(&group).unwrap(), None);
        assert_eq!(parse_update(&json!({"update_id": -1})).unwrap(), None);
    }

    #[test]
    fn parses_callback_actor_and_message_chat_separately() {
        let parsed = parse_update(&callback_update("mysubs", 42, 42))
            .unwrap()
            .unwrap();
        assert_eq!(
            parsed,
            BotUpdate::Callback {
                update_id: 18,
                callback: TelegramCallback {
                    id: "callback-1".into(),
                    chat_id: "42".into(),
                    message_chat_id: "42".into(),
                    chat_type: TelegramChatType::Private,
                    message_id: 7,
                    data: "mysubs".into(),
                },
            }
        );
        let mismatch = parse_update(&callback_update("mysubs", 99, 42))
            .unwrap()
            .unwrap();
        let BotUpdate::Callback { callback, .. } = mismatch else {
            panic!("expected callback");
        };
        assert_eq!(callback.chat_id, "99");
        assert_eq!(callback.message_chat_id, "42");
    }

    #[test]
    fn serializes_keyboard_targets_and_bounds_text_and_callback_data() {
        let response = TelegramResponse {
            text: "x".repeat(5000),
            keyboard: Some(vec![vec![
                TelegramButton {
                    text: "ok".into(),
                    callback_data: "list:1".into(),
                },
                TelegramButton {
                    text: "bad".into(),
                    callback_data: "x".repeat(65),
                },
            ]]),
            notifications: Vec::new(),
            answer_callback_query_id: None,
            edit_message: None,
        };
        let body = send_message_payload("42", &response);
        assert_eq!(body["text"].as_str().unwrap().chars().count(), 4096);
        assert_eq!(
            body["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
            "list:1"
        );
        assert!(body["reply_markup"]["inline_keyboard"][0][1].is_null());
        assert!(body.get("parse_mode").is_none());
        let edit = edit_message_payload("42", 7, &response);
        assert_eq!(edit["chat_id"], "42");
        assert_eq!(edit["message_id"], 7);
        assert_eq!(
            answer_callback_payload("callback-1")["callback_query_id"],
            "callback-1"
        );
    }

    #[tokio::test]
    async fn dispatch_calls_existing_handlers_and_always_answers_callbacks() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let recorded = calls.clone();
        let update = parse_update(&message_update()).unwrap().unwrap();
        dispatch(&db, update, 1_000, move |method, body| {
            recorded.lock().unwrap().push((method, body));
            async { Ok((200, r#"{"ok":true}"#.into())) }
        })
        .await
        .unwrap();
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "sendMessage");
            assert_eq!(calls[0].1["chat_id"], "42");
        }

        let source_id = db
            .add_kol("weibo", "source", "source", None, false, false, false)
            .await
            .unwrap();
        let callback = BotUpdate::Callback {
            update_id: 18,
            callback: TelegramCallback {
                id: "callback-1".into(),
                chat_id: "42".into(),
                message_chat_id: "42".into(),
                chat_type: TelegramChatType::Private,
                message_id: 7,
                data: format!("sub:{source_id}"),
            },
        };
        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let recorded = calls.clone();
        dispatch(&db, callback, 1_000, move |method, body| {
            recorded.lock().unwrap().push((method, body));
            async { Ok((200, r#"{"ok":true}"#.into())) }
        })
        .await
        .unwrap();
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls[0].0, "answerCallbackQuery");
            assert_eq!(calls[1].0, "editMessageText");
            assert_eq!(calls[1].1["chat_id"], "42");
            assert_eq!(calls[1].1["message_id"], 7);
        }
        assert!(db
            .my_subscriptions(1)
            .await
            .unwrap()
            .iter()
            .any(|item| item["id"] == source_id));
    }

    #[tokio::test]
    async fn dispatch_ignores_overlong_input() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        let update = json!({
            "update_id": 20,
            "message": {
                "chat": {"id": 42, "type": "private"},
                "text": "x".repeat(4097)
            }
        });
        let calls = Arc::new(Mutex::new(0));
        let recorded = calls.clone();
        dispatch(
            &db,
            parse_update(&update).unwrap().unwrap(),
            1_000,
            move |_method, _body| {
                *recorded.lock().unwrap() += 1;
                async { Ok((200, r#"{"ok":true}"#.into())) }
            },
        )
        .await
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 0);
    }
    #[tokio::test]
    async fn dispatch_rejects_api_ok_false_and_keeps_actor_chat_check() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        let mismatch = parse_update(&callback_update("help", 99, 42))
            .unwrap()
            .unwrap();
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = calls.clone();
        let error = dispatch(&db, mismatch, 1_000, move |method, _body| {
            recorded.lock().unwrap().push(method);
            async { Ok((200, r#"{"ok":false}"#.into())) }
        })
        .await
        .unwrap_err();
        assert!(error.contains("rejected"));
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.as_slice(), &["answerCallbackQuery"]);
        }
        assert!(handle_callback(
            &db,
            TelegramCallback {
                id: "callback-1".into(),
                chat_id: "99".into(),
                message_chat_id: "42".into(),
                chat_type: TelegramChatType::Private,
                message_id: 7,
                data: "help".into(),
            }
        )
        .await
        .unwrap()
        .is_none());
    }
}
