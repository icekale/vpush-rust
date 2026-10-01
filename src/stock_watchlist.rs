use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::db::Db;

const QUOTE_URL: &str = "https://stock.xueqiu.com/v5/stock/batch/quote.json";
const SEARCH_URL: &str = "https://searchapi.eastmoney.com/api/suggest/get";
const US_SEARCH_URL: &str = "https://smartbox.gtimg.cn/s3/";
const MAX_SYMBOLS: i64 = 50;
const MAX_SEARCH: usize = 40;
const QUOTE_MAX_AGE_MS: i64 = 120_000;
const BATCH_SIZE: usize = 20;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Market {
    Cn,
    Hk,
    Us,
}

impl Market {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cn" => Ok(Self::Cn),
            "hk" => Ok(Self::Hk),
            "us" => Ok(Self::Us),
            _ => Err("市场必须是 cn、hk 或 us".into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Hk => "hk",
            Self::Us => "us",
        }
    }

    fn currency(self) -> &'static str {
        match self {
            Self::Cn => "CNY",
            Self::Hk => "HKD",
            Self::Us => "USD",
        }
    }

    fn quote_type(self) -> i64 {
        match self {
            Self::Cn => 11,
            Self::Hk => 30,
            Self::Us => 0,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Instrument {
    market: Market,
    symbol: String,
    xueqiu: String,
}

impl Instrument {
    fn key(&self) -> String {
        format!("{}:{}", self.market.as_str(), self.symbol)
    }

    fn currency(&self) -> &'static str {
        self.market.currency()
    }
}

#[derive(Clone, Debug)]
struct Quote {
    instrument: Instrument,
    name: String,
    price: f64,
    percent: Option<f64>,
    quoted_at: String,
    observed_at_ms: i64,
    tick_size: Option<f64>,
    variable_tick_size: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Above,
    Below,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
        }
    }
}

#[derive(Debug)]
pub struct WatchlistError {
    pub status: u16,
    pub detail: String,
}

impl WatchlistError {
    fn bad(detail: impl Into<String>) -> Self {
        Self {
            status: 400,
            detail: detail.into(),
        }
    }

    fn gateway(detail: impl Into<String>) -> Self {
        Self {
            status: 502,
            detail: detail.into(),
        }
    }

    fn db(err: sqlx::Error) -> Self {
        Self {
            status: 500,
            detail: err.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AddInput {
    pub market: String,
    pub symbol: String,
}

#[derive(Debug, Deserialize)]
pub struct AlertInput {
    pub target: Option<f64>,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct AlertsInput {
    pub above: AlertInput,
    pub below: AlertInput,
}

#[derive(Debug, Deserialize)]
pub struct SearchInput {
    pub market: String,
    pub q: String,
}

#[derive(Clone)]
struct CacheEntry {
    quote: Option<Quote>,
    error: Option<String>,
}

struct QuoteCache {
    at: Option<Instant>,
    refreshing: bool,
    entries: HashMap<String, CacheEntry>,
}

fn cache() -> &'static Mutex<QuoteCache> {
    static CACHE: OnceLock<Mutex<QuoteCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(QuoteCache {
            at: None,
            refreshing: false,
            entries: HashMap::new(),
        })
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn normalize_symbol(market: &str, raw: &str) -> Result<Instrument, String> {
    let market = Market::parse(market)?;
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 20 {
        return Err("股票代码无效".into());
    }
    match market {
        Market::Cn => {
            let mut code = raw.to_ascii_uppercase();
            let explicit_exchange = if code.starts_with("SH.") || code.starts_with("SH") {
                Some("SH")
            } else if code.starts_with("SZ.") || code.starts_with("SZ") {
                Some("SZ")
            } else {
                None
            };
            for prefix in ["SH.", "SZ.", "SH", "SZ"] {
                if code.starts_with(prefix) {
                    code = code[prefix.len()..].to_string();
                    break;
                }
            }
            if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("A股代码必须是六位普通股票代码".into());
            }
            let ordinary = [
                "000", "001", "002", "003", "300", "301", "600", "601", "603", "605", "688", "689",
            ];
            if !ordinary.iter().any(|prefix| code.starts_with(prefix)) {
                return Err("仅支持A股普通股票，不支持ETF、指数、基金或权证".into());
            }
            let exchange = if code.starts_with("6") { "SH" } else { "SZ" };
            if explicit_exchange.is_some_and(|explicit| explicit != exchange) {
                return Err("A股代码与交易所前缀不匹配".into());
            }
            Ok(Instrument {
                market,
                xueqiu: format!("{exchange}{code}"),
                symbol: code,
            })
        }
        Market::Hk => {
            let mut code = raw.to_ascii_uppercase();
            if let Some(stripped) = code.strip_prefix("HK") {
                code = stripped.to_string();
            }
            if code.len() > 5 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("港股代码必须是五位以内数字".into());
            }
            let code = format!("{code:0>5}");
            if code == "00000" {
                return Err("港股代码无效".into());
            }
            Ok(Instrument {
                market,
                xueqiu: code.clone(),
                symbol: code,
            })
        }
        Market::Us => {
            let code = raw.to_ascii_uppercase();
            let valid = code.len() <= 10
                && code.len() >= 1
                && code
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic())
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
            if !valid {
                return Err("美股代码格式无效".into());
            }
            Ok(Instrument {
                market,
                xueqiu: code.clone(),
                symbol: code,
            })
        }
    }
}

fn parse_timestamp(value: i64) -> Result<i64, String> {
    (value > 0)
        .then_some(value)
        .ok_or_else(|| "雪球行情时间戳无效".into())
}

fn number(value: &Value) -> Option<f64> {
    let value = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    value.is_finite().then_some(value)
}

fn parse_batch_quotes(
    payload: &Value,
    wanted: &[Instrument],
    now_ms: i64,
) -> Result<HashMap<String, Quote>, String> {
    let items = payload
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
        .ok_or_else(|| "雪球行情缺少 data.items".to_string())?;
    let mut out = HashMap::new();
    for instrument in wanted {
        let Some(quote) = items
            .iter()
            .filter_map(|item| item.get("quote"))
            .find(|quote| {
                quote
                    .get("symbol")
                    .and_then(Value::as_str)
                    .is_some_and(|symbol| symbol.eq_ignore_ascii_case(&instrument.xueqiu))
            })
        else {
            continue;
        };
        if quote.get("type").and_then(Value::as_i64) != Some(instrument.market.quote_type())
            || quote.get("currency").and_then(Value::as_str) != Some(instrument.currency())
        {
            continue;
        }
        let Some(price) = number(&quote["current"]).filter(|price| *price > 0.0) else {
            continue;
        };
        let Some(timestamp) = quote.get("timestamp").and_then(Value::as_i64) else {
            continue;
        };
        // Old valid quotes remain historical display data; freshness is checked before alerts.
        if parse_timestamp(timestamp).is_err()
            || timestamp < 1_000_000_000_000
            || timestamp > now_ms
        {
            continue;
        }
        let Some((quoted_at, _)) =
            crate::market::exchange_timestamp(instrument.market.as_str(), timestamp)
        else {
            continue;
        };
        let name = quote
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let percent = quote.get("percent").and_then(number);
        let tick_size = quote
            .get("tick_size")
            .and_then(number)
            .filter(|tick| *tick > 0.0);
        let variable_tick_size = quote
            .get("variable_tick_size")
            .and_then(Value::as_str)
            .map(str::to_string);
        out.insert(
            instrument.key(),
            Quote {
                instrument: instrument.clone(),
                name,
                price,
                percent,
                quoted_at,
                observed_at_ms: timestamp,
                tick_size,
                variable_tick_size,
            },
        );
    }
    if out.is_empty() {
        Err("雪球行情没有有效报价".into())
    } else {
        Ok(out)
    }
}

fn crossed(direction: Direction, previous_side: i64, current_side: i64) -> bool {
    match direction {
        Direction::Above => previous_side == -1 && current_side == 1,
        Direction::Below => previous_side == 1 && current_side == -1,
    }
}

fn baseline_side(direction: Direction, target: f64, price: f64) -> i64 {
    match direction {
        Direction::Above => i64::from(price >= target) * 2 - 1,
        Direction::Below => i64::from(price <= target) * -2 + 1,
    }
}

fn documented_tick(market: Market, price: f64) -> Option<f64> {
    if !price.is_finite() || price <= 0.0 {
        return None;
    }
    match market {
        Market::Cn => Some(0.01),
        Market::Us => Some(if price < 1.0 { 0.0001 } else { 0.01 }),
        Market::Hk => None,
    }
}

fn variable_tick(raw: &str, target: f64) -> Option<f64> {
    let values = raw
        .split_whitespace()
        .map(|value| {
            value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value > 0.0)
        })
        .collect::<Option<Vec<_>>>()?;
    if values.is_empty() || values.len() % 2 != 1 {
        return None;
    }
    let mut previous_upper = 0.0;
    for pair in values.chunks(2) {
        if let Some(upper) = pair.get(1) {
            if *upper <= previous_upper {
                return None;
            }
            previous_upper = *upper;
        }
    }
    for pair in values.chunks(2) {
        if pair.get(1).is_none_or(|upper| target < *upper) {
            return Some(pair[0]);
        }
    }
    None
}

