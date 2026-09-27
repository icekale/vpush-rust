//! 雪球组合：调仓写入 posts，净值、持仓和今日涨跌写入 cube_snapshots。
//! 身份沿用雪球 App 隐式账号。每轮只拉调仓第一页。

use std::time::Duration;

use serde_json::{json, Value};

use crate::db::Db;

const HISTORY: &str = "https://api.xueqiu.com/cubes/rebalancing/history.json";
const QUOTE: &str = "https://api.xueqiu.com/cubes/quote.json";
const CURRENT: &str = "https://api.xueqiu.com/cubes/rebalancing/current.json";
const NAV: &str = "https://api.xueqiu.com/cubes/nav_daily/all.json";

struct Quote {
    net: Option<f64>,
    day: Option<f64>,
    annual: Option<f64>,
}

struct ComboPost {
    external_id: String,
    title: String,
    content: String,
    url: String,
    published_at: String,
    detail: Value,
}

enum FetchErr {
    Dead,
    Other(String),
}

impl std::fmt::Display for FetchErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchErr::Dead => write!(f, "雪球身份失效"),
            FetchErr::Other(msg) => write!(f, "{msg}"),
        }
    }
}

pub fn spawn(db: Db) {
    if !crate::xueqiu::fetch_enabled() {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = poll(&db).await {
                tracing::warn!("组合抓取: {err}");
            }
            tokio::time::sleep(Duration::from_secs(crate::xueqiu::poll_wait(&db).await)).await;
        }
    });
}

async fn poll(db: &Db) -> Result<(), String> {
    let kols = db.kols_to_fetch("combination").await.map_err(|e| e.to_string())?;
    if kols.is_empty() {
        return Ok(());
    }
    let exit = crate::proxy_admin::acquire(db, "combination").await?;
    let proxy = exit.as_ref().map(|item| item.url.clone());
    let proxy_id = exit.map(|item| item.id);
    let mut cookie = crate::xueqiu::app_cookie(db).await?;
    for (id, name, external_id) in kols {
        let symbol = cube_symbol(&external_id);
        if !symbol.starts_with("ZH") {
            tracing::warn!(kol = id, "无效的组合编码: {external_id}");
            continue;
        }
        match sync_one(db, &cookie, id, &name, &symbol, proxy.clone()).await {
            Ok(()) => {
                crate::proxy_admin::note(db, proxy_id, true, "").await;
                let _ = db.note_kol_fetch(id, None).await;
            }
            Err(FetchErr::Dead) => {
                tracing::warn!(kol = id, "雪球身份失效，重新注册");
                cookie = crate::xueqiu::rotate_app_cookie(db).await?;
                if let Err(err) = sync_one(db, &cookie, id, &name, &symbol, proxy.clone()).await {
                    let detail = err.to_string();
                    let _ = db.note_kol_fetch(id, Some(&detail)).await;
                    tracing::warn!(kol = id, "{err}");
                } else {
                    let _ = db.note_kol_fetch(id, None).await;
                }
            }
            Err(FetchErr::Other(msg)) => {
                crate::proxy_admin::note(db, proxy_id, false, &msg).await;
                let _ = db.note_kol_fetch(id, Some(&msg)).await;
                tracing::warn!(kol = id, "{msg}");
            }
        }
    }
    Ok(())
}

