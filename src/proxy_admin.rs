use std::io::Read;

use serde_json::{json, Value};

use crate::db::Db;

const PLATFORMS: [&str; 7] = ["combination", "ima", "truth", "twitter", "weibo", "xueqiu", "zsxq"];
const ROUTES_KEY: &str = "proxy_routes";

#[derive(Debug)]
pub struct AdminError {
    pub status: u16,
    pub detail: String,
}

#[derive(Clone)]
struct Node {
    protocol: String,
    host: String,
    port: i64,
    username: String,
    password: String,
}

pub async fn pools(db: &Db) -> Result<Value, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT p.id, p.name, p.kind, p.extract_url, p.protocol, p.expire_seconds,
                p.refresh_interval_seconds, p.enabled, p.last_extract_at, p.last_error,
                (SELECT COUNT(*) FROM proxies x WHERE x.pool_id = p.id) AS proxy_count
         FROM proxy_pools p ORDER BY p.id",
    )
    .fetch_all(db.pool())
    .await?;
    let items: Vec<Value> = rows.iter().map(pool_json).collect();
    Ok(json!({"items": items}))
}

pub async fn create_pool(
    db: &Db,
    name: &str,
    kind: &str,
    protocol: &str,
    extract_url: &str,
    expire_seconds: i64,
    refresh_seconds: i64,
) -> Result<Value, AdminError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(bad("名称不能为空"));
    }
    check_pool(kind, protocol, extract_url)?;
    let result = sqlx::query(
        "INSERT INTO proxy_pools (name, kind, extract_url, protocol, expire_seconds, refresh_interval_seconds, enabled)
         VALUES (?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(name)
    .bind(kind)
    .bind(extract_url.trim())
    .bind(protocol)
    .bind(expire_seconds.max(0))
    .bind(refresh_seconds.max(0))
    .execute(db.pool())
    .await;
    let id = match result {
        Ok(done) => done.last_insert_rowid(),
        Err(err) if err.to_string().contains("UNIQUE") => return Err(bad("代理池名称已存在")),
        Err(_) => return Err(fail(500, "创建代理池失败")),
    };
    one_pool(db, id).await
}

pub async fn delete_pool(db: &Db, id: i64) -> Result<(), AdminError> {
    if one_pool(db, id).await.is_err() {
        return Err(missing("代理池不存在"));
    }
    let mut tx = db.pool().begin().await.map_err(|_| fail(500, "删除代理池失败"))?;
    sqlx::query("DELETE FROM proxies WHERE pool_id = ?").bind(id).execute(&mut *tx).await.map_err(|_| fail(500, "删除代理池失败"))?;
    sqlx::query("DELETE FROM proxy_pools WHERE id = ?").bind(id).execute(&mut *tx).await.map_err(|_| fail(500, "删除代理池失败"))?;
    tx.commit().await.map_err(|_| fail(500, "删除代理池失败"))
}

pub async fn import_text(db: &Db, pool_id: i64, text: &str, protocol: Option<&str>) -> Result<Value, AdminError> {
    let pool = one_pool(db, pool_id).await.map_err(|_| missing("代理池不存在"))?;
    let default = protocol.unwrap_or_else(|| pool["protocol"].as_str().unwrap_or("http"));
    if norm_protocol(default).is_empty() {
        return Err(bad("协议须为 http 或 socks5"));
    }
    let nodes = parse_lines(text, default);
    let mut ids = std::collections::BTreeSet::new();
    for node in nodes {
        ids.insert(upsert(db, pool_id, &node, "manual", None).await?);
    }
    Ok(json!({"imported": ids.len()}))
}

pub async fn extract_target(db: &Db, pool_id: i64) -> Result<String, AdminError> {
    let pool = one_pool(db, pool_id).await.map_err(|_| missing("代理池不存在"))?;
    if pool["kind"] != "extract" {
        return Err(bad("不是提取池"));
    }
    let url = pool["extract_url"].as_str().unwrap_or("").trim().to_string();
    check_extract_url(&url)?;
    Ok(url)
}

pub async fn store_extracted(db: &Db, pool_id: i64, body: &str, now: i64) -> Result<Value, AdminError> {
    let pool = one_pool(db, pool_id).await.map_err(|_| missing("代理池不存在"))?;
    let protocol = pool["protocol"].as_str().unwrap_or("http");
    let expire = pool["expire_seconds"].as_i64().unwrap_or(0);
    let expires_at = if expire > 0 { Some(now + expire) } else { None };
    let lines = parse_extract(body);
    let nodes = parse_lines(&lines.join("\n"), protocol);
    for node in &nodes {
        upsert(db, pool_id, node, "extract", expires_at).await?;
    }
    sqlx::query("UPDATE proxy_pools SET last_extract_at = ?, last_error = '' WHERE id = ?")
        .bind(now)
        .bind(pool_id)
        .execute(db.pool())
        .await
        .map_err(|_| fail(500, "保存提取结果失败"))?;
    Ok(json!({"imported": nodes.len(), "parsed": lines.len()}))
}

pub async fn refresh_due(
    db: &Db,
    now: i64,
    mut fetch: impl FnMut(&str) -> Result<String, String>,
) -> Result<usize, sqlx::Error> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT id, extract_url, refresh_interval_seconds, COALESCE(last_extract_at, 0) AS last_extract_at
         FROM proxy_pools
         WHERE enabled = 1 AND kind = 'extract' AND refresh_interval_seconds > 0
         ORDER BY id",
    )
    .fetch_all(db.pool())
    .await?;
    let mut refreshed = 0;
    for row in rows {
        let id: i64 = row.get("id");
        let interval: i64 = row.get("refresh_interval_seconds");
        let last: i64 = row.get("last_extract_at");
        if last > 0 && now.saturating_sub(last) < interval {
            continue;
        }
        let url: String = row.get("extract_url");
        match fetch(url.trim()) {
            Ok(body) => {
                if store_extracted(db, id, &body, now).await.is_ok() {
                    refreshed += 1;
                }
            }
            Err(err) => note_extract_error(db, id, &err).await,
        }
    }
    Ok(refreshed)
}