fn valid_target_for(
    market: Market,
    target: f64,
    provider_tick: Option<f64>,
    provider_bands: Option<&str>,
) -> bool {
    let tick = if let Some(bands) = provider_bands {
        variable_tick(bands, target)
    } else if market == Market::Hk {
        // The HK current-price tick alone cannot validate a target in another band.
        None
    } else {
        documented_tick(market, target).or(provider_tick)
    };
    let Some(tick) = tick else {
        return false;
    };
    target.is_finite()
        && target > 0.0
        && ((target / tick).round() * tick - target).abs() <= tick * 1e-7
}

#[cfg(test)]
fn valid_target(market: &str, target: f64) -> bool {
    Market::parse(market)
        .ok()
        .is_some_and(|market| valid_target_for(market, target, None, None))
}

fn source_error(status: u16, text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if status == 429 {
        return "雪球行情限流 HTTP 429".into();
    }
    if status == 401 || status == 403 {
        return format!("雪球行情身份/权限错误 HTTP {status}");
    }
    if text.contains("EO_Bot_Ssid")
        || text.contains("__tst_status")
        || text.contains("aliyun_waf")
        || text.contains("挑战")
        || lower.contains("captcha")
        || lower.contains("challenge")
    {
        return "雪球行情返回挑战页".into();
    }
    if text.trim().is_empty() {
        return "雪球行情返回空响应".into();
    }
    if serde_json::from_str::<Value>(text).is_err() {
        return "雪球行情返回非 JSON".into();
    }
    format!("雪球行情 HTTP {status} 错误")
}

fn fetch_batch(cookie: &str, instruments: &[Instrument]) -> Result<HashMap<String, Quote>, String> {
    let symbols = instruments
        .iter()
        .map(|instrument| instrument.xueqiu.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(5))
        .build();
    let response = agent
        .get(QUOTE_URL)
        .query("symbol", &symbols)
        .query("extend", "detail")
        .set("User-Agent", crate::xueqiu::APP_UA)
        .set("Accept", "application/json, text/plain, */*")
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .set("Origin", "https://xueqiu.com")
        .set("X-Requested-With", "XMLHttpRequest")
        .set("Referer", "https://xueqiu.com/")
        .set("Cookie", cookie)
        .call();
    let (status, text) = match response {
        Ok(response) => {
            let status = response.status();
            (
                status,
                response.into_string().map_err(|err| err.to_string())?,
            )
        }
        Err(ureq::Error::Status(status, response)) => {
            (status, response.into_string().unwrap_or_default())
        }
        Err(err) => return Err(format!("雪球行情请求失败: {err}")),
    };
    parse_quote_response(status, &text, instruments, now_ms())
}

fn parse_quote_response(
    status: u16,
    text: &str,
    instruments: &[Instrument],
    now: i64,
) -> Result<HashMap<String, Quote>, String> {
    if status != 200 {
        return Err(source_error(status, text));
    }
    let payload = serde_json::from_str::<Value>(text).map_err(|_| source_error(status, text))?;
    let error_code = match payload.get("error_code") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_str()?.parse().ok())
            .ok_or_else(|| "雪球行情错误码格式无效".to_string())?,
    };
    if error_code != 0 {
        return Err(if error_code == 110017 {
            "雪球行情限流 110017".into()
        } else {
            format!("雪球行情身份或接口错误 {error_code}")
        });
    }
    parse_batch_quotes(&payload, instruments, now)
}

async fn all_instruments(db: &Db) -> Result<Vec<Instrument>, WatchlistError> {
    let rows = sqlx::query("SELECT market, symbol FROM stock_watchlist ORDER BY market, symbol")
        .fetch_all(db.pool())
        .await
        .map_err(WatchlistError::db)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let instrument = normalize_symbol(
            &row.get::<String, _>("market"),
            &row.get::<String, _>("symbol"),
        )
        .map_err(WatchlistError::bad)?;
        if seen.insert(instrument.key()) {
            out.push(instrument);
        }
    }
    Ok(out)
}