async fn sync_one(db: &Db, cookie: &str, kol_id: i64, name: &str, symbol: &str, proxy: Option<String>) -> Result<(), FetchErr> {
    let history = get_json(
        cookie,
        HISTORY,
        &[("cube_symbol", symbol), ("page", "1"), ("count", "20")],
        proxy.as_deref(),
    )
    .await?;
    let rows = history.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let watermark = db.max_published_at(kol_id).await.map_err(|e| FetchErr::Other(e.to_string()))?;
    let mut fresh = Vec::new();
    for item in &rows {
        if item.get("status").and_then(Value::as_str) != Some("success") {
            continue;
        }
        let external_id = field_str(item, "id");
        if external_id.is_empty() || stale(&crate::xueqiu::published_of(&item["updated_at"]), &watermark) {
            continue;
        }
        if !db
            .has_post("combination", &external_id)
            .await
            .map_err(|e| FetchErr::Other(e.to_string()))?
        {
            fresh.push(item.clone());
        }
    }
    refresh_snapshots(db, cookie, kol_id, symbol, !fresh.is_empty(), proxy.as_deref()).await;
    let quote = load_quote(db, kol_id).await;
    let (mut holdings, cash) = load_holdings(db, kol_id).await;
    if !fresh.is_empty() {
        for item in fresh.iter().rev() {
            let histories = item.get("rebalancing_histories").cloned().unwrap_or(Value::Null);
            holdings = apply_rebalancing(&holdings, &histories);
        }
        let payload = json!({ "holdings": holdings, "cash": cash }).to_string();
        db.set_cube_snapshot(kol_id, "holdings", &payload)
            .await
            .map_err(|e| FetchErr::Other(e.to_string()))?;
    }
    for post in build_posts(name, symbol, &quote, &holdings, &rows, &watermark) {
        let is_new = !db
            .has_post("combination", &post.external_id)
            .await
            .map_err(|e| FetchErr::Other(e.to_string()))?;
        let detail = post.detail.to_string();
        db.save_combo(
            kol_id,
            &post.external_id,
            &post.title,
            &post.content,
            &post.url,
            &post.published_at,
            &detail,
        )
        .await
        .map_err(|e| FetchErr::Other(e.to_string()))?;
        if !is_new {
            continue;
        }
        let push = db
            .should_push(kol_id, "post")
            .await
            .map_err(|e| FetchErr::Other(e.to_string()))?;
        if !push {
            continue;
        }
        crate::push::deliver(
            db,
            kol_id,
            &crate::feishu::Note {
                kol_name: name,
                platform: "combination",
                post_type: "post",
                title: &post.title,
                content: &post.content,
                url: &post.url,
                published_at: &post.published_at,
            },
        )
        .await;
    }
    Ok(())
}

async fn refresh_snapshots(db: &Db, cookie: &str, kol_id: i64, symbol: &str, force_holdings: bool, proxy: Option<&str>) {
    let jobs = [
        ("quote", QUOTE, vec![("code", symbol), ("cube_symbol", symbol)], 60, false),
        ("holdings", CURRENT, vec![("cube_symbol", symbol)], 300, force_holdings),
        ("nav", NAV, vec![("cube_symbol", symbol)], 3600, false),
    ];
    for (kind, url, query, ttl, force) in jobs {
        if !force && db.cube_fresh(kol_id, kind, ttl).await.unwrap_or(false) {
            continue;
        }
        let owned: Vec<(&str, &str)> = query;
        let data = match get_json(cookie, url, &owned, proxy).await {
            Ok(data) => data,
            Err(err) => {
                tracing::warn!(kol = kol_id, "{kind} 快照抓取失败: {err}");
                continue;
            }
        };
        if xueqiu_error(&data) {
            tracing::warn!(kol = kol_id, "{kind} 快照接口返回错误");
            continue;
        }
        let payload = match kind {
            "quote" => {
                let quote = parse_quote(&data);
                if quote.net.is_none() && quote.day.is_none() {
                    continue;
                }
                quote_json(&quote).to_string()
            }
            "holdings" => {
                if !looks_like_holdings(&data) {
                    continue;
                }
                json!({ "holdings": parse_holdings(&data), "cash": parse_cash(&data) }).to_string()
            }
            _ => {
                let series = parse_nav(&data);
                if series.is_empty() {
                    continue;
                }
                json!({ "series": series, "benchmark": parse_benchmark(&data) }).to_string()
            }
        };
        if let Err(err) = db.set_cube_snapshot(kol_id, kind, &payload).await {
            tracing::warn!(kol = kol_id, "{kind} 快照写入失败: {err}");
        }
    }
}

async fn load_quote(db: &Db, kol_id: i64) -> Quote {
    let raw = db.cube_snapshot(kol_id, "quote").await.ok().flatten();
    raw.and_then(|(payload, _)| serde_json::from_str(&payload).ok())
        .map(|value| parse_quote(&value))
        .unwrap_or(Quote { net: None, day: None, annual: None })
}