pub async fn note_extract_error(db: &Db, pool_id: i64, error: &str) {
    let error: String = error.chars().take(300).collect();
    let _ = sqlx::query("UPDATE proxy_pools SET last_error = ? WHERE id = ?")
        .bind(error)
        .bind(pool_id)
        .execute(db.pool())
        .await;
}

pub async fn proxies(db: &Db) -> Result<Value, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, pool_id, protocol, host, port, username, password, source, status, expires_at
         FROM proxies ORDER BY id DESC",
    )
    .fetch_all(db.pool())
    .await?;
    let items: Vec<Value> = rows.iter().map(proxy_json).collect();
    Ok(json!({"items": items}))
}

pub async fn delete_proxy(db: &Db, id: i64) -> Result<(), AdminError> {
    let done = sqlx::query("DELETE FROM proxies WHERE id = ?")
        .bind(id)
        .execute(db.pool())
        .await
        .map_err(|_| fail(500, "删除代理失败"))?;
    if done.rows_affected() == 0 {
        return Err(missing("代理不存在"));
    }
    Ok(())
}

pub async fn proxy_endpoint(db: &Db, id: i64) -> Result<String, AdminError> {
    let row = proxy_row(db, id).await?;
    Ok(endpoint(&row))
}

pub async fn finish_probe(db: &Db, id: i64, ok: bool, status_code: Option<u16>, error: &str) -> Result<Value, AdminError> {
    if proxy_row(db, id).await.is_err() {
        return Err(missing("代理不存在"));
    }
    let now = now_secs();
    if ok {
        sqlx::query("UPDATE proxies SET status = 'ok', fail_count = 0, last_ok_at = ?, last_error = '' WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(db.pool())
            .await
            .map_err(|_| fail(500, "记录测试结果失败"))?;
        return Ok(json!({"ok": true, "status_code": status_code.unwrap_or(204)}));
    }
    let error: String = error.chars().take(200).collect();
    sqlx::query("UPDATE proxies SET status = 'fail', fail_count = fail_count + 1, last_fail_at = ?, last_error = ? WHERE id = ?")
        .bind(now)
        .bind(&error)
        .bind(id)
        .execute(db.pool())
        .await
        .map_err(|_| fail(500, "记录测试结果失败"))?;
    let mut body = json!({"ok": false, "error": error});
    if let Some(code) = status_code {
        body["status_code"] = json!(code);
    }
    Ok(body)
}

#[derive(Clone, Debug)]
pub struct Exit {
    pub id: i64,
    pub url: String,
}

pub async fn acquire(db: &Db, platform: &str) -> Result<Option<Exit>, String> {
    let saved = routes(db).await.map_err(|err| err.to_string())?;
    let route = &saved[platform];
    let mode = route["mode"].as_str().unwrap_or("direct");
    let now = now_secs();
    if mode == "direct" {
        return Ok(None);
    }
    if mode == "proxy" {
        let id = route["proxy_id"].as_i64().ok_or("指定代理不存在")?;
        return Ok(Some(load_exit(db, id, now).await?));
    }
    let pool_id = route["pool_id"].as_i64().ok_or("代理池为空")?;
    let rows = usable(db, pool_id, now).await?;
    if rows.is_empty() {
        return Err("代理池为空".into());
    }
    let index = (now as usize).wrapping_mul(17) % rows.len();
    Ok(Some(rows[index].clone()))
}

pub async fn note(db: &Db, proxy_id: Option<i64>, ok: bool, error: &str) {
    let Some(id) = proxy_id else { return };
    let now = now_secs();
    if ok {
        let _ = sqlx::query("UPDATE proxies SET status = 'ok', fail_count = 0, last_ok_at = ?, last_error = '' WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(db.pool())
            .await;
        return;
    }
    let current = sqlx::query("SELECT fail_count, status FROM proxies WHERE id = ?")
        .bind(id)
        .fetch_optional(db.pool())
        .await;
    let Ok(Some(row)) = current else { return };
    use sqlx::Row;
    let fails = row.get::<i64, _>("fail_count") + 1;
    let status = if fails >= 3 { "dead".to_string() } else { row.get::<String, _>("status") };
    let error: String = error.chars().take(200).collect();
    let _ = sqlx::query("UPDATE proxies SET status = ?, fail_count = ?, last_fail_at = ?, last_error = ? WHERE id = ?")
        .bind(status)
        .bind(fails)
        .bind(now)
        .bind(error)
        .bind(id)
        .execute(db.pool())
        .await;
}

pub fn http_agent(proxy: Option<&str>, connect: std::time::Duration, read: std::time::Duration) -> Result<ureq::Agent, String> {
    let mut builder = ureq::AgentBuilder::new().timeout_connect(connect).timeout_read(read);
    if let Some(url) = proxy {
        builder = builder.proxy(ureq::Proxy::new(url).map_err(|err| err.to_string())?);
    }
    Ok(builder.build())
}

pub async fn routes(db: &Db) -> Result<Value, sqlx::Error> {
    let raw = db.setting(ROUTES_KEY).await?.unwrap_or_default();
    Ok(normalize(&serde_json::from_str(&raw).unwrap_or(Value::Null)))
}

pub async fn save_routes(db: &Db, body: &Value) -> Result<Value, AdminError> {
    let obj = body.as_object().ok_or_else(|| bad("路由格式无效"))?;
    for (platform, route) in obj {
        if !PLATFORMS.contains(&platform.as_str()) {
            return Err(bad(&format!("未知平台: {platform}")));
        }
        let route = route.as_object().ok_or_else(|| bad("路由格式无效"))?;
        let mode = route.get("mode").and_then(Value::as_str).unwrap_or("");
        if !matches!(mode, "direct" | "pool" | "proxy") {
            return Err(bad("mode 须为 direct / pool / proxy"));
        }
        if mode == "pool" {
            let id = route.get("pool_id").and_then(Value::as_i64).ok_or_else(|| bad("代理池不存在"))?;
            if one_pool(db, id).await.is_err() {
                return Err(bad("代理池不存在"));
            }
        }
        if mode == "proxy" {
            let id = route.get("proxy_id").and_then(Value::as_i64).ok_or_else(|| bad("指定代理不存在"))?;
            if proxy_row(db, id).await.is_err() {
                return Err(bad("指定代理不存在"));
            }
        }
    }
    let normalized = normalize(body);
    db.set_setting(ROUTES_KEY, &normalized.to_string()).await.map_err(|_| fail(500, "保存出口失败"))?;
    Ok(normalized)
}

pub fn live_get(url: &str) -> Result<String, String> {
    let response = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(20))
        .redirects(0)
        .build()
        .get(url)
        .call()
        .map_err(|err| err.to_string())?;
    if !(200..300).contains(&response.status()) {
        return Err(format!("提取 HTTP {}", response.status()));
    }
    let mut buf = Vec::new();
    response.into_reader().take(1_048_576).read_to_end(&mut buf).map_err(|err| err.to_string())?;
    String::from_utf8(buf).map_err(|_| "提取结果不是文本".to_string())
}

pub fn live_probe(proxy_url: &str) -> Result<u16, String> {
    let proxy = ureq::Proxy::new(proxy_url).map_err(|err| err.to_string())?;
    let response = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(8))
        .redirects(0)
        .build()
        .get("https://www.gstatic.com/generate_204")
        .call()
        .map_err(|err| match err {
            ureq::Error::Status(code, _) => format!("status:{code}"),
            other => other.to_string(),
        })?;
    Ok(response.status())
}