async fn refresh_cache(db: &Db) -> Result<(), WatchlistError> {
    let instruments = all_instruments(db).await?;
    if instruments.is_empty() {
        return Ok(());
    }
    {
        let mut cache = cache().lock().unwrap_or_else(|err| err.into_inner());
        if cache.refreshing
            || cache
                .at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return Ok(());
        }
        cache.refreshing = true;
    }
    let result = async {
        let cookie = crate::xueqiu::app_cookie(db)
            .await
            .map_err(WatchlistError::gateway)?;
        let mut quotes = HashMap::new();
        let mut errors = HashMap::new();
        for (index, batch) in instruments.chunks(BATCH_SIZE).enumerate() {
            if index > 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            match tokio::task::spawn_blocking({
                let cookie = cookie.clone();
                let batch = batch.to_vec();
                move || fetch_batch(&cookie, &batch)
            })
            .await
            {
                Ok(Ok(found)) => quotes.extend(found),
                Ok(Err(err)) => {
                    for instrument in batch {
                        errors.insert(instrument.key(), err.clone());
                    }
                }
                Err(err) => {
                    for instrument in batch {
                        errors.insert(instrument.key(), err.to_string());
                    }
                }
            }
        }
        let mut cache = cache().lock().unwrap_or_else(|err| err.into_inner());
        for instrument in instruments {
            let key = instrument.key();
            if let Some(quote) = quotes.remove(&key) {
                cache.entries.insert(
                    key,
                    CacheEntry {
                        quote: Some(quote),
                        error: None,
                    },
                );
            } else {
                let error = errors
                    .remove(&key)
                    .unwrap_or_else(|| "雪球行情缺少该标的".into());
                cache.entries.insert(
                    key,
                    CacheEntry {
                        quote: None,
                        error: Some(error),
                    },
                );
            }
        }
        cache.at = Some(Instant::now());
        cache.refreshing = false;
        Ok::<(), WatchlistError>(())
    }
    .await;
    if let Err(error) = &result {
        let mut cache = cache().lock().unwrap_or_else(|err| err.into_inner());
        for entry in cache.entries.values_mut() {
            entry.quote = None;
            entry.error = Some(error.detail.clone());
        }
        cache.at = Some(Instant::now());
        cache.refreshing = false;
    }
    result
}

fn cached(key: &str) -> Option<CacheEntry> {
    cache()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .entries
        .get(key)
        .cloned()
}

pub async fn list(db: &Db, user_id: i64) -> Result<Value, WatchlistError> {
    refresh_cache(db).await?;
    let rows = sqlx::query("SELECT w.market, w.symbol, a.target AS above_target, a.enabled AS above_enabled, a.delivery_status AS above_status, a.last_triggered_at AS above_triggered, b.target AS below_target, b.enabled AS below_enabled, b.delivery_status AS below_status, b.last_triggered_at AS below_triggered FROM stock_watchlist w LEFT JOIN stock_price_alerts a ON a.user_id=w.user_id AND a.market=w.market AND a.symbol=w.symbol AND a.direction='above' LEFT JOIN stock_price_alerts b ON b.user_id=w.user_id AND b.market=w.market AND b.symbol=w.symbol AND b.direction='below' WHERE w.user_id=? ORDER BY w.market,w.symbol")
        .bind(user_id).fetch_all(db.pool()).await.map_err(WatchlistError::db)?;
    let now = now_ms();
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let market = row.get::<String, _>("market");
        let symbol = row.get::<String, _>("symbol");
        let instrument = normalize_symbol(&market, &symbol).map_err(WatchlistError::bad)?;
        let entry = cached(&instrument.key());
        let quote = entry.as_ref().and_then(|entry| entry.quote.as_ref());
        let error = entry
            .as_ref()
            .and_then(|entry| entry.error.clone())
            .or_else(|| {
                (!crate::market::exchange_calendar_known(market.as_str(), now / 1000))
                    .then(|| "该年度交易日历尚未核验".to_string())
            });
        let status = if error.is_some()
            || quote.is_none()
            || quote.is_some_and(|quote| quote.observed_at_ms > now)
        {
            "unavailable"
        } else if !crate::market::regular_session_open(market.as_str(), now / 1000) {
            "closed"
        } else if quote.is_some_and(|quote| now - quote.observed_at_ms > QUOTE_MAX_AGE_MS) {
            "stale"
        } else {
            "live"
        };
        let visible_quote = quote.filter(|_| status == "live" || status == "closed");
        let mut item = json!({
            "market": market, "symbol": symbol,
            "name": quote.map(|quote| quote.name.clone()).unwrap_or_default(),
            "currency": instrument.currency(), "price": visible_quote.map(|quote| quote.price),
            "percent": visible_quote.and_then(|quote| quote.percent),
            "tick_size": quote.and_then(|quote| quote.tick_size),
            "quoted_at": quote.map(|quote| quote.quoted_at.clone()), "status": status,
            "alerts": {"above": alert_json_from_columns(row, "above"), "below": alert_json_from_columns(row, "below")},
        });
        if let Some(error) = error {
            item["error"] = json!(error);
        }
        items.push(item);
    }
    Ok(json!({"items": items, "source": "xueqiu"}))
}

fn alert_json_from_columns(row: &sqlx::sqlite::SqliteRow, direction: &str) -> Value {
    let prefix = if direction == "above" {
        "above"
    } else {
        "below"
    };
    let target_column = format!("{prefix}_target");
    let enabled_column = format!("{prefix}_enabled");
    let status_column = format!("{prefix}_status");
    let triggered_column = format!("{prefix}_triggered");
    json!({
        "target": row.get::<Option<f64>, _>(target_column.as_str()),
        "enabled": row.get::<Option<i64>, _>(enabled_column.as_str()).is_some_and(|value| value != 0),
        "delivery_status": row.get::<Option<String>, _>(status_column.as_str()).unwrap_or_default(),
        "last_triggered_at": row.get::<Option<String>, _>(triggered_column.as_str()),
    })
}

pub async fn add(db: &Db, user_id: i64, input: AddInput) -> Result<Value, WatchlistError> {
    let instrument = normalize_symbol(&input.market, &input.symbol).map_err(WatchlistError::bad)?;
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM stock_watchlist WHERE user_id=? AND market=? AND symbol=?",
    )
    .bind(user_id)
    .bind(instrument.market.as_str())
    .bind(&instrument.symbol)
    .fetch_one(db.pool())
    .await
    .map_err(WatchlistError::db)?;
    if exists == 0
        && cached(&instrument.key())
            .is_none_or(|entry| entry.error.is_some() || entry.quote.is_none())
    {
        // Security verification happens on add; recurring quotes always use shared batches.
        let cookie = crate::xueqiu::app_cookie(db)
            .await
            .map_err(WatchlistError::gateway)?;
        let wanted = vec![instrument.clone()];
        let mut found = tokio::task::spawn_blocking(move || fetch_batch(&cookie, &wanted))
            .await
            .map_err(|err| WatchlistError::gateway(err.to_string()))?
            .map_err(WatchlistError::gateway)?;
        let quote = found
            .remove(&instrument.key())
            .ok_or_else(|| WatchlistError::bad("未核验为该市场的普通股票"))?;
        cache()
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .insert(
                instrument.key(),
                CacheEntry {
                    quote: Some(quote),
                    error: None,
                },
            );
    }
    insert_watchlist(db, user_id, instrument).await
}

async fn insert_watchlist(
    db: &Db,
    user_id: i64,
    instrument: Instrument,
) -> Result<Value, WatchlistError> {
    let inserted = sqlx::query("INSERT OR IGNORE INTO stock_watchlist (user_id,market,symbol) SELECT ?,?,? WHERE (SELECT COUNT(*) FROM stock_watchlist WHERE user_id=?) < ?")
        .bind(user_id).bind(instrument.market.as_str()).bind(&instrument.symbol).bind(user_id).bind(MAX_SYMBOLS)
        .execute(db.pool()).await.map_err(WatchlistError::db)?;
    if inserted.rows_affected() == 0 {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM stock_watchlist WHERE user_id=? AND market=? AND symbol=?",
        )
        .bind(user_id)
        .bind(instrument.market.as_str())
        .bind(&instrument.symbol)
        .fetch_one(db.pool())
        .await
        .map_err(WatchlistError::db)?;
        if exists == 0 {
            return Err(WatchlistError::bad("自选股数量已达上限"));
        }
    }
    Ok(
        json!({"market": instrument.market.as_str(), "symbol": instrument.symbol, "currency": instrument.currency()}),
    )
}

