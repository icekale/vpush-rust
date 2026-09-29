use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::db::Db;

pub(crate) const EXTRACT_PROMPT: &str = "从研报文本抽取结构化信息，只输出一个 JSON 对象，字段 rating、target_price、thesis、report_kind、tickers。tickers 每项含 code、name、stance，最多 8 个。不确定就留空。";
const DIGEST_PROMPT: &str = "把同一标的的研报要点汇编成中文综述，只输出一个 JSON 对象，字段 consensus、evolution、divergence。evolution 是 {date, point} 数组。没有的信息留空，不要编造。";

pub async fn read_text(path: String) -> Option<String> {
    tokio::fs::read_to_string(path).await.ok()
}

pub async fn extract_due<R, RF, A, AF>(
    db: &Db,
    now: i64,
    deadline: std::time::Instant,
    mut read: R,
    mut ask: A,
) -> Result<usize, sqlx::Error>
where
    R: FnMut(String) -> RF,
    RF: std::future::Future<Output = Option<String>>,
    A: FnMut(String, String) -> AF,
    AF: std::future::Future<Output = Result<String, String>>,
{
    if db.setting("report_extract_enabled").await?.as_deref() == Some("0") {
        return Ok(0);
    }
    let interval = number(db, "report_extract_interval_seconds", 3600).await?;
    let last = number(db, "report_extract_last_at", 0).await?;
    if last > 0 && now.saturating_sub(last) < interval {
        return Ok(0);
    }
    let today = beijing_key(db).await?;
    let done_key = format!("report_extract_done_{today}");
    let done_today = number(db, &done_key, 0).await?;
    let daily_limit = number(db, "report_extract_daily_limit", 1000).await?;
    if done_today >= daily_limit {
        return Ok(0);
    }
    db.set_setting("report_extract_last_at", &now.to_string())
        .await?;
    let backfill_days = number(db, "report_extract_backfill_days", 2).await?;
    let min_date = if backfill_days == 0 {
        String::new()
    } else {
        sqlx::query_scalar("SELECT date('now', '+8 hours', ?)")
            .bind(format!("-{backfill_days} days"))
            .fetch_one(db.pool())
            .await?
    };
    let groups = extract_groups(db, &min_date).await?;
    if groups.is_empty() {
        return Ok(0);
    }
    if db
        .setting("report_extract_pipeline_version")
        .await?
        .as_deref()
        != Some("4")
    {
        reset_recoverable(db, &groups, &min_date).await?;
        db.set_setting("report_extract_pipeline_version", "4")
            .await?;
    }
    let batch = number(db, "report_extract_batch", 80)
        .await?
        .min(daily_limit - done_today) as usize;
    if batch == 0 {
        return Ok(0);
    }
    let per_group = (batch / groups.len()).max(1) as i64;
    let mut docs = Vec::new();
    for group in &groups {
        let rows = sqlx::query(
            "SELECT d.group_id, d.media_id, d.txt_path, d.pdf_path, d.name, d.sort_date \
             FROM ima_document_index d LEFT JOIN report_extractions re \
             ON re.group_id = d.group_id AND re.media_id = d.media_id \
             WHERE d.group_id = ? AND (d.txt_path != '' OR d.pdf_path != '') AND re.media_id IS NULL \
             AND (? = '' OR d.sort_date >= ?) \
             ORDER BY (d.sort_date = '') ASC, d.sort_date DESC, d.media_id LIMIT ?",
        )
        .bind(group)
        .bind(&min_date)
        .bind(&min_date)
        .bind(per_group)
        .fetch_all(db.pool())
        .await?;
        docs.extend(rows);
    }
    docs.sort_by(|left, right| {
        right
            .get::<String, _>("sort_date")
            .cmp(&left.get::<String, _>("sort_date"))
            .then(
                left.get::<String, _>("group_id")
                    .cmp(&right.get::<String, _>("group_id")),
            )
    });
    docs.truncate(batch);
    if docs.is_empty() {
        return Ok(0);
    }
    let model = db
        .setting("report_extract_model")
        .await?
        .unwrap_or_default();
    let mut completed = 0;
    let mut unresolved = 0;
    for doc in &docs {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let group_id = doc.get::<String, _>("group_id");
        let media_id = doc.get::<String, _>("media_id");
        let txt_path = doc.get::<String, _>("txt_path");
        let pdf_path = doc.get::<String, _>("pdf_path");
        let name = doc.get::<String, _>("name");
        let mut opened = false;
        let mut text = String::new();
        if !txt_path.is_empty() {
            if let Some(body) = read(txt_path).await {
                opened = true;
                text = body;
            }
        }
        if text.trim().is_empty() && !pdf_path.is_empty() {
            if let Some(body) = read(pdf_path).await {
                opened = true;
                text = body;
            }
        }
        let usable = collapse(&text);
        if usable.is_empty() {
            if !opened
                && (!doc.get::<String, _>("txt_path").is_empty()
                    || !doc.get::<String, _>("pdf_path").is_empty())
            {
                unresolved += 1;
                continue;
            }
            if name.trim().is_empty() {
                save_extraction(
                    db,
                    &group_id,
                    &media_id,
                    "",
                    "",
                    "",
                    "",
                    "",
                    &[],
                    &model,
                    "notext",
                )
                .await?;
                continue;
            }
        }
        let hashed = hash16(&usable);
        if usable.chars().count() < 200 && name.trim().is_empty() {
            save_extraction(
                db,
                &group_id,
                &media_id,
                &hashed,
                "",
                "",
                "",
                "",
                &[],
                &model,
                "empty",
            )
            .await?;
            completed += 1;
            continue;
        }
        let answer = match ask(name.clone(), usable.chars().take(8000).collect()).await {
            Ok(answer) => answer,
            Err(_) => {
                save_extraction(
                    db,
                    &group_id,
                    &media_id,
                    &hashed,
                    "",
                    "",
                    "",
                    "",
                    &[],
                    &model,
                    "failed",
                )
                .await?;
                continue;
            }
        };
        match clean_extraction(&answer) {
            Some(cleaned) => {
                save_extraction(
                    db,
                    &group_id,
                    &media_id,
                    &hashed,
                    &cleaned.report_kind,
                    &cleaned.rating,
                    &cleaned.target_price,
                    &cleaned.thesis,
                    &cleaned.tickers,
                    &model,
                    &cleaned.status,
                )
                .await?;
                completed += 1;
            }
            None => {
                save_extraction(
                    db,
                    &group_id,
                    &media_id,
                    &hashed,
                    "",
                    "",
                    "",
                    "",
                    &[],
                    &model,
                    "failed",
                )
                .await?;
            }
        }
    }
    if unresolved == docs.len() {
        return Ok(0);
    }
    if completed > 0 {
        db.set_setting(&done_key, &(done_today + completed as i64).to_string())
            .await?;
    }
    Ok(completed)
}