async fn load_holdings(db: &Db, kol_id: i64) -> (Vec<Value>, Value) {
    let Some((payload, _)) = db.cube_snapshot(kol_id, "holdings").await.ok().flatten() else {
        return (Vec::new(), Value::Null);
    };
    let parsed: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
    let cash = parsed.get("cash").cloned().unwrap_or(Value::Null);
    (holdings_rows(&parsed), cash)
}

pub fn holdings_response(snap: Option<(String, String)>) -> Value {
    let Some((payload, fetched_at)) = snap else {
        return json!({ "holdings": [], "cash": null, "updated_at": "" });
    };
    let parsed: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
    if parsed.is_array() {
        return json!({ "holdings": parsed, "cash": null, "updated_at": fetched_at });
    }
    json!({
        "holdings": parsed.get("holdings").cloned().unwrap_or_else(|| json!([])),
        "cash": parsed.get("cash").cloned().unwrap_or(Value::Null),
        "updated_at": fetched_at,
    })
}

pub fn nav_response(snap: Option<(String, String)>) -> Value {
    let Some((payload, fetched_at)) = snap else {
        return json!({ "series": [], "benchmark": [], "updated_at": "" });
    };
    let parsed: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
    if parsed.is_array() {
        return json!({ "series": parsed, "benchmark": [], "updated_at": fetched_at });
    }
    json!({
        "series": parsed.get("series").cloned().unwrap_or_else(|| json!([])),
        "benchmark": parsed.get("benchmark").cloned().unwrap_or_else(|| json!([])),
        "updated_at": fetched_at,
    })
}

fn cube_symbol(external_id: &str) -> String {
    let bytes = external_id.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'Z' && bytes[i + 1] == b'H' && bytes[i + 2].is_ascii_digit() {
            let start = i;
            i += 2;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            return external_id[start..i].to_string();
        }
        i += 1;
    }
    external_id.trim().to_string()
}

fn parse_quote(input: &Value) -> Quote {
    let mut data = input.clone();
    if let Some(inner) = data.get("data").filter(|v| v.is_object()).cloned() {
        data = inner;
    }
    if data.get("net_value").is_none() && data.get("daily_gain").is_none() {
        if let Some(inner) = data.as_object().and_then(|obj| obj.values().find(|v| v.is_object())).cloned() {
            data = inner;
        }
    }
    let day = ["daily_gain", "day_percent_gain", "percent"].into_iter().find_map(|key| num(&data[key]));
    let annual = ["annualized_gain", "annualized_gain_rate"].into_iter().find_map(|key| num(&data[key]));
    Quote { net: num(&data["net_value"]), day, annual }
}

fn quote_json(quote: &Quote) -> Value {
    json!({
        "net_value": quote.net,
        "day_percent_gain": quote.day,
        "annualized_gain": quote.annual,
    })
}

fn parse_holdings(data: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for row in holdings_rows(data) {
        let weight = num(&row["weight"]).or_else(|| num(&row["target_weight"]));
        let Some(weight) = weight else { continue };
        if !row.get("weight").is_some_and(Value::is_number) && !row.get("target_weight").is_some_and(Value::is_number) {
            continue;
        }
        let mut item = json!({
            "name": field_str(&row, "stock_name"),
            "symbol": field_str(&row, "stock_symbol"),
            "weight": round_n(weight, 2),
        });
        if let Some(prev) = num(&row["prev_weight"]) {
            if row.get("prev_weight").is_some_and(Value::is_number) {
                item["prev"] = json!(round_n(prev, 2));
            }
        }
        out.push(item);
    }
    out
}

fn holdings_rows(data: &Value) -> Vec<Value> {
    if let Some(rows) = data.as_array() {
        return rows.clone();
    }
    let inner = data.get("data");
    if let Some(rows) = inner.and_then(Value::as_array) {
        return rows.clone();
    }
    if let Some(rows) = inner.and_then(|v| v.get("holdings")).and_then(Value::as_array) {
        return rows.clone();
    }
    if let Some(rows) = data.get("holdings").and_then(Value::as_array) {
        return rows.clone();
    }
    data.get("last_rb")
        .and_then(|v| v.get("holdings"))
        .and_then(Value::as_array)
        .or_else(|| {
            inner
                .and_then(|v| v.get("last_rb"))
                .and_then(|v| v.get("holdings"))
                .and_then(Value::as_array)
        })
        .cloned()
        .unwrap_or_default()
}

