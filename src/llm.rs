use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde_json::{json, Value};

const MAX_BODY: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub base: String,
    pub key: String,
    pub model: String,
    pub format: String,
    pub user_supplied: bool,
}

#[derive(Debug)]
pub enum LlmError {
    Bad(&'static str),
    Failed,
}

#[derive(Debug)]
pub struct Exchange {
    pub method: &'static str,
    pub url: String,
    pub host: String,
    pub pin: Option<IpAddr>,
    pub body: Option<Vec<u8>>,
}

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub fn normalize_format(value: &str) -> String {
    let raw = value.trim().to_ascii_lowercase().replace('_', "-");
    if raw == "responses" || raw == "openai-responses" {
        "responses".into()
    } else {
        "chat".into()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn runtime(
    base: Option<&str>,
    key: Option<&str>,
    saved_base: &str,
    saved_key: &str,
    model: Option<&str>,
    saved_model: &str,
    format: Option<&str>,
    saved_format: &str,
    is_admin: bool,
) -> Result<Config, LlmError> {
    let base = base.unwrap_or(saved_base).trim().to_string();
    let mut key = key.unwrap_or("").trim().to_string();
    if key.is_empty() || key.contains('…') {
        key = saved_key.trim().to_string();
    }
    if base.is_empty() || key.is_empty() {
        return Err(LlmError::Bad("请先填写 API 地址和 Key"));
    }
    let model = model.unwrap_or(saved_model).trim().to_string();
    let format = normalize_format(format.unwrap_or(saved_format));
    Ok(Config {
        base,
        key,
        model,
        format,
        user_supplied: !is_admin,
    })
}

pub fn prepare(
    cfg: &Config,
    suffix: &str,
    method: &'static str,
    body: Option<Vec<u8>>,
    resolve: impl Fn(&str) -> Vec<IpAddr>,
) -> Result<Exchange, LlmError> {
    let url = format!(
        "{}/{}",
        cfg.base.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    );
    let parts = parse_base(&url)?;
    if cfg.user_supplied {
        let pin = if let Ok(ip) = parts.host.parse::<IpAddr>() {
            if !crate::img_proxy::ip_allowed(ip) {
                return Err(LlmError::Bad("LLM 地址须为 http(s) URL"));
            }
            None
        } else {
            let mut ips = resolve(&parts.host);
            if !crate::img_proxy::resolution_ok(&ips) {
                return Err(LlmError::Bad("LLM 地址须为 http(s) URL"));
            }
            ips.sort_by_key(|ip| ip.to_string());
            Some(ips[0])
        };
        Ok(Exchange {
            method,
            url,
            host: parts.host,
            pin,
            body,
        })
    } else {
        Ok(Exchange {
            method,
            url,
            host: parts.host,
            pin: None,
            body,
        })
    }
}

pub fn models_from(reply: &Reply) -> Result<Vec<String>, LlmError> {
    if !(200..300).contains(&reply.status) {
        return Err(LlmError::Failed);
    }
    let payload: Value = serde_json::from_slice(&reply.body).map_err(|_| LlmError::Failed)?;
    let rows = payload.get("data").unwrap_or(&payload);
    let Some(rows) = rows.as_array() else {
        return Err(LlmError::Failed);
    };
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        let item = if let Some(text) = row.as_str() {
            text.trim().to_string()
        } else {
            row.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        if !item.is_empty() {
            seen.insert(item);
        }
    }
    Ok(seen.into_iter().collect())
}

pub fn probe_body(cfg: &Config) -> Vec<u8> {
    let model = if cfg.model.is_empty() {
        "gpt-4o-mini"
    } else {
        cfg.model.as_str()
    };
    let payload = if cfg.format == "responses" {
        json!({
            "model": model,
            "input": [{"role": "user", "content": "Reply with exactly: PONG"}],
            "temperature": 0,
            "max_output_tokens": 16,
        })
    } else {
        json!({
            "model": model,
            "messages": [{"role": "user", "content": "Reply with exactly: PONG"}],
            "temperature": 0,
            "max_tokens": 16,
        })
    };
    serde_json::to_vec(&payload).unwrap_or_default()
}

pub async fn complete(cfg: &Config, prompt: &str) -> Result<String, String> {
    let model = if cfg.model.is_empty() {
        "gpt-4o-mini"
    } else {
        cfg.model.as_str()
    };
    let payload = if cfg.format == "responses" {
        json!({"model": model, "input": [{"role": "user", "content": prompt}], "temperature": 0})
    } else {
        json!({"model": model, "messages": [{"role": "user", "content": prompt}], "temperature": 0})
    };
    let body = serde_json::to_vec(&payload).map_err(|_| "请求失败".to_string())?;
    let exchange = prepare(cfg, probe_suffix(cfg), "POST", Some(body), resolve_host)
        .map_err(|_| "LLM 地址不可用".to_string())?;
    let reply = send(&exchange, &cfg.key)
        .await
        .map_err(|_| "LLM 无响应".to_string())?;
    if !(200..300).contains(&reply.status) {
        return Err("LLM 无响应".into());
    }
    let payload =
        serde_json::from_slice::<Value>(&reply.body).map_err(|_| "LLM 无响应".to_string())?;
    let text = completion_text(&payload, &cfg.format);
    if text.trim().is_empty() {
        Err("LLM 无响应".into())
    } else {
        Ok(text)
    }
}

pub fn probe_suffix(cfg: &Config) -> &'static str {
    if cfg.format == "responses" {
        "responses"
    } else {
        "chat/completions"
    }
}

pub fn probe_result(reply: &Reply, cfg: &Config, latency_ms: u128) -> Value {
    let failed = json!({
        "ok": false,
        "latency_ms": latency_ms,
        "error": "无响应或地址/Key/模型不正确",
    });
    if !(200..300).contains(&reply.status) {
        return failed;
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&reply.body) else {
        return failed;
    };
    let text = completion_text(&payload, &cfg.format);
    if text.is_empty() {
        return failed;
    }
    let mut result = json!({
        "ok": true,
        "latency_ms": latency_ms,
        "format": cfg.format,
        "model": cfg.model,
    });
    if let Some(total) = usage_total(&payload) {
        result["usage"] = json!({"total_tokens": total});
    }
    result
}

pub fn resolve_host(host: &str) -> Vec<std::net::IpAddr> {
    crate::img_proxy::resolve(host)
}

pub async fn send(exchange: &Exchange, key: &str) -> Result<Reply, LlmError> {
    let mut builder = wreq::Client::builder()
        .redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(20));
    if let Some(ip) = exchange.pin {
        builder = builder.resolve(&exchange.host, SocketAddr::new(ip, 0));
    }
    let client = builder.build().map_err(|_| LlmError::Failed)?;
    let method = if exchange.method == "POST" {
        wreq::Method::POST
    } else {
        wreq::Method::GET
    };
    let mut request = client
        .request(method, &exchange.url)
        .header("Authorization", format!("Bearer {key}"));
    if let Some(body) = &exchange.body {
        request = request
            .header("Content-Type", "application/json")
            .body(body.clone());
    }
    let mut response = request.send().await.map_err(|_| LlmError::Failed)?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(LlmError::Failed);
    }
    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|len| len > MAX_BODY)
    {
        return Err(LlmError::Failed);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| LlmError::Failed)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(LlmError::Failed);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Reply { status, body })
}