fn pool_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    use sqlx::Row;
    json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "kind": row.get::<String, _>("kind"),
        "extract_url": mask_url(&row.get::<String, _>("extract_url")),
        "extract_url_set": !row.get::<String, _>("extract_url").is_empty(),
        "protocol": row.get::<String, _>("protocol"),
        "expire_seconds": row.get::<i64, _>("expire_seconds"),
        "refresh_interval_seconds": row.get::<i64, _>("refresh_interval_seconds"),
        "enabled": row.get::<i64, _>("enabled"),
        "last_extract_at": row.try_get::<i64, _>("last_extract_at").ok(),
        "last_error": row.get::<String, _>("last_error"),
        "proxy_count": row.get::<i64, _>("proxy_count"),
    })
}

fn proxy_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    use sqlx::Row;
    let password = row.get::<String, _>("password");
    json!({
        "id": row.get::<i64, _>("id"),
        "pool_id": row.get::<i64, _>("pool_id"),
        "protocol": row.get::<String, _>("protocol"),
        "host": row.get::<String, _>("host"),
        "port": row.get::<i64, _>("port"),
        "username": row.get::<String, _>("username"),
        "password": if password.is_empty() { "" } else { "***" },
        "has_password": !password.is_empty(),
        "source": row.get::<String, _>("source"),
        "status": row.get::<String, _>("status"),
        "expires_at": row.try_get::<i64, _>("expires_at").ok(),
    })
}

