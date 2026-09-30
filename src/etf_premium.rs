//! ETF IOPV 溢价行情与用户阈值提醒。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::db::Db;
use serde_json::{json, Value};
use sqlx::Row;

const FUNDS: &[(&str, &str)] = &[("513100", "纳指ETF国泰"), ("513500", "标普500ETF博时")];

fn fund_name(symbol: &str) -> Option<&'static str> {
    FUNDS
        .iter()
        .find(|(code, _)| *code == symbol)
        .map(|(_, name)| *name)
}

fn premium(price: f64, reference: f64) -> Option<f64> {
    if !price.is_finite() || !reference.is_finite() || price <= 0.0 || reference <= 0.0 {
        return None;
    }
    let rate = (price / reference - 1.0) * 100.0;
    rate.is_finite().then_some(rate)
}

pub fn valid_setting(symbol: &str, threshold: f64) -> bool {
    fund_name(symbol).is_some() && threshold.is_finite() && (0.0..=100.0).contains(&threshold)
}

pub async fn settings(db: &Db, user_id: i64) -> Result<Value, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT symbol, enabled, threshold_pct FROM etf_premium_alerts WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(db.pool())
    .await?;
    let items = FUNDS
        .iter()
        .map(|(symbol, name)| {
            let row = rows
                .iter()
                .find(|row| row.get::<String, _>("symbol") == *symbol);
            json!({"symbol": symbol, "name": name,
            "enabled": row.is_some_and(|row| row.get::<bool, _>("enabled")),
            "threshold_pct": row.map(|row| row.get::<f64, _>("threshold_pct")).unwrap_or(5.0)})
        })
        .collect::<Vec<_>>();
    let enabled: bool = sqlx::query_scalar("SELECT notify_enabled FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(db.pool())
        .await?;
    Ok(json!({"items": items, "notify_enabled": enabled}))
}

pub async fn save_setting(
    db: &Db,
    user_id: i64,
    symbol: &str,
    enabled: bool,
    threshold: f64,
) -> Result<Value, sqlx::Error> {
    sqlx::query("INSERT INTO etf_premium_alerts (user_id, symbol, enabled, threshold_pct)
        VALUES (?, ?, ?, ?) ON CONFLICT(user_id, symbol) DO UPDATE SET
        above_threshold = CASE WHEN enabled != excluded.enabled OR threshold_pct != excluded.threshold_pct
            THEN 0 ELSE above_threshold END,
        enabled = excluded.enabled, threshold_pct = excluded.threshold_pct")
        .bind(user_id).bind(symbol).bind(enabled).bind(threshold).execute(db.pool()).await?;
    Ok(
        json!({"symbol": symbol, "name": fund_name(symbol), "enabled": enabled, "threshold_pct": threshold}),
    )
}

// SQLite 的条件更新同时保存触发快照和去重状态，重启不会重复提醒。
async fn claim(
    db: &Db,
    user_id: i64,
    symbol: &str,
    rate: f64,
    at: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query(
        "UPDATE etf_premium_alerts SET above_threshold = 0
        WHERE user_id = ? AND symbol = ? AND threshold_pct > ?",
    )
    .bind(user_id)
    .bind(symbol)
    .bind(rate)
    .execute(db.pool())
    .await?;
    let result = sqlx::query("UPDATE etf_premium_alerts SET above_threshold = 1,
        last_triggered_pct = ?, last_threshold_pct = threshold_pct, last_triggered_at = ?, delivery_status = 'pending'
        WHERE user_id = ? AND symbol = ? AND enabled = 1 AND above_threshold = 0 AND threshold_pct <= ?")
        .bind(rate).bind(at).bind(user_id).bind(symbol).bind(rate).execute(db.pool()).await?;
    Ok(result.rows_affected() == 1)
}

async fn mark_delivery(
    db: &Db,
    user_id: i64,
    symbol: &str,
    at: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE etf_premium_alerts SET delivery_status = ? WHERE user_id = ? AND symbol = ? AND last_triggered_at = ?")
        .bind(status).bind(user_id).bind(symbol).bind(at).execute(db.pool()).await?;
    Ok(())
}

pub async fn latest_alert(db: &Db, user_id: i64) -> Result<Option<Value>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT symbol, last_triggered_pct, last_threshold_pct, last_triggered_at, delivery_status
        FROM etf_premium_alerts WHERE user_id = ? AND last_triggered_at IS NOT NULL
        ORDER BY last_triggered_at DESC, symbol LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.map(|row| {
        let symbol: String = row.get("symbol");
        json!({"symbol": symbol, "name": fund_name(&symbol),
            "triggered_pct": row.get::<f64, _>("last_triggered_pct"),
            "threshold_pct": row.get::<f64, _>("last_threshold_pct"),
            "triggered_at": row.get::<String, _>("last_triggered_at"),
            "delivery_status": row.get::<String, _>("delivery_status")})
    }))
}

