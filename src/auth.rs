use base64::Engine;
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;
use subtle::ConstantTimeEq;

const PBKDF2_ITERS: u32 = 200_000;
pub const TOKEN_TTL_SECS: u64 = 30 * 24 * 3600;
pub const USERNAME_MIN: usize = 6;
pub const USERNAME_MAX: usize = 30;
pub const PASSWORD_MIN: usize = 10;
pub const PASSWORD_MAX: usize = 128;
pub const USERNAME_MSG: &str = "用户名仅限中文、字母、数字、下划线和连字符，须以中文或字母开头";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Claims {
    pub uid: i64,
    pub name: String,
    pub ver: i64,
    pub exp: i64,
    pub created_at: String,
}

pub fn hash_password(password: &str, salt_hex: Option<&str>) -> String {
    let salt_hex = salt_hex.map(str::to_string).unwrap_or_else(|| {
        let mut salt = [0u8; 16];
        getrandom::getrandom(&mut salt).expect("os rng");
        hex::encode(salt)
    });
    let salt = hex::decode(&salt_hex).unwrap_or_default();
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ITERS, &mut out);
    format!("{salt_hex}${}", hex::encode(out))
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let Some((salt, _)) = stored.split_once('$') else {
        return false;
    };
    let got = hash_password(password, Some(salt));
    bool::from(got.as_bytes().ct_eq(stored.as_bytes()))
}

pub fn validate_username(username: &str) -> Result<String, &'static str> {
    let name = username.trim();
    if name.chars().count() < USERNAME_MIN {
        return Err("用户名至少6位");
    }
    if name.chars().count() > USERNAME_MAX {
        return Err("用户名最长30位");
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(USERNAME_MSG);
    };
    if !first.is_ascii_alphabetic() && !is_cjk(first) {
        return Err(USERNAME_MSG);
    }
    if !chars.all(is_username_rest) {
        return Err(USERNAME_MSG);
    }
    Ok(name.to_string())
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

fn is_username_rest(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || is_cjk(c)
}

pub fn create_token(
    uid: i64,
    name: &str,
    secret: &str,
    ver: i64,
    now: u64,
    created_at: &str,
) -> String {
    let claims = Claims {
        uid,
        name: name.to_string(),
        ver,
        exp: (now + TOKEN_TTL_SECS) as i64,
        created_at: created_at.to_string(),
    };
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).expect("claims"));
    let sig = sign(secret, &body);
    format!("{body}.{sig}")
}

pub fn verify_token(token: &str, secret: &str, now: u64) -> Option<Claims> {
    let (body, sig) = token.split_once('.')?;
    let expected = sign(secret, body);
    if !bool::from(expected.as_bytes().ct_eq(sig.as_bytes())) {
        return None;
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .ok()?;
    let claims: Claims = serde_json::from_slice(&raw).ok()?;
    if claims.exp < now as i64 {
        return None;
    }
    Some(claims)
}

fn sign(secret: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn random_secret() -> String {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("os rng");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let stored = hash_password("password1234", None);
        assert!(verify_password("password1234", &stored));
        assert!(!verify_password("password1235", &stored));
        assert!(!verify_password("password1234", "nope"));
    }

    #[test]
    fn token_roundtrip() {
        let token = create_token(7, "admin", "secret", 2, 1_000, "2020-01-01 00:00:00");
        let claims = verify_token(&token, "secret", 1_000).unwrap();
        assert_eq!(claims.uid, 7);
        assert_eq!(claims.name, "admin");
        assert_eq!(claims.ver, 2);
        assert_eq!(claims.created_at, "2020-01-01 00:00:00");
        assert!(verify_token(&token, "other", 1_000).is_none());
        assert!(verify_token(&token, "secret", claims.exp as u64 + 1).is_none());
        let legacy = format!(
            "{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(br#"{"uid":7,"name":"admin","ver":2,"exp":99999}"#),
            ""
        );
        let (body, _) = legacy.split_once('.').unwrap();
        let signed = format!("{body}.{}", sign("secret", body));
        assert!(verify_token(&signed, "secret", 1_000).is_none());
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("abcde").is_err());
        assert!(validate_username("abc def").is_err());
        assert_eq!(validate_username("abcdef").unwrap(), "abcdef");
        assert_eq!(validate_username(" 中文用户名字 ").unwrap(), "中文用户名字");
    }
}