fn parse_cash(data: &Value) -> Option<f64> {
    let inner = data.get("data").filter(|v| v.is_object());
    for obj in [data.get("last_rb"), inner.and_then(|v| v.get("last_rb"))] {
        let Some(cash) = obj.and_then(|v| num(&v["cash"])) else { continue };
        if obj.is_some_and(|v| v.get("cash").is_some_and(Value::is_number) || v.get("cash").is_some_and(Value::is_string)) {
            return Some(round_n(cash, 2));
        }
    }
    None
}

fn looks_like_holdings(data: &Value) -> bool {
    if data.is_array() {
        return true;
    }
    if data.get("holdings").is_some_and(Value::is_array) || data.get("last_rb").is_some_and(Value::is_object) {
        return true;
    }
    let inner = data.get("data");
    inner.is_some_and(Value::is_array)
        || inner.is_some_and(|v| v.get("holdings").is_some_and(Value::is_array) || v.get("last_rb").is_some_and(Value::is_object))
}

fn parse_nav(data: &Value) -> Vec<Value> {
    data.as_array().and_then(|rows| rows.first()).map(nav_series).unwrap_or_default()
}

fn parse_benchmark(data: &Value) -> Vec<Value> {
    data.as_array().and_then(|rows| rows.get(1)).map(nav_series).unwrap_or_default()
}