pub async fn remove(
    db: &Db,
    user_id: i64,
    market: &str,
    symbol: &str,
) -> Result<Value, WatchlistError> {
    let instrument = normalize_symbol(market, symbol).map_err(WatchlistError::bad)?;
    let result =
        sqlx::query("DELETE FROM stock_watchlist WHERE user_id=? AND market=? AND symbol=?")
            .bind(user_id)
            .bind(instrument.market.as_str())
            .bind(&instrument.symbol)
            .execute(db.pool())
            .await
            .map_err(WatchlistError::db)?;
    Ok(json!({"removed": result.rows_affected() == 1}))
}

pub async fn save_alerts(
    db: &Db,
    user_id: i64,
    market: &str,
    symbol: &str,
    input: AlertsInput,
) -> Result<Value, WatchlistError> {
    let instrument = normalize_symbol(market, symbol).map_err(WatchlistError::bad)?;
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM stock_watchlist WHERE user_id=? AND market=? AND symbol=?",
    )
    .bind(user_id)
    .bind(instrument.market.as_str())
    .bind(&instrument.symbol)
    .fetch_one(db.pool())
    .await
    .map_err(WatchlistError::db)?;
    if exists == 0 {
        return Err(WatchlistError {
            status: 404,
            detail: "自选股不存在".into(),
        });
    }
    let above_target = input.above.target;
    let below_target = input.below.target;
    let mut tx = db.pool().begin().await.map_err(WatchlistError::db)?;
    for (direction, rule) in [
        (Direction::Above, input.above),
        (Direction::Below, input.below),
    ] {
        if rule.enabled && rule.target.is_none() {
            return Err(WatchlistError::bad("启用提醒时必须填写目标价"));
        }
        if let Some(target) = rule.target {
            if rule.enabled
                && cached(&instrument.key())
                    .and_then(|entry| entry.quote)
                    .is_none()
            {
                return Err(WatchlistError::bad(
                    "该证券尚未通过雪球行情验证，暂不能启用提醒",
                ));
            }
            let (provider_tick, provider_bands) = cached(&instrument.key())
                .and_then(|entry| entry.quote)
                .map(|quote| (quote.tick_size, quote.variable_tick_size))
                .unwrap_or((None, None));
            if !valid_target_for(
                instrument.market,
                target,
                provider_tick,
                provider_bands.as_deref(),
            ) {
                return Err(WatchlistError::bad("目标价不是该市场支持的最小报价单位"));
            }
            sqlx::query("INSERT INTO stock_price_alerts (user_id,market,symbol,direction,target,enabled) VALUES (?,?,?,?,?,?) ON CONFLICT(user_id,market,symbol,direction) DO UPDATE SET target=excluded.target, enabled=excluded.enabled, baseline_side=CASE WHEN stock_price_alerts.target != excluded.target OR stock_price_alerts.enabled != excluded.enabled THEN NULL ELSE stock_price_alerts.baseline_side END, last_observed_at=CASE WHEN stock_price_alerts.target != excluded.target OR stock_price_alerts.enabled != excluded.enabled THEN NULL ELSE stock_price_alerts.last_observed_at END, last_triggered_at=CASE WHEN stock_price_alerts.target != excluded.target OR stock_price_alerts.enabled != excluded.enabled THEN NULL ELSE stock_price_alerts.last_triggered_at END, delivery_status=CASE WHEN stock_price_alerts.target != excluded.target OR stock_price_alerts.enabled != excluded.enabled THEN '' ELSE stock_price_alerts.delivery_status END")
                .bind(user_id).bind(instrument.market.as_str()).bind(&instrument.symbol).bind(direction.as_str()).bind(target).bind(rule.enabled).execute(&mut *tx).await.map_err(WatchlistError::db)?;
        } else {
            sqlx::query("DELETE FROM stock_price_alerts WHERE user_id=? AND market=? AND symbol=? AND direction=?")
                .bind(user_id).bind(instrument.market.as_str()).bind(&instrument.symbol).bind(direction.as_str()).execute(&mut *tx).await.map_err(WatchlistError::db)?;
        }
    }
    tx.commit().await.map_err(WatchlistError::db)?;
    Ok(
        json!({"market": instrument.market.as_str(), "symbol": instrument.symbol, "above": above_target, "below": below_target}),
    )
}

fn parse_search_candidates(payload: &Value, market: Market) -> Result<Vec<Instrument>, String> {
    let table = payload
        .get("QuotationCodeTable")
        .ok_or_else(|| "搜索源缺少结果表".to_string())?;
    if table.get("Status").and_then(Value::as_i64) != Some(0) {
        return Err("搜索源返回异常状态".into());
    }
    let rows = table
        .get("Data")
        .and_then(Value::as_array)
        .ok_or_else(|| "搜索源缺少结果列表".to_string())?;
    let classify = match market {
        Market::Cn => "AStock",
        Market::Hk => "HK",
        Market::Us => "UsStock",
    };
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for row in rows {
        if row.get("Classify").and_then(Value::as_str) != Some(classify) {
            continue;
        }
        let security_type = row.get("TypeUS").and_then(Value::as_str);
        let ordinary = match market {
            Market::Cn => true,
            Market::Hk => security_type == Some("3"),
            Market::Us => matches!(security_type, Some("1" | "3")),
        };
        if !ordinary {
            continue;
        }
        if let Some(code) = row.get("Code").and_then(Value::as_str) {
            if let Ok(instrument) = normalize_symbol(market.as_str(), code) {
                if seen.insert(instrument.key()) {
                    candidates.push(instrument);
                }
            }
        }
    }
    Ok(candidates)
}

fn parse_tencent_us_candidates(text: &str) -> Result<Vec<Instrument>, String> {
    let hints: String = serde_json::from_str(
        text.trim()
            .strip_prefix("v_hint=")
            .ok_or_else(|| "美股搜索源响应无效".to_string())?
            .trim_end_matches(';'),
    )
    .map_err(|_| "美股搜索源响应无效".to_string())?;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for row in hints.split('^') {
        let mut fields = row.split('~');
        if fields.next() != Some("us") {
            continue;
        }
        let Some(code) = fields
            .next()
            .and_then(|code| code.rsplit_once('.').map(|(symbol, _)| symbol))
        else {
            continue;
        };
        if let Ok(instrument) = normalize_symbol("us", code) {
            if seen.insert(instrument.key()) {
                candidates.push(instrument);
                if candidates.len() == BATCH_SIZE {
                    break;
                }
            }
        }
    }
    Ok(candidates)
}