async fn one_pool(db: &Db, id: i64) -> Result<Value, AdminError> {
    let row = sqlx::query(
        "SELECT p.id, p.name, p.kind, p.extract_url, p.protocol, p.expire_seconds,
                p.refresh_interval_seconds, p.enabled, p.last_extract_at, p.last_error,
                (SELECT COUNT(*) FROM proxies x WHERE x.pool_id = p.id) AS proxy_count
         FROM proxy_pools p WHERE p.id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await
    .map_err(|_| fail(500, "读取代理池失败"))?;
    row.as_ref().map(pool_json).ok_or_else(|| missing("代理池不存在"))
}

async fn load_exit(db: &Db, id: i64, now: i64) -> Result<Exit, String> {
    use sqlx::Row;
    let row = sqlx::query("SELECT id, protocol, host, port, username, password, expires_at FROM proxies WHERE id = ?")
        .bind(id)
        .fetch_optional(db.pool())
        .await
        .map_err(|err| err.to_string())?
        .ok_or("指定代理不存在")?;
    if expired(row.try_get("expires_at").ok(), now) {
        return Err("指定代理已过期".into());
    }
    Ok(exit_from(&row))
}

async fn usable(db: &Db, pool_id: i64, now: i64) -> Result<Vec<Exit>, String> {
    let rows = sqlx::query(
        "SELECT id, protocol, host, port, username, password, expires_at FROM proxies
         WHERE pool_id = ? AND status IN ('unknown', 'ok')
           AND (expires_at IS NULL OR expires_at = 0 OR expires_at > ?)
         ORDER BY id",
    )
    .bind(pool_id)
    .bind(now)
    .fetch_all(db.pool())
    .await
    .map_err(|err| err.to_string())?;
    Ok(rows.iter().map(exit_from).collect())
}

fn exit_from(row: &sqlx::sqlite::SqliteRow) -> Exit {
    use sqlx::Row;
    let node = Node {
        protocol: row.get("protocol"),
        host: row.get("host"),
        port: row.get("port"),
        username: row.get("username"),
        password: row.get("password"),
    };
    Exit { id: row.get("id"), url: endpoint(&node) }
}

fn expired(expires_at: Option<i64>, now: i64) -> bool {
    matches!(expires_at, Some(at) if at > 0 && at <= now)
}

async fn proxy_row(db: &Db, id: i64) -> Result<Node, AdminError> {
    use sqlx::Row;
    let row = sqlx::query("SELECT protocol, host, port, username, password FROM proxies WHERE id = ?")
        .bind(id)
        .fetch_optional(db.pool())
        .await
        .map_err(|_| fail(500, "读取代理失败"))?
        .ok_or_else(|| missing("代理不存在"))?;
    Ok(Node {
        protocol: row.get("protocol"),
        host: row.get("host"),
        port: row.get("port"),
        username: row.get("username"),
        password: row.get("password"),
    })
}

async fn upsert(db: &Db, pool_id: i64, node: &Node, source: &str, expires_at: Option<i64>) -> Result<i64, AdminError> {
    let row = sqlx::query(
        "INSERT INTO proxies (pool_id, protocol, host, port, username, password, source, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(pool_id, protocol, host, port, username)
         DO UPDATE SET password = excluded.password, source = excluded.source, expires_at = excluded.expires_at
         RETURNING id",
    )
    .bind(pool_id)
    .bind(&node.protocol)
    .bind(&node.host)
    .bind(node.port)
    .bind(&node.username)
    .bind(&node.password)
    .bind(source)
    .bind(expires_at)
    .fetch_one(db.pool())
    .await
    .map_err(|_| fail(500, "保存代理失败"))?;
    use sqlx::Row;
    Ok(row.get("id"))
}

fn endpoint(node: &Node) -> String {
    let scheme = if node.protocol == "socks5" { "socks5" } else { "http" };
    if node.username.is_empty() && node.password.is_empty() {
        format!("{scheme}://{}:{}", node.host, node.port)
    } else {
        format!(
            "{scheme}://{}:{}@{}:{}",
            encode(&node.username),
            encode(&node.password),
            node.host,
            node.port
        )
    }
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn normalize(raw: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for platform in PLATFORMS {
        out.insert(platform.to_string(), json!({"mode": "direct"}));
    }
    let Some(obj) = raw.as_object() else { return Value::Object(out) };
    for platform in PLATFORMS {
        let Some(route) = obj.get(platform).and_then(Value::as_object) else { continue };
        let mode = route.get("mode").and_then(Value::as_str).unwrap_or("direct");
        let item = match mode {
            "pool" => route.get("pool_id").and_then(Value::as_i64).map(|id| json!({"mode": "pool", "pool_id": id})),
            "proxy" => route.get("proxy_id").and_then(Value::as_i64).map(|id| json!({"mode": "proxy", "proxy_id": id})),
            "direct" => Some(json!({"mode": "direct"})),
            _ => None,
        };
        if let Some(item) = item {
            out.insert(platform.to_string(), item);
        }
    }
    Value::Object(out)
}

fn parse_extract(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let Ok(data) = serde_json::from_str::<Value>(text) else {
        return plain_lines(text);
    };
    match data {
        Value::Array(items) => from_list(&items),
        Value::Object(obj) => {
            for key in ["data", "list", "proxies"] {
                if let Some(Value::Array(items)) = obj.get(key) {
                    return from_list(items);
                }
            }
            obj_line(&Value::Object(obj)).into_iter().collect()
        }
        Value::String(text) => plain_lines(&text),
        _ => Vec::new(),
    }
}

fn from_list(items: &[Value]) -> Vec<String> {
    items.iter().filter_map(|item| match item {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Object(_) => obj_line(item),
        _ => None,
    }).collect()
}

fn obj_line(item: &Value) -> Option<String> {
    let host = item.get("ip").or_else(|| item.get("host")).or_else(|| item.get("addr")).and_then(Value::as_str)?;
    let port = item.get("port").and_then(|value| value.as_i64().or_else(|| value.as_str().and_then(|text| text.parse().ok())))?;
    if host.is_empty() { None } else { Some(format!("{host}:{port}")) }
}

fn plain_lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_string).collect()
}