fn nav_series(obj: &Value) -> Vec<Value> {
    obj.get("list")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let value = row.get("value").filter(|v| v.is_number()).and_then(num)?;
                    Some(json!({ "date": field_str(row, "date"), "value": round_n(value, 4) }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn apply_rebalancing(holdings: &[Value], histories: &Value) -> Vec<Value> {
    let mut order = Vec::new();
    let mut index: Vec<(String, Value)> = Vec::new();
    for row in holdings {
        let key = holding_key(&field_str(row, "symbol"), &field_str(row, "name"));
        if key.is_empty() {
            continue;
        }
        let kept = json!({
            "name": field_str(row, "name"),
            "symbol": field_str(row, "symbol"),
            "weight": row.get("weight").cloned().unwrap_or(Value::Null),
        });
        if let Some(pos) = index.iter().position(|(k, _)| k == &key) {
            index[pos].1 = kept;
        } else {
            order.push(key.clone());
            index.push((key, kept));
        }
    }
    let rows = histories.as_array().cloned().unwrap_or_default();
    for row in rows {
        let name = field_str(&row, "stock_name");
        let symbol = field_str(&row, "stock_symbol");
        let key = holding_key(&symbol, &name);
        if key.is_empty() {
            continue;
        }
        let target = row.get("target_weight").filter(|v| v.is_number()).and_then(num)
            .or_else(|| row.get("weight").filter(|v| v.is_number()).and_then(num));
        if target.is_none_or(|w| w <= 0.0) {
            index.retain(|(k, _)| k != &key);
            continue;
        }
        let kept = json!({ "name": name, "symbol": symbol, "weight": round_n(target.unwrap_or(0.0), 2) });
        if let Some(pos) = index.iter().position(|(k, _)| k == &key) {
            index[pos].1 = kept;
        } else {
            order.push(key.clone());
            index.push((key, kept));
        }
    }
    order
        .into_iter()
        .filter_map(|key| index.iter().find(|(k, _)| k == &key).map(|(_, row)| row.clone()))
        .collect()
}

fn build_posts(
    name: &str,
    symbol: &str,
    quote: &Quote,
    holdings: &[Value],
    rows: &[Value],
    watermark: &str,
) -> Vec<ComboPost> {
    let mut stats_parts = Vec::new();
    if let Some(day) = quote.day {
        let sign = if day >= 0.0 { "+" } else { "" };
        stats_parts.push(format!("今日 {sign}{day:.2}%"));
    }
    if let Some(annual) = quote.annual {
        stats_parts.push(format!("年化 {annual:.1}%"));
    }
    if let Some(net) = quote.net {
        stats_parts.push(format!("净值 {net:.3}"));
    }
    let stats_line = stats_parts.join(" · ");
    let mut stats = Vec::new();
    if let Some(day) = quote.day {
        let sign = if day >= 0.0 { "+" } else { "" };
        stats.push(json!(["今日", format!("{sign}{day:.2}%")]));
    }
    if let Some(annual) = quote.annual {
        stats.push(json!(["年化", format!("{annual:.1}%")]));
    }
    if let Some(net) = quote.net {
        stats.push(json!(["净值", format!("{net:.3}")]));
    }
    let mut posts = Vec::new();
    for item in rows {
        if item.get("status").and_then(Value::as_str) != Some("success") {
            continue;
        }
        let external_id = field_str(item, "id");
        let published_at = crate::xueqiu::published_of(&item["updated_at"]);
        if external_id.is_empty() || stale(&published_at, watermark) {
            continue;
        }
        let histories = item.get("rebalancing_histories").and_then(Value::as_array);
        let Some(histories) = histories else { continue };
        if histories.is_empty() {
            continue;
        }
        let mut lines = Vec::new();
        let mut actions = Vec::new();
        for row in histories {
            let prev = row.get("prev_weight").filter(|v| v.is_number()).and_then(num);
            let target = row.get("target_weight").filter(|v| v.is_number()).and_then(num);
            if let (Some(prev), Some(target)) = (prev, target) {
                if (target - prev).abs() < 1e-9 {
                    continue;
                }
            }
            let stock = field_str(row, "stock_name");
            let stock_symbol = field_str(row, "stock_symbol");
            let prev_s = prev.map(|n| format!("{n:.1}%")).unwrap_or_default();
            let target_s = target.map(|n| format!("{n:.1}%")).unwrap_or_default();
            let kind = if prev.is_none() && target.is_some() {
                lines.push(format!("🆕 {stock} 新建 {target_s}"));
                "新建"
            } else if prev.is_some() && target.is_none_or(|n| n <= 0.0) {
                lines.push(format!("🗑 {stock} 清仓 {prev_s}"));
                "清仓"
            } else if let (Some(prev), Some(target)) = (prev, target) {
                if target > prev {
                    lines.push(format!("➕ {stock} {prev_s} → {target_s}"));
                    "增持"
                } else {
                    lines.push(format!("➖ {stock} {prev_s} → {target_s}"));
                    "减持"
                }
            } else {
                ""
            };
            if kind.is_empty() {
                continue;
            }
            let mut action = json!({
                "type": kind,
                "stock": stock,
                "symbol": stock_symbol,
                "prev": if prev_s.is_empty() { "0.0%" } else { &prev_s },
                "target": if target_s.is_empty() { "0.0%" } else { &target_s },
            });
            if let Some(price) = ["price", "stock_price", "trade_price"].into_iter().find_map(|key| num(&row[key])) {
                action["price"] = json!(format!("{price:.2}"));
            }
            actions.push(action);
        }
        if lines.is_empty() && actions.is_empty() {
            continue;
        }
        let cash_pct = match (num(&item["cash_value"]), quote.net) {
            (Some(cash), Some(net)) if net != 0.0 => format!("{:.1}%", cash / net * 100.0),
            _ => String::new(),
        };
        let mut content = lines.join("\n");
        if !cash_pct.is_empty() {
            let cash_line = format!("现金 {cash_pct}");
            content = if content.is_empty() { cash_line } else { format!("{content}\n{cash_line}") };
        }
        if !stats_line.is_empty() {
            content = if content.is_empty() { stats_line.clone() } else { format!("{stats_line}\n{content}") };
        }
        posts.push(ComboPost {
            external_id,
            title: format!("{name} 调仓"),
            content,
            url: format!("https://xueqiu.com/P/{symbol}"),
            published_at,
            detail: json!({
                "stats": stats,
                "actions": actions,
                "holdings": holdings,
                "cash": cash_pct,
            }),
        });
    }
    posts
}

fn stale(published_at: &str, watermark: &str) -> bool {
    !watermark.is_empty() && published_at < watermark
}

fn holding_key(symbol: &str, name: &str) -> String {
    let symbol = symbol.trim();
    if !symbol.is_empty() { symbol.to_string() } else { name.trim().to_string() }
}

fn xueqiu_error(data: &Value) -> bool {
    match &data["error_code"] {
        Value::Null => false,
        Value::Number(n) => n.as_i64() != Some(0),
        Value::String(s) => !s.is_empty() && s != "0",
        _ => false,
    }
}

fn num(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn round_n(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (value * scale).round() / scale
}

fn field_str(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn session_dead(value: &Value) -> bool {
    let code = field_str(value, "error_code");
    if code == "10022" || code == "400016" {
        return true;
    }
    let text = format!(
        "{} {}",
        field_str(value, "error_description"),
        field_str(value, "message")
    )
    .to_lowercase();
    ["重新登录", "请登录", "登录帐号", "登录账号", "login"].iter().any(|m| text.contains(m))
}

async fn get_json(cookie: &str, url: &str, query: &[(&str, &str)], proxy: Option<&str>) -> Result<Value, FetchErr> {
    let cookie = cookie.to_string();
    let url = url.to_string();
    let proxy = proxy.map(str::to_string);
    let query: Vec<(String, String)> = query.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    match tokio::task::spawn_blocking(move || fetch_json(&cookie, &url, &query, proxy.as_deref())).await {
        Ok(result) => result,
        Err(err) => Err(FetchErr::Other(err.to_string())),
    }
}

fn fetch_json(cookie: &str, url: &str, query: &[(String, String)], proxy: Option<&str>) -> Result<Value, FetchErr> {
    let agent = crate::proxy_admin::http_agent(proxy, Duration::from_secs(15), Duration::from_secs(20)).map_err(FetchErr::Other)?;
    let mut req = agent.get(url);
    for (key, value) in query {
        req = req.query(key, value);
    }
    let req = req
        .set("User-Agent", crate::xueqiu::APP_UA)
        .set("Accept", "application/json, text/plain, */*")
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .set("Origin", "https://xueqiu.com")
        .set("X-Requested-With", "XMLHttpRequest")
        .set("Referer", "https://xueqiu.com/")
        .set("Cookie", cookie);
    let (status, text) = read_response(req.call()).map_err(FetchErr::Other)?;
    if text.contains("EO_Bot_Ssid") || text.contains("__tst_status") || text.contains("aliyun_waf") {
        return Err(FetchErr::Other("组合接口返回挑战页".into()));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| FetchErr::Other(e.to_string()))?;
    if status == 401 || status == 403 || session_dead(&value) {
        return Err(FetchErr::Dead);
    }
    if status == 429 || field_str(&value, "error_code") == "110017" {
        return Err(FetchErr::Other(format!("雪球限流 {status}")));
    }
    if status != 200 {
        return Err(FetchErr::Other(format!("组合接口 HTTP {status}")));
    }
    Ok(value)
}

fn read_response(result: Result<ureq::Response, ureq::Error>) -> Result<(u16, String), String> {
    match result {
        Ok(resp) => {
            let status = resp.status();
            resp.into_string().map(|text| (status, text)).map_err(|e| e.to_string())
        }
        Err(ureq::Error::Status(status, resp)) => {
            resp.into_string().map(|text| (status, text)).map_err(|e| e.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_holdings_nav_and_rebalance_card() {
        assert_eq!(cube_symbol("https://xueqiu.com/P/ZH1234567?from=home"), "ZH1234567");
        let quote = parse_quote(&json!({"ZH1": {"net_value": "1.2345", "daily_gain": "-0.5", "annualized_gain_rate": "12.34"}}));
        assert_eq!(quote.net, Some(1.2345));
        assert_eq!(quote.day, Some(-0.5));
        assert_eq!(quote.annual, Some(12.34));
        let current = json!({"last_rb": {"cash": 12.345, "holdings": [
            {"stock_name": "茅台", "stock_symbol": "SH600519", "weight": 20, "prev_weight": 18},
            {"stock_name": "现金伪行", "stock_symbol": "", "weight": "nope"}
        ]}, "cash": 99});
        let holdings = parse_holdings(&current);
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0]["prev"], 18.0);
        assert_eq!(parse_cash(&current), Some(12.35));
        let changed = apply_rebalancing(&holdings, &json!([
            {"stock_name": "茅台", "stock_symbol": "SH600519", "target_weight": 0},
            {"stock_name": "招行", "stock_symbol": "SH600036", "target_weight": 15.126}
        ]));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0]["symbol"], "SH600036");
        assert_eq!(changed[0]["weight"], 15.13);
        let history = json!({"list": [
            {"id": 9, "status": "success", "updated_at": 1710000000000_i64, "cash_value": 0.15, "rebalancing_histories": [
                {"stock_name": "招行", "stock_symbol": "SH600036", "prev_weight": 10, "target_weight": 15, "price": 30.5},
                {"stock_name": "茅台", "stock_symbol": "SH600519", "prev_weight": 20, "target_weight": 20}
            ]},
            {"id": 8, "status": "success", "updated_at": "2020-01-01 00:00", "rebalancing_histories": [
                {"stock_name": "旧仓", "stock_symbol": "SZ1", "prev_weight": 1, "target_weight": 2}
            ]}
        ]});
        let posts = build_posts(
            "示例组合",
            "ZH1",
            &quote,
            &changed,
            history["list"].as_array().unwrap(),
            "2024-01-01 00:00",
        );
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].external_id, "9");
        assert!(posts[0].content.starts_with("今日 -0.50% · 年化 12.3% · 净值 1.234"));
        assert!(posts[0].content.contains("➕ 招行 10.0% → 15.0%"));
        assert!(posts[0].content.contains("现金 12.2%"));
        assert!(!posts[0].content.contains("茅台"));
        assert_eq!(posts[0].detail["actions"][0]["price"], "30.50");
        assert_eq!(posts[0].detail["holdings"][0]["symbol"], "SH600036");
        let nav = json!([
            {"list": [{"date": "2024-01-02", "value": 1.2}, {"date": "2024-01-03", "value": "x"}]},
            {"list": [{"date": "2024-01-02", "value": 1}]}
        ]);
        assert_eq!(parse_nav(&nav).len(), 1);
        assert_eq!(parse_benchmark(&nav)[0]["value"], 1.0);
        assert_eq!(holdings_response(None)["holdings"], json!([]));
        assert_eq!(nav_response(None)["series"], json!([]));
    }

    #[tokio::test]
    async fn saved_rebalance_is_visible_on_the_kol_page() {
        let path = std::env::temp_dir().join(format!(
            "vpush-combo-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        let kol = db.add_kol("combination", "示例组合", "ZH1", None, false, false, false).await.unwrap();
        db.subscribe(admin.id, true, kol, "post").await.unwrap();
        db.set_cube_snapshot(kol, "quote", r#"{"net_value":1.2,"day_percent_gain":0.5,"annualized_gain":8}"#).await.unwrap();
        let posts = build_posts(
            "示例组合",
            "ZH1",
            &Quote { net: Some(1.2), day: Some(0.5), annual: Some(8.0) },
            &[json!({"name":"招行","symbol":"SH600036","weight":15.0})],
            &[json!({"id": 3, "status": "success", "updated_at": "2024-03-02 09:30", "rebalancing_histories": [
                {"stock_name": "招行", "stock_symbol": "SH600036", "target_weight": 15}
            ]})],
            "",
        );
        let post = &posts[0];
        db.save_combo(kol, &post.external_id, &post.title, &post.content, &post.url, &post.published_at, &post.detail.to_string()).await.unwrap();
        let page = db.kol_posts(admin.id, true, kol, 10).await.unwrap().unwrap();
        assert_eq!(page[0]["detail"]["actions"][0]["type"], "新建");
        let listed = db.catalog(admin.id, true, "combination", 0).await.unwrap();
        assert_eq!(listed[0]["quote"]["net_value"], 1.2);
        assert!(!listed[0]["quote_at"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