#[derive(Clone, Debug)]
struct Quote {
    symbol: &'static str,
    price: Option<f64>,
    iopv: Option<f64>,
    premium_rate: Option<f64>,
    market_at: Option<String>,
    market_unix: Option<i64>,
    reference_at: Option<String>,
    reference_unix: Option<i64>,
}

impl Quote {
    fn fresh(&self, now: i64) -> bool {
        [self.market_unix, self.reference_unix]
            .iter()
            .all(|at| at.is_some_and(|at| (0..=120).contains(&(now - at))))
    }

    fn alertable(&self, _now: i64) -> bool {
        false
    }

    fn render(&self, now: i64, fetch_ok: bool) -> Value {
        let rate = self
            .premium_rate
            .or_else(|| self.price.zip(self.iopv).and_then(|(p, v)| premium(p, v)));
        let status = if !fetch_ok || rate.is_none() {
            "unavailable"
        } else if !crate::market::mainland_open(now) {
            "closed"
        } else if !self.fresh(now) {
            "stale"
        } else {
            "reference_only"
        };
        json!({"symbol": self.symbol, "name": fund_name(self.symbol), "market_price": self.price,
            "reference_value": self.iopv, "premium_rate": rate,
            "reference_type": if self.iopv.is_some() { "iopv_unverified_realtime" } else { "unavailable" },
            "market_at": self.market_at, "reference_at": self.reference_at,
            "stale": !fetch_ok || !self.fresh(now), "status": status, "alerts_enabled": false,
            "source": "雪球批量行情（IOPV实时性待盘中验证）"})
    }
}

fn xueqiu_time(time: &str, timestamp: i64) -> Result<(String, i64), String> {
    let compact = time.replace(['-', ' ', ':'], "");
    let Some((at, unix)) = crate::market::mainland_quote_time(&compact) else {
        return Err("雪球行情时间非法".into());
    };
    if timestamp <= 0 || timestamp / 1000 != unix {
        return Err("雪球行情时间戳不一致".into());
    }
    Ok((at, unix))
}

fn finite_value(value: &Value) -> Option<f64> {
    let value = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    value.is_finite().then_some(value)
}

fn positive_value(value: &Value) -> Option<f64> {
    finite_value(value).filter(|value| *value > 0.0)
}

fn parse_xueqiu_quotes(value: &Value) -> Result<Vec<Quote>, String> {
    let items = value
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
        .ok_or_else(|| "雪球行情缺少 data.items".to_string())?;
    let mut quotes = Vec::with_capacity(FUNDS.len());
    for (symbol, _) in FUNDS {
        let target = format!("SH{symbol}");
        let quote = items
            .iter()
            .filter_map(|item| item.get("quote"))
            .find(|quote| quote.get("symbol").and_then(Value::as_str) == Some(target.as_str()))
            .ok_or_else(|| format!("雪球行情缺少 {symbol}"))?;
        let timestamp = quote
            .get("timestamp")
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("雪球行情 {symbol} 缺少时间戳"))?;
        let time = quote
            .get("time")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("雪球行情 {symbol} 缺少报价时间"))?;
        let (market_at, market_unix) = xueqiu_time(time, timestamp)?;
        quotes.push(Quote {
            symbol,
            price: positive_value(&quote["current"]),
            iopv: positive_value(&quote["iopv"]),
            premium_rate: finite_value(&quote["premium_rate"]),
            market_at: Some(market_at.clone()),
            market_unix: Some(market_unix),
            reference_at: Some(market_at),
            reference_unix: Some(market_unix),
        });
    }
    Ok(quotes)
}