fn parse_lines(text: &str, default_protocol: &str) -> Vec<Node> {
    plain_lines(text).into_iter().filter_map(|line| parse_one(&line, default_protocol)).collect()
}

fn parse_one(line: &str, default_protocol: &str) -> Option<Node> {
    if let Some((scheme, rest)) = line.split_once("://") {
        let protocol = norm_protocol(scheme);
        if protocol.is_empty() { return None; }
        let (auth, hostport) = match rest.rsplit_once('@') {
            Some((auth, hostport)) => (Some(auth), hostport),
            None => (None, rest),
        };
        let (host, port) = split_host_port(hostport)?;
        let (username, password) = auth.map(split_user_pass).unwrap_or_default();
        return Some(Node { protocol, host, port, username, password });
    }
    if let Some((left, right)) = line.rsplit_once('@') {
        if let Some((host, port)) = split_host_port(right) {
            let (username, password) = split_user_pass(left);
            return Some(Node { protocol: norm_protocol(default_protocol), host, port, username, password });
        }
    }
    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() >= 2 {
        let port = parts[1].parse::<i64>().ok().filter(|port| (1..=65535).contains(port))?;
        let host = parts[0].trim().to_string();
        if !host_ok(&host) { return None; }
        let username = if parts.len() >= 3 { decode(parts[2]) } else { String::new() };
        let password = if parts.len() >= 4 { decode(&parts[3..].join(":")) } else { String::new() };
        return Some(Node { protocol: norm_protocol(default_protocol), host, port, username, password });
    }
    None
}