fn fetch_search(market: Market, q: &str) -> Result<Vec<Instrument>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(5))
        .build();
    let response = agent
        .get(SEARCH_URL)
        .query("input", q)
        .query("type", "14")
        .query("count", "20")
        .call()
        .map_err(|err| format!("搜索源请求失败: {err}"))?;
    let text = response
        .into_string()
        .map_err(|err| format!("搜索源响应无效: {err}"))?;
    let payload: Value =
        serde_json::from_str(&text).map_err(|err| format!("搜索源响应无效: {err}"))?;
    let candidates = parse_search_candidates(&payload, market)?;
    if candidates.is_empty() && market == Market::Us {
        let response = agent
            .get(US_SEARCH_URL)
            .query("q", q)
            .query("t", "all")
            .call()
            .map_err(|err| format!("美股搜索源请求失败: {err}"))?;
        let text = response
            .into_string()
            .map_err(|err| format!("美股搜索源响应无效: {err}"))?;
        return parse_tencent_us_candidates(&text);
    }
    Ok(candidates)
}

fn needs_name_search(market: Market, direct: &Option<Instrument>) -> bool {
    market == Market::Us || direct.is_none()
}

pub async fn search(db: &Db, input: SearchInput) -> Result<Value, WatchlistError> {
    let market = Market::parse(&input.market).map_err(WatchlistError::bad)?;
    let q = input.q.trim();
    if q.is_empty() || q.len() > MAX_SEARCH {
        return Err(WatchlistError::bad("搜索词长度无效"));
    }
    let cookie = crate::xueqiu::app_cookie(db)
        .await
        .map_err(WatchlistError::gateway)?;
    let direct = normalize_symbol(market.as_str(), q).ok();
    let candidates = if needs_name_search(market, &direct) {
        let suggestions = tokio::task::spawn_blocking({
            let query = q.to_string();
            move || fetch_search(market, &query)
        })
        .await
        .map_err(|error| WatchlistError::gateway(error.to_string()))?;
        match suggestions {
            Ok(items) if !items.is_empty() => items,
            Ok(_) => direct.into_iter().collect(),
            Err(_) if direct.is_some() => direct.into_iter().collect(),
            Err(error) => {
                return Err(WatchlistError::gateway(format!(
                    "名称搜索暂不可用：{error}"
                )))
            }
        }
    } else {
        direct.into_iter().collect()
    };
    if candidates.is_empty() {
        return Ok(json!({"items": []}));
    }
    let mut verified = HashMap::new();
    for batch in candidates.chunks(BATCH_SIZE) {
        let found = tokio::task::spawn_blocking({
            let cookie = cookie.clone();
            let batch = batch.to_vec();
            move || fetch_batch(&cookie, &batch)
        })
        .await
        .map_err(|err| WatchlistError::gateway(err.to_string()))?
        .map_err(WatchlistError::gateway)?;
        verified.extend(found);
    }
    let mut items = Vec::new();
    let mut shared = cache().lock().unwrap_or_else(|err| err.into_inner());
    for instrument in candidates {
        if let Some(quote) = verified.remove(&instrument.key()) {
            items.push(json!({"market": quote.instrument.market.as_str(), "symbol": quote.instrument.symbol, "name": quote.name, "currency": quote.instrument.currency()}));
            shared.entries.insert(
                instrument.key(),
                CacheEntry {
                    quote: Some(quote),
                    error: None,
                },
            );
        }
    }
    Ok(json!({"items": items}))
}

async fn claim_transition(
    db: &Db,
    quote: &Quote,
    user_id: i64,
    direction: Direction,
    target: f64,
    now: &str,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT baseline_side,last_observed_at FROM stock_price_alerts WHERE user_id=? AND market=? AND symbol=? AND direction=? AND enabled=1 AND target=?")
        .bind(user_id).bind(quote.instrument.market.as_str()).bind(&quote.instrument.symbol).bind(direction.as_str()).bind(target).fetch_optional(db.pool()).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let previous_at = row.get::<Option<String>, _>("last_observed_at");
    if previous_at
        .as_ref()
        .and_then(|at| at.parse::<i64>().ok())
        .is_some_and(|at| at >= quote.observed_at_ms)
    {
        return Ok(false);
    }
    let previous_side = row.get::<Option<i64>, _>("baseline_side");
    let current_side = baseline_side(direction, target, quote.price);
    let is_claim = previous_side.is_some_and(|side| crossed(direction, side, current_side));
    // One compare-and-update also invalidates observations racing a rule edit.
    let changed = sqlx::query("UPDATE stock_price_alerts SET baseline_side=?,last_observed_at=?,last_triggered_at=CASE WHEN ? THEN ? ELSE last_triggered_at END,delivery_status=CASE WHEN ? THEN 'pending' ELSE delivery_status END WHERE user_id=? AND market=? AND symbol=? AND direction=? AND enabled=1 AND target=? AND baseline_side IS ? AND last_observed_at IS ?")
        .bind(current_side).bind(quote.observed_at_ms.to_string()).bind(is_claim).bind(now).bind(is_claim)
        .bind(user_id).bind(quote.instrument.market.as_str()).bind(&quote.instrument.symbol).bind(direction.as_str()).bind(target)
        .bind(previous_side).bind(previous_at).execute(db.pool()).await?.rows_affected();
    Ok(is_claim && changed == 1)
}

async fn deliver_claim(db: &Db, quote: &Quote, user_id: i64, direction: Direction, target: f64) {
    let pending = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM stock_price_alerts WHERE user_id=? AND market=? AND symbol=? AND direction=? AND enabled=1 AND target=? AND last_triggered_at=? AND delivery_status='pending'")
        .bind(user_id).bind(quote.instrument.market.as_str()).bind(&quote.instrument.symbol).bind(direction.as_str()).bind(target).bind(&quote.quoted_at).fetch_one(db.pool()).await.unwrap_or(0);
    if pending != 1 {
        return;
    }
    // A notification already handed to a channel cannot be recalled by a subsequent edit.
    let text = format!(
        "{} {} {} {}价提醒：现价 {:.4}，目标 {:.4}，行情时间 {}",
        quote.instrument.market.as_str().to_ascii_uppercase(),
        quote.instrument.symbol,
        quote.instrument.currency(),
        if direction == Direction::Above {
            "上穿"
        } else {
            "下穿"
        },
        quote.price,
        target,
        quote.quoted_at
    );
    let status = match crate::push::send_user_alert(db, user_id, &text).await {
        Ok(status) => status.to_string(),
        Err(err) => {
            tracing::warn!(user_id, "自选股提醒推送失败: {err}");
            "failed".into()
        }
    };
    let _ = sqlx::query("UPDATE stock_price_alerts SET delivery_status=? WHERE user_id=? AND market=? AND symbol=? AND direction=? AND enabled=1 AND target=? AND last_triggered_at=? AND delivery_status='pending'")
        .bind(status).bind(user_id).bind(quote.instrument.market.as_str()).bind(&quote.instrument.symbol).bind(direction.as_str()).bind(target).bind(&quote.quoted_at).execute(db.pool()).await;
}

