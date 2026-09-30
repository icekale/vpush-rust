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

#[derive(Clone)]
struct Quote {
    symbol: &'static str,
    price: Option<f64>,
    iopv: Option<f64>,
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
        let rate = self.price.zip(self.iopv).and_then(|(p, v)| premium(p, v));
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
            "source": "新浪行情IOPV（实时性未独立验证）"})
    }
}

fn sina_time(parts: &[&str], date: usize, time: usize) -> Option<(String, i64)> {
    let date = parts.get(date)?;
    let time = parts.get(time)?;
    if date.len() != 10 || time.len() != 8 {
        return None;
    }
    crate::market::mainland_quote_time(&format!(
        "{}{}",
        date.replace('-', ""),
        time.replace(':', "")
    ))
}

fn positive(raw: &str) -> Option<f64> {
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn parse_quotes(text: &str) -> Vec<Quote> {
    // 新浪和腾讯的变量包装相同，只是变量前缀不同。
    let text = text.replace("hq_str_", "v_");
    let records = crate::market::quote_records(&text);
    FUNDS
        .iter()
        .map(|(symbol, _)| {
            let raw_symbol = format!("sh{symbol}");
            let parts = records
                .get(raw_symbol.as_str())
                .map(|raw| raw.split(',').collect::<Vec<_>>())
                .unwrap_or_default();
            let iopv_symbol = format!("sh{symbol}_iopv");
            let reference = records
                .get(iopv_symbol.as_str())
                .map(|raw| raw.split(',').collect::<Vec<_>>())
                .unwrap_or_default();
            let time = sina_time(&parts, 30, 31);
            let reference_time = sina_time(&reference, 0, 1);
            Quote {
                symbol,
                price: parts.get(3).and_then(|raw| positive(raw)),
                iopv: reference.get(2).and_then(|raw| positive(raw)),
                market_at: time.as_ref().map(|(at, _)| at.clone()),
                market_unix: time.map(|(_, at)| at),
                reference_at: reference_time.as_ref().map(|(at, _)| at.clone()),
                reference_unix: reference_time.map(|(_, at)| at),
            }
        })
        .collect()
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

async fn quotes() -> (Vec<Quote>, bool) {
    let mut cache = cache().lock().await;
    if cache
        .at
        .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
    {
        let text = tokio::task::spawn_blocking(|| {
            crate::market::http_text_with_referer(
                "https://hq.sinajs.cn/list=sh513100,sh513100_iopv,sh513500,sh513500_iopv",
                "https://finance.sina.com.cn/",
            )
        })
        .await
        .ok()
        .flatten();
        cache.fetch_ok = text.is_some();
        if let Some(text) = text {
            cache.items = parse_quotes(&text);
        }
        if cache.items.is_empty() {
            cache.items = parse_quotes("");
        }
        cache.at = Some(Instant::now());
    }
    (cache.items.clone(), cache.fetch_ok)
}

pub async fn snapshot() -> Value {
    let (items, ok) = quotes().await;
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
        let text = format!("【ETF高溢价提醒】\n\n{} {}\n当前溢价率：{rate:.2}%\n提醒水位：{threshold:.2}%\n触发时间：{}\n\n参考价值：新浪行情IOPV（{:.4}）\nIOPV更新时间：{}",
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
            let (quotes, ok) = quotes().await;
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

    fn sample(symbol: &str, at: &str, price: &str, iopv: &str) -> String {
        let (date, time) = if at.len() == 14 && at.bytes().all(|b| b.is_ascii_digit()) {
            (
                format!("{}-{}-{}", &at[..4], &at[4..6], &at[6..8]),
                format!("{}:{}:{}", &at[8..10], &at[10..12], &at[12..14]),
            )
        } else {
            (at.to_owned(), at.to_owned())
        };
        let mut parts = vec![""; 33];
        parts[3] = price;
        parts[30] = &date;
        parts[31] = &time;
        format!(
            "var hq_str_sh{symbol}=\"{}\";\nvar hq_str_sh{symbol}_iopv=\"{date},{time},{iopv}\";",
            parts.join(",")
        )
    }

    #[test]
    fn source_snapshot_and_sessions_guard_alerts() {
        let raw = sample("513100", "20260930143031", "2.352", "2.0381");
        let quote = parse_quotes(&raw).remove(0);
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
        for at in ["20260930120000", "20260930150100", "20261003100000"] {
            let quote = parse_quotes(&sample("513100", at, "2.352", "2.0381")).remove(0);
            assert!(!quote.alertable(quote.market_unix.unwrap()));
        }
        for at in [
            "bad-time",
            "20260900103000",
            "20260230103000",
            "20260930146000",
        ] {
            let quote = parse_quotes(&sample("513100", at, "2.352", "2.0381")).remove(0);
            assert!(quote.market_at.is_none());
            assert!(!quote.alertable(now));
        }
        for value in ["", "-", "0", "NaN", "inf"] {
            let quote = parse_quotes(&sample("513100", "20260930143031", "2.352", value)).remove(0);
            assert_eq!(quote.render(now, true)["premium_rate"], Value::Null);
            assert!(!quote.alertable(now));
        }
        assert!(parse_quotes("").iter().all(|quote| !quote.alertable(now)));
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
        let quote = parse_quotes(&sample("513100", "20260930143031", "1.1", "1.0")).remove(0);
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
    #[ignore = "访问新浪公网，手动验证行情源"]
    async fn public_iopv_snapshot_has_both_funds_and_independent_times() {
        let (quotes, ok) = quotes().await;
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