fn split_host_port(text: &str) -> Option<(String, i64)> {
    let (host, port) = text.rsplit_once(':')?;
    let port = port.parse::<i64>().ok().filter(|port| (1..=65535).contains(port))?;
    let host = host.trim().to_string();
    if !host_ok(&host) { return None; }
    Some((host, port))
}

fn split_user_pass(text: &str) -> (String, String) {
    match text.split_once(':') {
        Some((user, password)) => (decode(user), decode(password)),
        None => (decode(text), String::new()),
    }
}

fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.contains('/')
        && !host.contains('@')
        && !host.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn norm_protocol(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "http" | "https" => "http".into(),
        "socks5" | "socks5h" => "socks5".into(),
        _ => String::new(),
    }
}

fn check_pool(kind: &str, protocol: &str, extract_url: &str) -> Result<(), AdminError> {
    if !matches!(kind, "static" | "extract") {
        return Err(bad("kind 须为 static 或 extract"));
    }
    if norm_protocol(protocol).is_empty() {
        return Err(bad("protocol 须为 http 或 socks5"));
    }
    if kind == "extract" {
        check_extract_url(extract_url)?;
    }
    Ok(())
}

fn check_extract_url(url: &str) -> Result<(), AdminError> {
    let Some((scheme, rest)) = url.trim().split_once("://") else {
        return Err(bad("提取 URL 仅支持 http/https"));
    };
    if !matches!(scheme, "http" | "https") || rest.is_empty() {
        return Err(bad("提取 URL 仅支持 http/https"));
    }
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    if hostport.contains('@') || hostport.is_empty() {
        return Err(bad("提取 URL 仅支持 http/https"));
    }
    let host = hostport.rsplit_once(':').map(|(host, _)| host).unwrap_or(hostport);
    if private_host(host) {
        return Err(bad("提取 URL 不能指向本机或内网"));
    }
    Ok(())
}

fn private_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".local") || host == "::1" {
        return true;
    }
    let Some(ip) = host.parse::<std::net::Ipv4Addr>().ok() else { return false };
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast()
}