pub async fn digest_due<A, AF>(
    db: &Db,
    now: i64,
    deadline: std::time::Instant,
    mut ask: A,
) -> Result<usize, sqlx::Error>
where
    A: FnMut(String) -> AF,
    AF: std::future::Future<Output = Result<String, String>>,
{
    if db.setting("ima_digest_enabled").await?.as_deref() == Some("0") {
        return Ok(0);
    }
    let interval = number(db, "ima_digest_interval_seconds", 3600).await?;
    let last = number(db, "ima_digest_last_at", 0).await?;
    if interval > 0 && last > 0 && now.saturating_sub(last) < interval {
        return Ok(0);
    }
    if interval > 0 {
        db.set_setting("ima_digest_last_at", &now.to_string())
            .await?;
    }
    let today = beijing_key(db).await?;
    let done_key = format!("ima_digest_done_{today}");
    let done_today = number(db, &done_key, 0).await?;
    let daily_limit = number(db, "ima_digest_daily_limit", 30).await?;
    if done_today >= daily_limit {
        return Ok(0);
    }
    let min_reports = number(db, "ima_digest_min_reports", 3).await?;
    let batch = number(db, "ima_digest_batch", 5)
        .await?
        .min(daily_limit - done_today) as usize;
    if batch == 0 {
        return Ok(0);
    }
    let rows = sqlx::query(
        "SELECT t.code AS code, MAX(t.name) AS name, \
         COUNT(DISTINCT t.group_id || '/' || t.media_id) AS report_count, \
         COALESCE(MAX(d.sort_date), '') AS latest_date, COALESCE(g.signature, '') AS signature \
         FROM report_extraction_tickers t \
         JOIN ima_document_index d ON d.group_id = t.group_id AND d.media_id = t.media_id \
         LEFT JOIN ima_ticker_digests g ON g.kind = 'ticker' AND g.code = t.code \
         GROUP BY t.code HAVING report_count >= ? \
         ORDER BY report_count DESC, latest_date DESC LIMIT 200",
    )
    .bind(min_reports)
    .fetch_all(db.pool())
    .await?;
    let model = db.setting("ima_digest_model").await?.unwrap_or_default();
    let mut done = 0;
    for row in rows {
        if done == batch || std::time::Instant::now() >= deadline {
            break;
        }
        let count = row.get::<i64, _>("report_count");
        let signature = format!("{count}:{}", row.get::<String, _>("latest_date"));
        if row.get::<String, _>("signature") == signature {
            continue;
        }
        let code = row.get::<String, _>("code");
        let name = row.get::<String, _>("name");
        let reports = sqlx::query(
            "SELECT d.sort_date, d.name, re.rating, re.target_price, re.thesis \
             FROM report_extraction_tickers t \
             JOIN ima_document_index d ON d.group_id = t.group_id AND d.media_id = t.media_id \
             LEFT JOIN report_extractions re ON re.group_id = t.group_id AND re.media_id = t.media_id \
             WHERE t.code = ? ORDER BY (d.sort_date = '') ASC, d.sort_date DESC, d.media_id DESC LIMIT 40",
        )
        .bind(&code)
        .fetch_all(db.pool())
        .await?;
        let lines = source_lines(&reports);
        if lines.is_empty() {
            save_digest(
                db,
                &code,
                &name,
                &signature,
                count,
                "",
                &model,
                "failed:empty",
            )
            .await?;
            continue;
        }
        let prompt = format!(
            "{DIGEST_PROMPT}\n标的：{code} {name}\n研报要点：\n{}",
            lines.join("\n")
        );
        match ask(prompt).await {
            Ok(answer) => match clean_digest(&answer) {
                Some(digest) => {
                    save_digest(db, &code, &name, &signature, count, &digest, &model, "ok").await?;
                    done += 1;
                }
                None => {
                    save_digest(
                        db,
                        &code,
                        &name,
                        &signature,
                        count,
                        "",
                        &model,
                        "failed:parse",
                    )
                    .await?
                }
            },
            Err(_) => {
                save_digest(
                    db,
                    &code,
                    &name,
                    &signature,
                    count,
                    "",
                    &model,
                    "failed:llm",
                )
                .await?
            }
        }
    }
    if done > 0 {
        db.set_setting(&done_key, &(done_today + done as i64).to_string())
            .await?;
    }
    Ok(done)
}