fn alertable_at(quote: &Quote, now: i64) -> bool {
    quote.observed_at_ms <= now
        && now - quote.observed_at_ms <= QUOTE_MAX_AGE_MS
        && crate::market::regular_session_open(quote.instrument.market.as_str(), now / 1000)
        && crate::market::regular_session_open(
            quote.instrument.market.as_str(),
            quote.observed_at_ms / 1000,
        )
}

async fn process_quote(db: &Db, quote: &Quote) -> Result<(), sqlx::Error> {
    if !alertable_at(quote, now_ms()) {
        return Ok(());
    }
    let rows = sqlx::query("SELECT user_id,direction,target FROM stock_price_alerts WHERE market=? AND symbol=? AND enabled=1")
        .bind(quote.instrument.market.as_str()).bind(&quote.instrument.symbol).fetch_all(db.pool()).await?;
    for row in rows {
        let user_id = row.get::<i64, _>("user_id");
        let direction = if row.get::<String, _>("direction") == "above" {
            Direction::Above
        } else {
            Direction::Below
        };
        let target = row.get::<f64, _>("target");
        if claim_transition(db, quote, user_id, direction, target, &quote.quoted_at).await? {
            deliver_claim(db, quote, user_id, direction, target).await;
        }
    }
    Ok(())
}

async fn monitor_once(db: &Db) -> Result<(), WatchlistError> {
    let instruments = all_instruments(db).await?;
    if instruments.is_empty()
        || !instruments.iter().any(|instrument| {
            crate::market::regular_session_open(instrument.market.as_str(), now_ms() / 1000)
        })
    {
        return Ok(());
    }
    refresh_cache(db).await?;
    let entries = cache()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .entries
        .values()
        .filter(|entry| entry.error.is_none())
        .filter_map(|entry| entry.quote.clone())
        .collect::<Vec<_>>();
    let now = now_ms();
    for quote in entries {
        if alertable_at(&quote, now) {
            process_quote(db, &quote)
                .await
                .map_err(WatchlistError::db)?;
        }
    }
    Ok(())
}