fn mask_url(url: &str) -> String {
    match url.find(['?', '#']) {
        Some(index) => url[..index].to_string(),
        None => url.to_string(),
    }
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn bad(detail: &str) -> AdminError {
    AdminError { status: 400, detail: detail.to_string() }
}

fn missing(detail: &str) -> AdminError {
    AdminError { status: 404, detail: detail.to_string() }
}

fn fail(status: u16, detail: &str) -> AdminError {
    AdminError { status, detail: detail.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (Db, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "vpush-proxy-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        (Db::open(&path).await.unwrap(), path)
    }

    #[tokio::test]
    async fn import_masks_password_and_dedupes() {
        let (db, path) = db().await;
        let pool = create_pool(&db, "海外", "static", "socks5", "", 0, 0).await.unwrap();
        let id = pool["id"].as_i64().unwrap();
        let imported = import_text(
            &db,
            id,
            "1.2.3.4:8080\n1.2.3.4:8080\nsocks5://user:s3cret-token@5.6.7.8:1080\n",
            None,
        )
        .await
        .unwrap();
        assert_eq!(imported["imported"], 2);
        let listed = proxies(&db).await.unwrap();
        let body = listed.to_string();
        assert!(!body.contains("s3cret-token"));
        assert!(body.contains("***"));
        assert_eq!(listed["items"].as_array().unwrap().len(), 2);
        delete_proxy(&db, listed["items"][0]["id"].as_i64().unwrap()).await.unwrap();
        delete_pool(&db, id).await.unwrap();
        assert!(pools(&db).await.unwrap()["items"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn extract_stores_nodes_without_echoing_the_vendor_key() {
        let (db, path) = db().await;
        let err = create_pool(&db, "坏", "extract", "http", "http://127.0.0.1/get?token=secret", 300, 180).await.unwrap_err();
        assert_eq!(err.detail, "提取 URL 不能指向本机或内网");
        let pool = create_pool(&db, "提取", "extract", "http", "https://vendor.example/get?token=vendor-secret", 300, 180).await.unwrap();
        assert!(!pool.to_string().contains("vendor-secret"));
        let id = pool["id"].as_i64().unwrap();
        let stored = store_extracted(&db, id, r#"{"data":[{"ip":"9.9.9.9","port":8000}]}"#, 1_700_000_000).await.unwrap();
        assert_eq!(stored["imported"], 1);
        let listed = proxies(&db).await.unwrap();
        assert_eq!(listed["items"][0]["source"], "extract");
        assert_eq!(listed["items"][0]["expires_at"], 1_700_000_300);
        let failed = finish_probe(&db, listed["items"][0]["id"].as_i64().unwrap(), false, None, "连接被拒绝").await.unwrap();
        assert_eq!(failed["ok"], false);
        assert_eq!(proxies(&db).await.unwrap()["items"][0]["status"], "fail");
        let saved = save_routes(&db, &json!({"xueqiu": {"mode": "pool", "pool_id": id}})).await.unwrap();
        assert_eq!(saved["xueqiu"]["pool_id"], id);
        assert_eq!(saved["weibo"]["mode"], "direct");
        assert!(save_routes(&db, &json!({"xueqiu": {"mode": "pool", "pool_id": 999}})).await.is_err());
        let fresh = create_pool(&db, "可用", "static", "http", "", 0, 0).await.unwrap();
        let fresh_id = fresh["id"].as_i64().unwrap();
        import_text(&db, fresh_id, "9.9.9.9:8000", None).await.unwrap();
        save_routes(&db, &json!({"xueqiu": {"mode": "pool", "pool_id": fresh_id}})).await.unwrap();
        let chosen = acquire(&db, "xueqiu").await.unwrap().unwrap();
        assert!(chosen.url.contains("9.9.9.9:8000"));
        note(&db, Some(chosen.id), false, "超时").await;
        note(&db, Some(chosen.id), false, "超时").await;
        note(&db, Some(chosen.id), false, "超时").await;
        assert_eq!(acquire(&db, "xueqiu").await.unwrap_err(), "代理池为空");
        let _ = std::fs::remove_file(path);
    }
}