struct Cleaned {
    report_kind: String,
    rating: String,
    target_price: String,
    thesis: String,
    tickers: Vec<(String, String, String)>,
    status: String,
}

async fn extract_groups(db: &Db, min_date: &str) -> Result<Vec<String>, sqlx::Error> {
    if let Some(configured) = db.setting("report_extract_groups").await? {
        let groups: Vec<String> = configured
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        if !groups.is_empty() {
            return Ok(groups);
        }
    }
    let rows = sqlx::query(
        "SELECT DISTINCT group_id FROM ima_document_index \
         WHERE group_id NOT LIKE 'feishu-%' AND (txt_path != '' OR pdf_path != '') AND (? = '' OR sort_date >= ?) \
         ORDER BY group_id",
    )
    .bind(min_date)
    .bind(min_date)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.into_iter().map(|row| row.get("group_id")).collect())
}

async fn reset_recoverable(db: &Db, groups: &[String], min_date: &str) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT re.group_id, re.media_id FROM report_extractions re \
         JOIN ima_document_index d ON d.group_id = re.group_id AND d.media_id = re.media_id \
         WHERE re.status IN ('failed', 'notext', 'empty', 'nofile') AND (? = '' OR d.sort_date >= ?)",
    )
    .bind(min_date)
    .bind(min_date)
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        let group_id = row.get::<String, _>("group_id");
        if !groups.iter().any(|group| group == &group_id) {
            continue;
        }
        let media_id = row.get::<String, _>("media_id");
        sqlx::query("DELETE FROM report_extraction_tickers WHERE group_id = ? AND media_id = ?")
            .bind(&group_id)
            .bind(&media_id)
            .execute(db.pool())
            .await?;
        sqlx::query("DELETE FROM report_extractions WHERE group_id = ? AND media_id = ?")
            .bind(&group_id)
            .bind(&media_id)
            .execute(db.pool())
            .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn save_extraction(
    db: &Db,
    group_id: &str,
    media_id: &str,
    txt_hash: &str,
    report_kind: &str,
    rating: &str,
    target_price: &str,
    thesis: &str,
    tickers: &[(String, String, String)],
    model: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = db.pool().begin().await?;
    sqlx::query(
        "INSERT INTO report_extractions (group_id, media_id, txt_hash, report_kind, rating, target_price, thesis, model, status, extracted_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(group_id, media_id) DO UPDATE SET txt_hash = excluded.txt_hash, report_kind = excluded.report_kind, \
         rating = excluded.rating, target_price = excluded.target_price, thesis = excluded.thesis, model = excluded.model, \
         status = excluded.status, extracted_at = excluded.extracted_at",
    )
    .bind(group_id)
    .bind(media_id)
    .bind(txt_hash)
    .bind(report_kind)
    .bind(rating)
    .bind(target_price)
    .bind(thesis)
    .bind(model)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM report_extraction_tickers WHERE group_id = ? AND media_id = ?")
        .bind(group_id)
        .bind(media_id)
        .execute(&mut *tx)
        .await?;
    for (code, name, stance) in tickers {
        sqlx::query("INSERT OR IGNORE INTO report_extraction_tickers (group_id, media_id, code, name, stance) VALUES (?, ?, ?, ?, ?)")
            .bind(group_id)
            .bind(media_id)
            .bind(code)
            .bind(name)
            .bind(stance)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

#[allow(clippy::too_many_arguments)]
async fn save_digest(
    db: &Db,
    code: &str,
    name: &str,
    signature: &str,
    source_count: i64,
    digest: &str,
    model: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ima_ticker_digests (kind, code, name, signature, source_count, digest, model, status, updated_at) \
         VALUES ('ticker', ?, ?, ?, ?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(kind, code) DO UPDATE SET name = excluded.name, signature = excluded.signature, \
         source_count = excluded.source_count, digest = excluded.digest, model = excluded.model, \
         status = excluded.status, updated_at = excluded.updated_at",
    )
    .bind(code)
    .bind(name)
    .bind(signature)
    .bind(source_count)
    .bind(digest)
    .bind(model)
    .bind(status)
    .execute(db.pool())
    .await?;
    Ok(())
}

fn clean_extraction(raw: &str) -> Option<Cleaned> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let rating = rating_zh(
        value
            .get("rating")
            .and_then(|item| item.as_str())
            .unwrap_or(""),
    );
    let target_price = clip(
        value
            .get("target_price")
            .and_then(|item| item.as_str())
            .unwrap_or(""),
        64,
    );
    let thesis = clip(
        &collapse(
            value
                .get("thesis")
                .and_then(|item| item.as_str())
                .unwrap_or(""),
        ),
        200,
    );
    let report_kind = clip(
        value
            .get("report_kind")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim(),
        12,
    );
    let mut tickers = Vec::new();
    for item in value
        .get("tickers")
        .and_then(|item| item.as_array())
        .into_iter()
        .flatten()
    {
        let name = clip(
            item.get("name")
                .and_then(|item| item.as_str())
                .unwrap_or("")
                .trim(),
            32,
        );
        let Some(code) = normalize_code(
            item.get("code")
                .and_then(|item| item.as_str())
                .unwrap_or(""),
            &name,
        ) else {
            continue;
        };
        if tickers.iter().any(|(seen, _, _)| seen == &code) {
            continue;
        }
        tickers.push((
            code,
            name,
            clip(
                item.get("stance")
                    .and_then(|item| item.as_str())
                    .unwrap_or("")
                    .trim(),
                16,
            ),
        ));
        if tickers.len() == 8 {
            break;
        }
    }
    let rating = clip(&rating, 24);
    let status = if rating.is_empty()
        && target_price.is_empty()
        && thesis.is_empty()
        && tickers.is_empty()
    {
        "empty"
    } else {
        "ok"
    };
    Some(Cleaned {
        report_kind,
        rating,
        target_price,
        thesis,
        tickers,
        status: status.to_string(),
    })
}

fn clean_digest(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let mut evolution = Vec::new();
    for item in value
        .get("evolution")
        .and_then(|item| item.as_array())
        .into_iter()
        .flatten()
    {
        let point = clip(
            &collapse(
                item.get("point")
                    .and_then(|item| item.as_str())
                    .unwrap_or(""),
            ),
            400,
        );
        if point.is_empty() {
            continue;
        }
        evolution.push(serde_json::json!({"date": clip(item.get("date").and_then(|item| item.as_str()).unwrap_or("").trim(), 10), "point": point}));
        if evolution.len() == 24 {
            break;
        }
    }
    Some(serde_json::json!({
        "consensus": clip(&collapse(value.get("consensus").and_then(|item| item.as_str()).unwrap_or("")), 1200),
        "evolution": evolution,
        "divergence": clip(&collapse(value.get("divergence").and_then(|item| item.as_str()).unwrap_or("")), 1200),
    }).to_string())
}

fn source_lines(rows: &[sqlx::sqlite::SqliteRow]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let thesis = clip(&collapse(&row.get::<String, _>("thesis")), 300);
            if thesis.is_empty() {
                return None;
            }
            let head = [
                row.get::<String, _>("sort_date"),
                clip(&collapse(&row.get::<String, _>("name")), 80),
                rating_zh(&row.get::<String, _>("rating")),
                row.get::<String, _>("target_price"),
            ]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
            Some(format!("{head} | {thesis}"))
        })
        .collect()
}