pub fn spawn(db: Db) {
    if matches!(
        std::env::var("VPUSH_FETCH").ok().as_deref(),
        Some("0" | "false" | "False")
    ) {
        return;
    }
    tokio::spawn(async move {
        loop {
            if let Err(err) = monitor_once(&db).await {
                tracing::warn!("自选股行情监控: {}", err.detail);
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_quote(market: &str, symbol: &str, price: f64, at: i64) -> Quote {
        let instrument = normalize_symbol(market, symbol).unwrap();
        let quote = Quote {
            instrument: instrument.clone(),
            name: "fixture".into(),
            price,
            percent: Some(0.0),
            quoted_at: at.to_string(),
            observed_at_ms: at,
            tick_size: Some(0.01),
            variable_tick_size: None,
        };
        cache().lock().unwrap().entries.insert(
            instrument.key(),
            CacheEntry {
                quote: Some(quote.clone()),
                error: None,
            },
        );
        quote
    }

    #[tokio::test]
    async fn watchlist_is_user_scoped_and_alerts_cascade() {
        let db = Db::open(std::path::Path::new(":memory:")).await.unwrap();
        let first: i64 =
            sqlx::query("INSERT INTO users (username,password_hash) VALUES ('watch-one','')")
                .execute(db.pool())
                .await
                .unwrap()
                .last_insert_rowid();
        let second: i64 =
            sqlx::query("INSERT INTO users (username,password_hash) VALUES ('watch-two','')")
                .execute(db.pool())
                .await
                .unwrap()
                .last_insert_rowid();
        insert_watchlist(&db, first, normalize_symbol("cn", "600519").unwrap())
            .await
            .unwrap();
        insert_watchlist(&db, second, normalize_symbol("cn", "600519").unwrap())
            .await
            .unwrap();
        fixture_quote("cn", "600519", 1400.0, now_ms());
        let alerts = AlertsInput {
            above: AlertInput {
                target: Some(1500.0),
                enabled: true,
            },
            below: AlertInput {
                target: Some(1200.0),
                enabled: true,
            },
        };
        save_alerts(&db, first, "cn", "600519", alerts)
            .await
            .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stock_price_alerts WHERE user_id=?")
                .bind(first)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let other: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stock_price_alerts WHERE user_id=?")
                .bind(second)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 2);
        assert_eq!(other, 0);
        remove(&db, first, "cn", "600519").await.unwrap();
        let cascaded: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stock_price_alerts WHERE user_id=?")
                .bind(first)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let other_watch: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stock_watchlist WHERE user_id=?")
                .bind(second)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(cascaded, 0);
        assert_eq!(other_watch, 1);
    }

    #[tokio::test]
    async fn concurrent_claims_are_monotonic_and_single_winner() {
        let db = Db::open(std::path::Path::new(":memory:")).await.unwrap();
        let user: i64 =
            sqlx::query("INSERT INTO users (username,password_hash) VALUES ('claim-user','')")
                .execute(db.pool())
                .await
                .unwrap()
                .last_insert_rowid();
        let instrument = normalize_symbol("us", "AAPL").unwrap();
        fixture_quote("us", "AAPL", 99.0, now_ms());
        insert_watchlist(&db, user, instrument.clone())
            .await
            .unwrap();
        save_alerts(
            &db,
            user,
            "us",
            "AAPL",
            AlertsInput {
                above: AlertInput {
                    target: Some(100.0),
                    enabled: true,
                },
                below: AlertInput {
                    target: None,
                    enabled: false,
                },
            },
        )
        .await
        .unwrap();
        let quote = Quote {
            instrument,
            name: "Apple".into(),
            price: 99.0,
            percent: None,
            quoted_at: "2026-10-01T10:00:00-04:00".into(),
            observed_at_ms: 1_800_000_000_000,
            tick_size: Some(0.01),
            variable_tick_size: None,
        };
        assert!(
            !claim_transition(&db, &quote, user, Direction::Above, 100.0, "baseline")
                .await
                .unwrap()
        );
        let mut crossing = quote.clone();
        crossing.price = 101.0;
        crossing.observed_at_ms += 60_000;
        let (first, duplicate) = tokio::join!(
            claim_transition(&db, &crossing, user, Direction::Above, 100.0, "trigger"),
            claim_transition(&db, &crossing, user, Direction::Above, 100.0, "trigger")
        );
        assert_eq!(
            usize::from(first.unwrap()) + usize::from(duplicate.unwrap()),
            1
        );
        assert!(
            !claim_transition(&db, &quote, user, Direction::Above, 100.0, "old")
                .await
                .unwrap()
        );
        save_alerts(
            &db,
            user,
            "us",
            "AAPL",
            AlertsInput {
                above: AlertInput {
                    target: Some(110.0),
                    enabled: true,
                },
                below: AlertInput {
                    target: None,
                    enabled: false,
                },
            },
        )
        .await
        .unwrap();
        assert!(
            !claim_transition(&db, &crossing, user, Direction::Above, 100.0, "old-rule")
                .await
                .unwrap()
        );
        assert!(!claim_transition(
            &db,
            &crossing,
            user,
            Direction::Above,
            110.0,
            "new-baseline"
        )
        .await
        .unwrap());
        crossing.price = 111.0;
        crossing.observed_at_ms += 60_000;
        assert!(
            claim_transition(&db, &crossing, user, Direction::Above, 110.0, "new-trigger")
                .await
                .unwrap()
        );
    }

    #[test]
    fn normalizes_supported_equity_symbols_and_rejects_funds() {
        assert_eq!(
            normalize_symbol("cn", "sh.600000").unwrap().xueqiu,
            "SH600000"
        );
        assert_eq!(normalize_symbol("hk", "700").unwrap().symbol, "00700");
        assert_eq!(normalize_symbol("us", "aapl").unwrap().symbol, "AAPL");
        assert!(normalize_symbol("cn", "513100").is_err());
        assert!(normalize_symbol("hk", "700.W").is_err());
    }

    #[test]
    fn parses_tencent_us_name_fallback() {
        let text = r#"v_hint="us~tsla.oq~Tesla~tesla~GP^us~brk.b.n~Berkshire~brk~GP^hk~00700~Tencent~tx~GP""#;
        let found = parse_tencent_us_candidates(text).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|item| item.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["TSLA", "BRK.B"]
        );
        assert!(parse_tencent_us_candidates("not JSONP").is_err());
    }

    #[test]
    fn us_words_use_name_suggestions_even_when_they_look_like_symbols() {
        let apple = normalize_symbol("us", "Apple").ok();
        assert_eq!(apple.as_ref().unwrap().symbol, "APPLE");
        assert!(needs_name_search(Market::Us, &apple));
        let cn = normalize_symbol("cn", "600519").ok();
        assert!(!needs_name_search(Market::Cn, &cn));
    }

    #[test]
    fn parses_eastmoney_search_candidates_by_market() {
        let payload = json!({"QuotationCodeTable":{"Status":0,"Data":[
            {"Code":"600519","Classify":"AStock"},
            {"Code":"510300","Classify":"AStock"},
            {"Code":"00700","Classify":"HK","TypeUS":"3"},
            {"Code":"13005","Classify":"HK","TypeUS":"6"},
            {"Code":"AAPL","Classify":"UsStock","TypeUS":"1"},
            {"Code":"TME","Classify":"UsStock","TypeUS":"3"},
            {"Code":"AAPL24","Classify":"UsStock","TypeUS":"6"},
            {"Code":"AAPX","Classify":"UsStock","TypeUS":"5"},
            {"Code":"600519","Classify":"AStock"},
            {"Code":"123","Classify":"AStock"}
        ]}});
        for (market, symbols) in [
            (Market::Cn, vec!["600519"]),
            (Market::Hk, vec!["00700"]),
            (Market::Us, vec!["AAPL", "TME"]),
        ] {
            let found = parse_search_candidates(&payload, market).unwrap();
            assert_eq!(
                found
                    .iter()
                    .map(|item| item.symbol.as_str())
                    .collect::<Vec<_>>(),
                symbols
            );
        }
        assert!(parse_search_candidates(
            &json!({"QuotationCodeTable":{"Status":1,"Data":[]}}),
            Market::Cn
        )
        .is_err());
        assert!(
            parse_search_candidates(&json!({"QuotationCodeTable":{"Status":0}}), Market::Cn)
                .is_err()
        );
    }

    #[test]
    fn parses_mixed_batch_and_rejects_bad_freshness() {
        let now = 1_800_000_000_i64;
        let payload = json!({"data":{"items":[
            {"quote":{"symbol":"SH600000","type":11,"currency":"CNY","name":"浦发银行","current":10.2,"percent":1.0,"timestamp":now * 1000}},
            {"quote":{"symbol":"00700","type":30,"currency":"HKD","name":"腾讯控股","current":500.0,"percent":-2.0,"timestamp":now * 1000}},
            {"quote":{"symbol":"AAPL","type":0,"currency":"USD","name":"Apple","current":200.0,"percent":0.5,"timestamp":now * 1000}}
        ]}});
        let wanted = vec![
            normalize_symbol("cn", "600000").unwrap(),
            normalize_symbol("hk", "700").unwrap(),
            normalize_symbol("us", "aapl").unwrap(),
        ];
        let quotes = parse_batch_quotes(&payload, &wanted, now * 1000).unwrap();
        assert_eq!(quotes.len(), 3);
        assert_eq!(quotes[&wanted[1].key()].instrument.currency(), "HKD");
        assert!(parse_timestamp(now * 1000 + 1_000).unwrap() > now * 1000);
        let old = json!({"data":{"items":[{"quote":{"symbol":"SH600000","type":11,"currency":"CNY","current":10.2,"timestamp":(now - 121) * 1000}}]}});
        let historical = parse_batch_quotes(&old, &wanted[..1], now * 1000).unwrap();
        assert!(!alertable_at(&historical[&wanted[0].key()], now * 1000));
    }

    #[test]
    fn crossing_is_directional_and_rearms() {
        let side = |direction, price| baseline_side(direction, 10.0, price);
        assert!(!crossed(
            Direction::Above,
            side(Direction::Above, 9.0),
            side(Direction::Above, 9.5)
        ));
        assert!(crossed(
            Direction::Above,
            side(Direction::Above, 9.0),
            side(Direction::Above, 10.0)
        ));
        assert!(!crossed(
            Direction::Above,
            side(Direction::Above, 10.0),
            side(Direction::Above, 11.0)
        ));
        assert!(crossed(
            Direction::Below,
            side(Direction::Below, 11.0),
            side(Direction::Below, 10.0)
        ));
        assert_eq!(baseline_side(Direction::Above, 10.0, 9.0), -1);
        assert_eq!(baseline_side(Direction::Below, 10.0, 11.0), 1);
    }

    #[tokio::test]
    async fn both_directions_rearm_without_initial_or_repeat_alerts() {
        let db = Db::open(std::path::Path::new(":memory:")).await.unwrap();
        let user =
            sqlx::query("INSERT INTO users (username,password_hash) VALUES ('two-directions','')")
                .execute(db.pool())
                .await
                .unwrap()
                .last_insert_rowid();
        let mut quote = fixture_quote("us", "NVDA", 130.0, now_ms());
        insert_watchlist(&db, user, quote.instrument.clone())
            .await
            .unwrap();
        save_alerts(
            &db,
            user,
            "us",
            "NVDA",
            AlertsInput {
                above: AlertInput {
                    target: Some(120.0),
                    enabled: true,
                },
                below: AlertInput {
                    target: Some(100.0),
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
        for (price, up, down) in [
            (130.0, false, false),
            (119.0, false, false),
            (120.0, true, false),
            (130.0, false, false),
            (110.0, false, false),
            (100.0, false, true),
            (99.0, false, false),
            (101.0, false, false),
            (100.0, false, true),
        ] {
            quote.price = price;
            quote.observed_at_ms += 1;
            assert_eq!(
                claim_transition(&db, &quote, user, Direction::Above, 120.0, "fixture")
                    .await
                    .unwrap(),
                up
            );
            assert_eq!(
                claim_transition(&db, &quote, user, Direction::Below, 100.0, "fixture")
                    .await
                    .unwrap(),
                down
            );
        }
    }

    #[test]
    fn quote_response_classifies_transport_and_provider_errors() {
        for (status, body, expected) in [
            (200, "", "空响应"),
            (200, "{broken", "非 JSON"),
            (401, "", "401"),
            (403, "", "403"),
            (429, "", "429"),
            (200, "<html>captcha</html>", "挑战页"),
            (200, r#"{"error_code":110017}"#, "限流"),
            (200, r#"{"error_code":"110017"}"#, "限流"),
            (200, r#"{"error_code":400016}"#, "身份"),
            (200, r#"{"error_code":"invalid"}"#, "错误码格式无效"),
        ] {
            assert!(parse_quote_response(status, body, &[], now_ms())
                .unwrap_err()
                .contains(expected));
        }
    }

    #[test]
    fn invalid_quote_metadata_cannot_become_alertable() {
        let now = crate::market::mainland_quote_time("20260930100000")
            .unwrap()
            .1
            * 1000;
        let wanted = vec![normalize_symbol("cn", "600000").unwrap()];
        let valid =
            json!({"symbol":"SH600000","type":11,"currency":"CNY","current":10.0,"timestamp":now});
        let parse = |quote: Value| {
            parse_batch_quotes(&json!({"data":{"items":[{"quote":quote}]}}), &wanted, now)
        };
        let quote = parse(valid.clone()).unwrap().remove("cn:600000").unwrap();
        assert!(alertable_at(&quote, now));
        assert!(alertable_at(&quote, now + 120_000));
        assert!(!alertable_at(&quote, now + 120_001));
        assert!(!alertable_at(&quote, now - 1));
        for field in ["symbol", "type", "currency", "current", "timestamp"] {
            let mut broken = valid.clone();
            broken.as_object_mut().unwrap().remove(field);
            assert!(parse(broken).is_err(), "missing {field}");
        }
        for (field, bad) in [
            ("type", json!(13)),
            ("currency", json!("USD")),
            ("current", json!(-1)),
            ("current", json!("NaN")),
            ("timestamp", json!(now / 1000)),
            ("timestamp", json!(now + 1)),
            ("timestamp", json!("nonsense")),
            ("symbol", json!("SH600001")),
        ] {
            let mut broken = valid.clone();
            broken[field] = bad;
            assert!(parse(broken).is_err(), "invalid {field}");
        }
        let lunch = crate::market::mainland_quote_time("20260930120000")
            .unwrap()
            .1
            * 1000;
        let mut at_lunch = quote.clone();
        at_lunch.observed_at_ms = lunch;
        assert!(!alertable_at(&at_lunch, lunch));
        let unknown_calendar = crate::market::mainland_quote_time("20270104100000")
            .unwrap()
            .1;
        assert!(!crate::market::exchange_calendar_known(
            "cn",
            unknown_calendar
        ));
    }

    #[tokio::test]
    async fn delivery_is_suppressed_when_disabled_and_failed_without_channels() {
        let db = Db::open(std::path::Path::new(":memory:")).await.unwrap();
        let user = sqlx::query("INSERT INTO users (username,password_hash,notify_enabled) VALUES ('delivery-user','',0)")
            .execute(db.pool()).await.unwrap().last_insert_rowid();
        let mut quote = fixture_quote("us", "MSFT", 99.0, now_ms());
        insert_watchlist(&db, user, quote.instrument.clone())
            .await
            .unwrap();
        save_alerts(
            &db,
            user,
            "us",
            "MSFT",
            AlertsInput {
                above: AlertInput {
                    target: Some(100.0),
                    enabled: true,
                },
                below: AlertInput {
                    target: None,
                    enabled: false,
                },
            },
        )
        .await
        .unwrap();
        assert!(
            !claim_transition(&db, &quote, user, Direction::Above, 100.0, &quote.quoted_at)
                .await
                .unwrap()
        );
        quote.price = 100.0;
        quote.observed_at_ms += 1;
        quote.quoted_at = quote.observed_at_ms.to_string();
        assert!(
            claim_transition(&db, &quote, user, Direction::Above, 100.0, &quote.quoted_at)
                .await
                .unwrap()
        );
        deliver_claim(&db, &quote, user, Direction::Above, 100.0).await;
        let status: String =
            sqlx::query_scalar("SELECT delivery_status FROM stock_price_alerts WHERE user_id=?")
                .bind(user)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(status, "suppressed");
        quote.price = 99.0;
        quote.observed_at_ms += 1;
        assert!(
            !claim_transition(&db, &quote, user, Direction::Above, 100.0, "rearm")
                .await
                .unwrap()
        );
        sqlx::query("UPDATE users SET notify_enabled=1,dnd_start='',dnd_end='' WHERE id=?")
            .bind(user)
            .execute(db.pool())
            .await
            .unwrap();
        quote.price = 101.0;
        quote.observed_at_ms += 1;
        quote.quoted_at = quote.observed_at_ms.to_string();
        assert!(
            claim_transition(&db, &quote, user, Direction::Above, 100.0, &quote.quoted_at)
                .await
                .unwrap()
        );
        deliver_claim(&db, &quote, user, Direction::Above, 100.0).await;
        let status: String =
            sqlx::query_scalar("SELECT delivery_status FROM stock_price_alerts WHERE user_id=?")
                .bind(user)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(status, "failed");
        assert!(
            !claim_transition(&db, &quote, user, Direction::Above, 100.0, "no-retry")
                .await
                .unwrap()
        );
        assert!(save_alerts(
            &db,
            user + 100,
            "us",
            "MSFT",
            AlertsInput {
                above: AlertInput {
                    target: Some(100.0),
                    enabled: true
                },
                below: AlertInput {
                    target: None,
                    enabled: false
                },
            }
        )
        .await
        .is_err());
    }

    #[test]
    fn uses_target_band_variable_ticks() {
        let bands = "0.001 0.25 0.005 10 0.01 20.00 0.02 50 0.05 100.00 0.1 200.00 0.2 500.00 0.5 1000.00 1.00 2000.00 2.00 5000.00 5.00";
        assert_eq!(variable_tick(bands, 0.249), Some(0.001));
        assert_eq!(variable_tick(bands, 431.0), Some(0.2));
        assert!(valid_target_for(Market::Hk, 0.249, None, Some(bands)));
        assert!(!valid_target_for(Market::Hk, 0.2495, None, Some(bands)));
        assert_eq!(variable_tick("0.0001 1 0.01", 0.9), Some(0.0001));
    }

    #[test]
    fn holidays_are_not_regular_sessions() {
        let oct_first_2026 = crate::market::mainland_quote_time("20261001100000")
            .unwrap()
            .1;
        assert!(!crate::market::regular_session_open("cn", oct_first_2026));
        assert!(!crate::market::regular_session_open("hk", oct_first_2026));
    }

    #[test]
    fn validates_market_ticks() {
        assert!(valid_target("cn", 10.05));
        assert!(valid_target("cn", 10.01));
        assert!(valid_target("cn", 1258.62));
        assert!(!valid_target("cn", 10.001));
        assert!(!valid_target("hk", 15.02)); // Requires verified security-specific bands.
        assert!(valid_target("us", 10.01));
        assert!(valid_target("us", 0.001));
        assert!(!valid_target("us", 0.00001));
    }
}