fn decode_xueqiu_response(status: u16, text: &str) -> Result<Value, String> {
    if text.contains("EO_Bot_Ssid")
        || text.contains("__tst_status")
        || text.contains("aliyun_waf")
        || text.contains("挑战")
    {
        return Err("雪球行情返回挑战页".into());
    }
    if status == 401 || status == 403 {
        return Err(format!("雪球行情身份/权限错误 HTTP {status}"));
    }
    if status == 429 {
        return Err("雪球行情限流 HTTP 429".into());
    }
    if text.trim().is_empty() {
        return Err("雪球行情返回空响应".into());
    }
    let value: Value = serde_json::from_str(text).map_err(|_| "雪球行情返回非 JSON".to_string())?;
    let error_code = value
        .get("error_code")
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("error_code")
                .and_then(Value::as_str)?
                .parse()
                .ok()
        })
        .unwrap_or(0);
    if error_code == 110017 {
        return Err("雪球行情限流 110017".into());
    }
    if [400016, 10022, 400012, 400013, 70007, 20250, 20251].contains(&error_code) {
        return Err(format!("雪球行情身份失效 {error_code}"));
    }
    if status != 200 || error_code != 0 {
        return Err(format!("雪球行情 HTTP {status} 错误码 {error_code}"));
    }
    Ok(value)
}

fn fetch_xueqiu_quotes(cookie: &str) -> Result<Vec<Quote>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(8))
        .build();
    let response = agent
        .get("https://stock.xueqiu.com/v5/stock/batch/quote.json")
        .query("symbol", "SH513100,SH513500")
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
        Err(ureq::Error::Status(status, response)) => (
            status,
            response.into_string().map_err(|err| err.to_string())?,
        ),
        Err(err) => return Err(format!("雪球行情请求失败: {err}")),
    };
    let value = decode_xueqiu_response(status, &text)?;
    parse_xueqiu_quotes(&value)
}

#[derive(Default)]
struct Cache {
    items: Vec<Quote>,
    at: Option<Instant>,
    fetch_ok: bool,
}

fn cache() -> &'static tokio::sync::Mutex<Cache> {
    static CACHE: OnceLock<tokio::sync::Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(Cache::default()))
}

async fn quotes(db: &Db) -> (Vec<Quote>, bool) {
    let mut cache = cache().lock().await;
    if cache
        .at
        .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
    {
        let result = match crate::xueqiu::app_cookie(db).await {
            Ok(cookie) => tokio::task::spawn_blocking(move || fetch_xueqiu_quotes(&cookie))
                .await
                .map_err(|err| err.to_string())
                .and_then(|result| result),
            Err(err) => Err(err),
        };
        cache.fetch_ok = result.is_ok();
        match result {
            Ok(items) => cache.items = items,
            Err(err) => tracing::warn!("雪球 ETF 行情获取失败: {err}"),
        }
        cache.at = Some(Instant::now());
    }
    (cache.items.clone(), cache.fetch_ok)
}

pub async fn snapshot(db: &Db) -> Value {
    let (items, ok) = quotes(db).await;
    let now = crate::market::now_unix();
    json!({"items": items.iter().map(|quote| quote.render(now, ok)).collect::<Vec<_>>()})
}

async fn evaluate(db: &Db, quote: &Quote, now: i64) -> Result<(), sqlx::Error> {
    // IOPV 的实际计算时间尚未独立验证，参考值不具备可靠实时提醒条件。
    if !quote.alertable(now) {
        return Ok(());
    }
    let rate = premium(quote.price.unwrap(), quote.iopv.unwrap()).unwrap();
    let users: Vec<i64> = sqlx::query_scalar(
        "SELECT user_id FROM etf_premium_alerts WHERE symbol = ? AND enabled = 1",
    )
    .bind(quote.symbol)
    .fetch_all(db.pool())
    .await?;
    let started = Instant::now();
    // ponytail: 单实例顺序投递；用户规模增大时复用现有推送并发上限。
    for user_id in users {
        if !quote.alertable(now + started.elapsed().as_secs() as i64) {
            break;
        }
        let at = quote.market_at.as_deref().unwrap();
        if !claim(db, user_id, quote.symbol, rate, at).await? {
            continue;
        }
        let threshold: f64 = sqlx::query_scalar(
            "SELECT last_threshold_pct FROM etf_premium_alerts WHERE user_id = ? AND symbol = ?",
        )
        .bind(user_id)
        .bind(quote.symbol)
        .fetch_one(db.pool())
        .await?;
        let text = format!("【ETF高溢价提醒】\n\n{} {}\n当前溢价率：{rate:.2}%\n提醒水位：{threshold:.2}%\n触发时间：{}\n\n参考价值：雪球行情IOPV（{:.4}）\nIOPV更新时间：{}",
            quote.symbol, fund_name(quote.symbol).unwrap(), at.replace('T', " ").replace("+08:00", ""), quote.iopv.unwrap(),
            quote.reference_at.as_deref().unwrap().replace('T', " ").replace("+08:00", ""));
        let status = match crate::push::send_user_alert(db, user_id, &text).await {
            Ok(status) => status,
            Err(err) => {
                tracing::warn!(user_id, symbol = quote.symbol, "ETF提醒投递失败: {err}");
                "failed"
            }
        };
        mark_delivery(db, user_id, quote.symbol, at, status).await?;
    }
    Ok(())
}

