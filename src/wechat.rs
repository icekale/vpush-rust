use crate::db::{Db, RegisterError};

const SESSION_URL: &str = "https://api.weixin.qq.com/sns/jscode2session";

#[derive(Debug)]
pub enum WechatFail {
    Config,
    Closed,
    NeedInvite,
    Bad(&'static str),
    Msg(String),
}

pub fn parse_session(text: &str) -> Result<String, WechatFail> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| WechatFail::Bad("微信登录失败"))?;
    let code = value
        .get("errcode")
        .and_then(|item| item.as_i64())
        .unwrap_or(0);
    if code != 0 {
        let msg = value
            .get("errmsg")
            .and_then(|item| item.as_str())
            .unwrap_or("微信登录失败");
        return Err(WechatFail::Msg(format!("微信登录失败: {msg}")));
    }
    let openid = value
        .get("openid")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim();
    if !valid_openid(openid) {
        return Err(WechatFail::Bad("微信登录失败: 未返回 openid"));
    }
    Ok(openid.to_string())
}

pub async fn exchange(code: &str, app_id: &str, secret: &str) -> Result<String, WechatFail> {
    if app_id.trim().is_empty() || secret.trim().is_empty() {
        return Err(WechatFail::Config);
    }
    if !valid_code(code) {
        return Err(WechatFail::Bad("微信登录失败"));
    }
    let url = format!(
        "{SESSION_URL}?appid={}&secret={}&js_code={}&grant_type=authorization_code",
        encode(app_id),
        encode(secret),
        encode(code)
    );
    let body = tokio::task::spawn_blocking(move || {
        ureq::get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .call()
            .map_err(|_| "微信登录失败".to_string())
            .and_then(|response| {
                response
                    .into_string()
                    .map_err(|_| "微信登录失败".to_string())
            })
    })
    .await
    .map_err(|_| WechatFail::Bad("微信登录失败"))?
    .map_err(WechatFail::Msg)?;
    parse_session(&body)
}

pub async fn account(
    db: &Db,
    allow_register: bool,
    openid: &str,
    invite: &str,
) -> Result<i64, WechatFail> {
    if !valid_openid(openid) {
        return Err(WechatFail::Bad("微信登录态无效"));
    }
    if let Some(user) = db
        .user_by_openid(openid)
        .await
        .map_err(|_| WechatFail::Bad("微信登录失败"))?
    {
        return Ok(user.id);
    }
    if !allow_register {
        return Err(WechatFail::Closed);
    }
    let invite = invite.trim();
    if invite.is_empty() {
        return Err(WechatFail::NeedInvite);
    }
    let stem: String = openid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(10)
        .collect();
    if stem.len() < 6 {
        return Err(WechatFail::Bad("微信登录态无效"));
    }
    for suffix in 0..20 {
        let username = if suffix == 0 {
            format!("wx_{stem}")
        } else {
            format!("wx_{stem}{suffix}")
        };
        match db.register_wechat(invite, &username, openid).await {
            Ok(id) => return Ok(id),
            Err(RegisterError::Rejected("用户名已存在")) => continue,
            Err(RegisterError::Rejected("微信账号已存在")) => {
                let user = db
                    .user_by_openid(openid)
                    .await
                    .map_err(|_| WechatFail::Bad("微信登录失败"))?;
                return user
                    .map(|item| item.id)
                    .ok_or(WechatFail::Bad("微信登录失败"));
            }
            Err(RegisterError::Rejected(msg)) => return Err(WechatFail::Bad(msg)),
            Err(RegisterError::Db(_)) => return Err(WechatFail::Bad("微信登录失败")),
        }
    }
    Err(WechatFail::Bad("微信登录失败"))
}

fn valid_code(code: &str) -> bool {
    (1..=128).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_openid(openid: &str) -> bool {
    (8..=64).contains(&openid.len())
        && openid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_openid_and_rejects_wechat_errors() {
        assert_eq!(
            parse_session(r#"{"openid":"oABC1234567890","session_key":"secret"}"#).unwrap(),
            "oABC1234567890"
        );
        assert!(parse_session(r#"{"errcode":40029,"errmsg":"invalid code"}"#).is_err());
        assert!(parse_session(r#"{"openid":""}"#).is_err());
    }

    #[tokio::test]
    async fn first_login_uses_invite_and_second_does_not() {
        let path = std::env::temp_dir().join(format!(
            "vpush-wx-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO register_codes (code) VALUES ('WX1234')")
            .execute(db.pool())
            .await
            .unwrap();
        let id = account(&db, true, "oABC1234567890", "wx1234")
            .await
            .unwrap();
        let user = db.user_by_id(id).await.unwrap().unwrap();
        assert_eq!(user.username, "wx_oABC123456");
        assert!(user.password_hash.is_empty());
        assert_eq!(user.wechat_openid, "oABC1234567890");
        let again = account(&db, false, "oABC1234567890", "").await.unwrap();
        assert_eq!(again, id);
        assert!(account(&db, true, "oZZZ1234567890", "").await.is_err());
        let _ = std::fs::remove_file(path);
    }
}