fn normalize_code(raw: &str, name: &str) -> Option<String> {
    let code = raw.trim().to_uppercase();
    if let Some(digits) = a_share(&code) {
        return Some(digits);
    }
    if name.is_empty()
        || !code.chars().any(|item| item.is_ascii_alphabetic())
        || !symbol_like(&code)
    {
        return None;
    }
    Some(code.chars().take(16).collect())
}

fn a_share(code: &str) -> Option<String> {
    let mut rest = code;
    for prefix in ["SH", "SZ", "BJ"] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            if stripped
                .chars()
                .next()
                .is_some_and(|item| item.is_ascii_digit())
            {
                rest = stripped;
                break;
            }
        }
    }
    if let Some((digits, suffix)) = rest.split_once('.') {
        if !matches!(suffix, "SH" | "SZ" | "BJ" | "SS") {
            return None;
        }
        rest = digits;
    }
    (rest.len() == 6 && rest.chars().all(|item| item.is_ascii_digit())).then(|| rest.to_string())
}

fn symbol_like(code: &str) -> bool {
    let (head, suffix) = match code.split_once('.') {
        Some((head, suffix)) => (head, Some(suffix)),
        None => (code, None),
    };
    (1..=6).contains(&head.len())
        && head.chars().all(|item| item.is_ascii_alphanumeric())
        && suffix.is_none_or(|suffix| {
            (1..=4).contains(&suffix.len())
                && suffix.chars().all(|item| item.is_ascii_alphanumeric())
        })
}