pub fn spawn(db: Db) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(30));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let now = crate::market::now_unix();
            if !crate::market::mainland_open(now) {
                continue;
            }
            let (quotes, ok) = quotes(&db).await;
            if !ok {
                continue;
            }
            for quote in quotes {
                if let Err(err) = evaluate(&db, &quote, crate::market::now_unix()).await {
                    tracing::warn!(symbol = quote.symbol, "ETF阈值检查失败: {err}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_quote() -> Quote {
        parse_xueqiu_quotes(&xueqiu_payload(json!(1_790_751_600_000_i64)))
            .unwrap()
            .into_iter()
            .find(|quote| quote.symbol == "513100")
            .unwrap()
    }

    fn xueqiu_payload(timestamp: Value) -> Value {
        json!({"data":{"items":[
            {"quote":{"symbol":"SH513500","current":2.688,"iopv":2.447,"premium_rate":9.85,"time":"2026-09-30 15:00:00","timestamp":timestamp}},
            {"quote":{"symbol":"SH513100","current":2.352,"iopv":2.0381,"premium_rate":15.4,"time":"2026-09-30 15:00:00","timestamp":timestamp}}
        ]}})
    }

    #[test]
    fn xueqiu_batch_quotes_parse_price_iopv_premium_and_timestamp() {
        let quotes = parse_xueqiu_quotes(&xueqiu_payload(json!(1_790_751_600_000_i64))).unwrap();
        assert_eq!(
            quotes.iter().map(|q| q.symbol).collect::<Vec<_>>(),
            ["513100", "513500"]
        );
        assert_eq!(quotes[0].price, Some(2.352));
        assert_eq!(quotes[0].iopv, Some(2.0381));
        assert_eq!(quotes[0].premium_rate, Some(15.4));
        assert!(
            (premium(quotes[0].price.unwrap(), quotes[0].iopv.unwrap()).unwrap() - 15.4016).abs()
                < 0.001
        );
        assert_eq!(
            quotes[0].market_at.as_deref(),
            Some("2026-09-30T15:00:00+08:00")
        );
        assert_eq!(quotes[0].market_unix, Some(1_790_751_600));
    }

    #[test]
    fn xueqiu_batch_quotes_reject_missing_or_invalid_timestamps() {
        for timestamp in [Value::Null, json!("bad"), json!(0), json!(-1)] {
            assert!(parse_xueqiu_quotes(&xueqiu_payload(timestamp))
                .unwrap_err()
                .contains("时间戳"));
        }
    }

    #[test]
    fn xueqiu_response_errors_are_classified() {
        for (status, body, message) in [
            (200, "", "空响应"),
            (200, "<html>no json</html>", "非 JSON"),
            (401, "{}", "身份/权限"),
            (403, "{}", "身份/权限"),
            (429, "{}", "限流"),
            (403, "EO_Bot_Ssid challenge", "挑战页"),
            (
                400,
                r#"{"error_code":400016,"error_description":"identity mismatch"}"#,
                "身份失效",
            ),
        ] {
            assert!(decode_xueqiu_response(status, body)
                .unwrap_err()
                .contains(message));
        }
    }

    #[test]
    fn source_snapshot_and_sessions_guard_alerts() {
        let quote = sample_quote();
        let now = quote.market_unix.unwrap();
        assert!(
            (premium(quote.price.unwrap(), quote.iopv.unwrap()).unwrap() - 15.4015995).abs()
                < 0.00001
        );
        assert!(!quote.alertable(now));
        assert!(!quote.alertable(now + 121));
        assert!(!quote.alertable(now - 1));
        assert_eq!(quote.render(now, false)["status"], "unavailable");
        assert_eq!(quote.render(now, true)["status"], "reference_only");
        let mut stale_iopv = quote.clone();
        stale_iopv.reference_unix = Some(now - 121);
        assert!(!stale_iopv.alertable(now));
        stale_iopv.reference_unix = None;
        assert!(!stale_iopv.alertable(now));
    }

    #[tokio::test]
    async fn stale_quotes_never_claim_and_unbound_pushes_are_not_sent() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        db.ensure_admin("hash").await.unwrap();
        let user = db.user_by_username("admin").await.unwrap().unwrap();
        save_setting(&db, user.id, "513100", true, 5.0)
            .await
            .unwrap();
        let quote = sample_quote();
        let now = quote.market_unix.unwrap();
        evaluate(&db, &quote, now + 121).await.unwrap();
        assert!(latest_alert(&db, user.id).await.unwrap().is_none());
        evaluate(&db, &quote, now).await.unwrap();
        assert!(latest_alert(&db, user.id).await.unwrap().is_none());
        save_setting(&db, user.id, "513100", true, 6.0)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET notify_enabled = 0 WHERE id = ?")
            .bind(user.id)
            .execute(db.pool())
            .await
            .unwrap();
        evaluate(&db, &quote, now).await.unwrap();
        assert!(latest_alert(&db, user.id).await.unwrap().is_none());
        assert_eq!(
            crate::push::send_user_alert(&db, user.id, "test")
                .await
                .unwrap(),
            "suppressed"
        );
    }

    #[tokio::test]
    #[ignore = "访问雪球公网，手动验证行情源"]
    async fn public_iopv_snapshot_has_both_funds_and_independent_times() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        let (quotes, ok) = quotes(&db).await;
        assert!(ok);
        assert_eq!(quotes.len(), 2);
        for quote in quotes {
            assert!(quote.market_at.is_some());
            assert!(quote.reference_at.is_some());
            assert!(premium(quote.price.unwrap(), quote.iopv.unwrap()).is_some());
        }
    }

    #[test]
    fn premium_requires_positive_finite_values() {
        assert!((premium(1.1, 1.0).unwrap() - 10.0).abs() < 1e-10);
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(premium(1.1, value), None);
            assert_eq!(premium(value, 1.0), None);
        }
        assert!(valid_setting("513100", 13.58));
        assert!(!valid_setting("unknown", 5.0));
        for value in [-0.01, 100.01, f64::NAN] {
            assert!(!valid_setting("513500", value));
        }
    }

    #[tokio::test]
    async fn alerts_persist_crossings_and_the_trigger_threshold_per_user() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        db.ensure_admin("hash").await.unwrap();
        let user = db.user_by_username("admin").await.unwrap().unwrap();
        sqlx::query("INSERT INTO users (username) VALUES ('reader')")
            .execute(db.pool())
            .await
            .unwrap();
        let reader = db.user_by_username("reader").await.unwrap().unwrap();
        assert_eq!(
            settings(&db, user.id).await.unwrap()["items"][0]["enabled"],
            false
        );
        save_setting(&db, user.id, "513100", true, 13.58)
            .await
            .unwrap();
        save_setting(&db, reader.id, "513100", true, 20.0)
            .await
            .unwrap();
        let at = "2026-09-30T14:30:31+08:00";
        assert!(claim(&db, user.id, "513100", 13.73, at).await.unwrap());
        assert!(!claim(&db, user.id, "513100", 14.2, at).await.unwrap());
        assert!(!claim(&db, reader.id, "513100", 13.73, at).await.unwrap());
        assert!(!claim(&db, user.id, "513100", 13.2, at).await.unwrap());
        assert!(claim(&db, user.id, "513100", 13.58, at).await.unwrap());
        mark_delivery(&db, user.id, "513100", at, "sent")
            .await
            .unwrap();
        save_setting(&db, user.id, "513100", true, 10.0)
            .await
            .unwrap();
        let last = latest_alert(&db, user.id).await.unwrap().unwrap();
        assert_eq!(last["threshold_pct"], 13.58);
        assert_eq!(last["delivery_status"], "sent");
        assert!(claim(&db, user.id, "513100", 11.0, at).await.unwrap());
        save_setting(&db, user.id, "513100", false, 10.0)
            .await
            .unwrap();
        assert!(!claim(&db, user.id, "513100", 30.0, at).await.unwrap());
        assert!(latest_alert(&db, reader.id).await.unwrap().is_none());
    }
}