struct Parts {
    host: String,
}

fn parse_base(url: &str) -> Result<Parts, LlmError> {
    let bad = LlmError::Bad("LLM 地址须为 http(s) URL");
    if url.len() > 2048 {
        return Err(bad);
    }
    let lower = url.to_ascii_lowercase();
    let rest = if let Some(rest) = lower
        .strip_prefix("https://")
        .map(|_| &url["https://".len()..])
    {
        rest
    } else if lower.starts_with("http://") {
        &url["http://".len()..]
    } else {
        return Err(bad);
    };
    if rest.contains('@') || rest.is_empty() {
        return Err(bad);
    }
    let authority = rest
        .split_once(['/', '?', '#'])
        .map(|(host, _)| host)
        .unwrap_or(rest);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, _)) = rest.split_once(']') else {
            return Err(bad);
        };
        host
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => authority,
        }
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || host == "localhost" {
        return Err(bad);
    }
    Ok(Parts { host })
}

fn completion_text(payload: &Value, format: &str) -> String {
    if format == "responses" {
        let direct = payload
            .get("output_text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !direct.is_empty() {
            return direct.to_string();
        }
        for item in payload
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let content = item.get("content");
            if let Some(blocks) = content.and_then(Value::as_array) {
                for block in blocks {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    if !text.is_empty() {
                        return text.to_string();
                    }
                }
            } else if let Some(text) = content.and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    return text.trim().to_string();
                }
            }
        }
    }
    let message = payload
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let content = message.get("content");
    let text = if let Some(blocks) = content.and_then(Value::as_array) {
        blocks
            .iter()
            .map(|block| {
                if let Some(text) = block.as_str() {
                    text.to_string()
                } else {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                }
            })
            .collect::<String>()
    } else {
        content.and_then(Value::as_str).unwrap_or("").to_string()
    };
    let text = text.trim();
    if !text.is_empty() {
        return text.to_string();
    }
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn usage_total(payload: &Value) -> Option<i64> {
    let usage = payload.get("usage")?.as_object()?;
    for key in ["total_tokens", "total_token_count"] {
        if let Some(value) = usage.get(key).and_then(Value::as_i64) {
            return Some(value);
        }
    }
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = input + output;
    if total == 0 {
        None
    } else {
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn cfg(base: &str, admin: bool) -> Config {
        Config {
            base: base.into(),
            key: "sk-test".into(),
            model: "gpt-test".into(),
            format: "chat".into(),
            user_supplied: !admin,
        }
    }

    #[test]
    fn users_cannot_point_llm_at_a_private_host() {
        let err = prepare(
            &cfg("http://127.0.0.1:11434/v1", false),
            "models",
            "GET",
            None,
            |_| vec![],
        )
        .unwrap_err();
        assert!(matches!(err, LlmError::Bad("LLM 地址须为 http(s) URL")));
        assert!(prepare(
            &cfg("https://user:pass@api.openai.com/v1", false),
            "models",
            "GET",
            None,
            |_| vec![]
        )
        .is_err());
        assert!(prepare(
            &cfg("https://evil.example/v1", false),
            "models",
            "GET",
            None,
            |_| { vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))] }
        )
        .is_err());
        let exchange = prepare(
            &cfg("https://api.openai.com/v1", false),
            "models",
            "GET",
            None,
            |_| vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
        )
        .unwrap();
        assert_eq!(exchange.url, "https://api.openai.com/v1/models");
        assert_eq!(exchange.host, "api.openai.com");
        assert_eq!(
            exchange.pin,
            Some(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)))
        );
        let admin = prepare(
            &cfg("http://127.0.0.1:11434/v1", true),
            "models",
            "GET",
            None,
            |_| panic!("admin does not resolve"),
        )
        .unwrap();
        assert!(admin.pin.is_none());
        assert_eq!(admin.url, "http://127.0.0.1:11434/v1/models");
    }

    #[test]
    fn lists_ids_and_reads_probe_usage() {
        assert!(matches!(
            runtime(Some(""), Some(""), "", "", None, "", None, "", false),
            Err(LlmError::Bad("请先填写 API 地址和 Key"))
        ));
        let cfg = runtime(
            Some("https://api.openai.com/v1"),
            Some("sk-……cret"),
            "https://saved",
            "sk-saved",
            Some("gpt-test"),
            "",
            Some("openai-responses"),
            "",
            false,
        )
        .unwrap();
        assert_eq!(cfg.key, "sk-saved");
        assert_eq!(cfg.format, "responses");
        let reply = Reply {
            status: 200,
            body: br#"{"data":[{"id":"gpt-4o"},{"id":"gpt-4o"},"gpt-4o-mini"]}"#.to_vec(),
        };
        assert_eq!(
            models_from(&reply).unwrap(),
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );
        let probe = Reply {
            status: 200,
            body: br#"{"output_text":"PONG","usage":{"input_tokens":4,"output_tokens":5}}"#
                .to_vec(),
        };
        let result = probe_result(&probe, &cfg, 12);
        assert_eq!(result["ok"], true);
        assert_eq!(result["usage"]["total_tokens"], 9);
        assert_eq!(result["format"], "responses");
        assert_eq!(probe_suffix(&cfg), "responses");
    }

    #[tokio::test]
    async fn admin_lists_models_from_a_local_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|item| item == b"\r\n\r\n") {
                let n = tokio::io::AsyncReadExt::read(&mut sock, &mut buf)
                    .await
                    .unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let request = String::from_utf8_lossy(&request);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer sk-local"),
                "{request}"
            );
            let body = br#"{"data":[{"id":"local-model"}]}"#;
            let header = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut sock, header.as_bytes())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut sock, body)
                .await
                .unwrap();
        });
        let cfg = cfg(&format!("http://127.0.0.1:{port}/v1"), true);
        let exchange = prepare(&cfg, "models", "GET", None, |_| {
            panic!("admin does not resolve")
        })
        .unwrap();
        let reply = send(&exchange, "sk-local").await.unwrap();
        assert_eq!(
            models_from(&reply).unwrap(),
            vec!["local-model".to_string()]
        );
        server.await.unwrap();
    }
}