fn rating_zh(raw: &str) -> String {
    let key = raw.trim().to_lowercase().replace(['_', '-'], " ");
    let key = key.split_whitespace().collect::<Vec<_>>().join(" ");
    let mapped = match key.as_str() {
        "strong buy" => "强烈买入",
        "speculative buy" => "投机买入",
        "long term buy" => "长期买入",
        "buy" => "买入",
        "accumulate" | "add" | "overweight" => "增持",
        "outperform" | "market outperform" => "跑赢大市",
        "sector outperform" => "跑赢行业",
        "market perform" => "与大市同步",
        "sector perform" => "与行业同步",
        "equal weight" | "equalweight" => "标配",
        "neutral" => "中性",
        "hold" => "持有",
        "underperform" => "跑输大市",
        "sector underperform" => "跑输行业",
        "underweight" | "reduce" => "减持",
        "sell" => "卖出",
        _ => return raw.trim().to_string(),
    };
    mapped.to_string()
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn hash16(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut hasher = Sha256::new();
    hasher.update(bytes.get(..20000).unwrap_or(bytes));
    format!("{:x}", hasher.finalize())
        .chars()
        .take(16)
        .collect()
}

async fn number(db: &Db, key: &str, default: i64) -> Result<i64, sqlx::Error> {
    Ok(db
        .setting(key)
        .await?
        .and_then(|value| value.parse().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(default))
}

async fn beijing_key(db: &Db) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT replace(date('now', '+8 hours'), '-', '')")
        .fetch_one(db.pool())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "vpush-reports-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn later() -> std::time::Instant {
        std::time::Instant::now() + std::time::Duration::from_secs(3600)
    }

    async fn doc(db: &Db, group: &str, media: &str, day: &str, name: &str, txt: &str) {
        sqlx::query("INSERT INTO ima_document_index (group_id, media_id, sort_date, name, txt_path) VALUES (?, ?, ?, ?, ?)")
            .bind(group)
            .bind(media)
            .bind(day)
            .bind(name)
            .bind(txt)
            .execute(db.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn extract_is_fair_and_does_not_retry_failures() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        db.set_setting("report_extract_interval_seconds", "1")
            .await
            .unwrap();
        db.set_setting("report_extract_backfill_days", "0")
            .await
            .unwrap();
        db.set_setting("report_extract_batch", "2").await.unwrap();
        doc(&db, "a", "old", "2020-01-01", "旧", "old.txt").await;
        doc(&db, "a", "new", "2026-08-02", "新", "new.txt").await;
        doc(&db, "a", "gone", "2026-08-01", "失败", "gone.txt").await;
        doc(&db, "b", "one", "2026-08-01", "乙", "b.txt").await;
        doc(&db, "feishu-1", "skip", "2026-08-02", "飞书", "f.txt").await;
        sqlx::query("INSERT INTO report_extractions (group_id, media_id, status) VALUES ('a', 'gone', 'failed')").execute(db.pool()).await.unwrap();
        let mut asked = Vec::new();
        let done = extract_due(&db, 10, later(), |path| async move { Some(format!("正文 {path} {}", "甲".repeat(180))) }, |title, _| {
            asked.push(title);
            async { Ok(r#"{"rating":"Buy","target_price":"10","thesis":"逻辑","report_kind":"公司","tickers":[{"code":"SH600519","name":"别名","stance":"推荐"},{"code":"???","name":"","stance":""}]}"#.into()) }
        })
        .await
        .unwrap();
        assert_eq!(done, 2);
        assert!(asked.iter().any(|title| title == "新"));
        assert!(asked.iter().any(|title| title == "乙"));
        assert!(!asked.iter().any(|title| title == "旧" || title == "飞书"));
        let rating: String =
            sqlx::query_scalar("SELECT rating FROM report_extractions WHERE media_id = 'new'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(rating, "买入");
        let code: String =
            sqlx::query_scalar("SELECT code FROM report_extraction_tickers WHERE media_id = 'new'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(code, "600519");
        assert!(sqlx::query_scalar::<_, String>(
            "SELECT status FROM report_extractions WHERE media_id = 'gone'"
        )
        .fetch_optional(db.pool())
        .await
        .unwrap()
        .is_none());
        let again = extract_due(
            &db,
            10,
            later(),
            |_| async { Some("x".into()) },
            |_, _| async { Ok("{}".into()) },
        )
        .await
        .unwrap();
        assert_eq!(again, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn missing_file_retries_and_llm_failure_is_kept() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        db.set_setting("report_extract_interval_seconds", "1")
            .await
            .unwrap();
        db.set_setting("report_extract_backfill_days", "0")
            .await
            .unwrap();
        doc(&db, "a", "miss", "2026-08-02", "缺", "miss.txt").await;
        let skipped = extract_due(
            &db,
            20,
            later(),
            |_| async { None },
            |_, _| async { Ok("{}".into()) },
        )
        .await
        .unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM report_extractions")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        let failed = extract_due(
            &db,
            22,
            later(),
            |_| async { Some("甲".repeat(220)) },
            |_, _| async { Err("超时".into()) },
        )
        .await
        .unwrap();
        assert_eq!(failed, 0);
        let status: String =
            sqlx::query_scalar("SELECT status FROM report_extractions WHERE media_id = 'miss'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(status, "failed");
        let mut calls = 0;
        extract_due(
            &db,
            24,
            later(),
            |_| async { Some("甲".repeat(220)) },
            |_, _| {
                calls += 1;
                async { Ok("{}".into()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn digest_compiles_stale_tickers_once() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        for (media, day) in [
            ("m1", "2026-08-01"),
            ("m2", "2026-08-02"),
            ("m3", "2026-08-03"),
        ] {
            doc(&db, "a", media, day, "茅台点评", "").await;
            sqlx::query("INSERT INTO report_extractions (group_id, media_id, rating, thesis, status) VALUES ('a', ?, 'Buy', '继续看好', 'ok')").bind(media).execute(db.pool()).await.unwrap();
            sqlx::query("INSERT INTO report_extraction_tickers (group_id, media_id, code, name) VALUES ('a', ?, '600519', '贵州茅台')").bind(media).execute(db.pool()).await.unwrap();
        }
        doc(&db, "a", "small", "2026-08-03", "不够", "").await;
        sqlx::query("INSERT INTO report_extraction_tickers (group_id, media_id, code, name) VALUES ('a', 'small', '000001', '平安银行')").execute(db.pool()).await.unwrap();
        let mut calls = 0;
        db.set_setting("ima_digest_interval_seconds", "0")
            .await
            .unwrap();
        let done = digest_due(&db, 10, later(), |prompt| {
            calls += 1;
            assert!(prompt.contains("600519"));
            assert!(prompt.contains("继续看好"));
            assert!(!prompt.contains("000001"));
            async { Ok(r#"{"consensus":"一致看好","evolution":[{"date":"2026-08-03","point":"上调"}],"divergence":""}"#.into()) }
        })
        .await
        .unwrap();
        assert_eq!(done, 1);
        assert_eq!(calls, 1);
        let raw: String =
            sqlx::query_scalar("SELECT digest FROM ima_ticker_digests WHERE code = '600519'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(raw.contains("一致看好"));
        let again = digest_due(&db, 10, later(), |_| async { Err("不应再问".into()) })
            .await
            .unwrap();
        assert_eq!(again, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn spent_budget_leaves_documents_and_tickers_for_later() {
        let path = temp_db();
        let db = Db::open(&path).await.unwrap();
        db.set_setting("report_extract_interval_seconds", "1")
            .await
            .unwrap();
        db.set_setting("report_extract_backfill_days", "0")
            .await
            .unwrap();
        db.set_setting("ima_digest_interval_seconds", "0")
            .await
            .unwrap();
        doc(&db, "a", "m1", "2026-08-02", "茅台点评", "m1.txt").await;
        doc(&db, "a", "m2", "2026-08-03", "茅台跟踪", "m2.txt").await;
        doc(&db, "a", "m3", "2026-08-04", "茅台更新", "m3.txt").await;
        for media in ["m1", "m2", "m3"] {
            sqlx::query("INSERT INTO report_extraction_tickers (group_id, media_id, code, name) VALUES ('a', ?, '600519', '贵州茅台')").bind(media).execute(db.pool()).await.unwrap();
        }
        let spent = std::time::Instant::now();
        let mut calls = 0;
        let extracted = extract_due(
            &db,
            10,
            spent,
            |_| async { Some("甲".repeat(220)) },
            |_, _| {
                calls += 1;
                async { Err("不应调用".into()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(extracted, 0);
        let digested = digest_due(&db, 10, spent, |_| {
            calls += 1;
            async { Err("不应调用".into()) }
        })
        .await
        .unwrap();
        assert_eq!(digested, 0);
        assert_eq!(calls, 0);
        let marked: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM report_extractions) + (SELECT COUNT(*) FROM ima_ticker_digests)",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(marked, 0);
        let _ = std::fs::remove_file(&path);
    }
}
