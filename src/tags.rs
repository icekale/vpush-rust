use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use sqlx::Row;

use crate::db::{CatalogError, Db};

const TOPIC_MAX: usize = 3;
const STOCK_MAX: usize = 2;
const VOCAB_MAX: usize = 30;
const NAME_MAX: usize = 400;
const ALIAS_MAX: usize = 200;

static MAINTAINING: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct Rule {
    tag: String,
    keywords: Vec<String>,
}

#[derive(Clone)]
struct Alias {
    alias: String,
    stock: String,
}

pub async fn snapshot(db: &Db) -> Result<Value, sqlx::Error> {
    let vocab = vocabulary(db).await?;
    let names = stock_names(db).await?;
    let aliases = aliases(db).await?;
    let excluded = exclusions(db).await?;
    let dynamic = db.dynamic_tags().await?;
    let stats = stats(db).await?;
    let last = db
        .setting("tag_maintain_last")
        .await?
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    Ok(json!({
        "tags": vocab.iter().map(|rule| json!({"tag": rule.tag, "keywords": rule.keywords})).collect::<Vec<_>>(),
        "stock_names": names,
        "stock_aliases": aliases.iter().map(|item| json!({"alias": item.alias, "stock": item.stock})).collect::<Vec<_>>(),
        "excluded_stock_names": excluded,
        "universe": {"count": 0, "min_chars": 3},
        "dynamic_tags": dynamic,
        "stats": stats,
        "maintain": {"last": last, "llm_ready": false, "llm_model": ""},
    }))
}

pub async fn save(db: &Db, body: &Value) -> Result<Value, CatalogError> {
    if body.get("tags").is_none()
        && body.get("stock_names").is_none()
        && body.get("stock_aliases").is_none()
    {
        return Err(CatalogError::Bad("没有可保存的字段"));
    }
    if let Some(tags) = body.get("tags") {
        let rules = parse_rules(tags)?;
        db.set_setting(
            "tag_vocabulary",
            &serde_json::to_string(
                &rules
                    .iter()
                    .map(|rule| json!({"tag": rule.tag, "keywords": rule.keywords}))
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into()),
        )
        .await?;
    }
    if let Some(names) = body.get("stock_names") {
        let previous = stock_names(db).await?;
        let updated = parse_names(names)?;
        let mut excluded = exclusions(db).await?;
        for name in &previous {
            if !updated.contains(name) && !excluded.contains(name) {
                excluded.push(name.clone());
            }
        }
        excluded.retain(|name| !updated.contains(name));
        db.set_setting(
            "stock_names",
            &serde_json::to_string(&updated).unwrap_or_else(|_| "[]".into()),
        )
        .await?;
        db.set_setting(
            "stock_names_excluded",
            &serde_json::to_string(&excluded).unwrap_or_else(|_| "[]".into()),
        )
        .await?;
    }
    if let Some(raw) = body.get("stock_aliases") {
        let aliases = parse_aliases(raw)?;
        db.set_setting(
            "stock_aliases",
            &serde_json::to_string(
                &aliases
                    .iter()
                    .map(|item| json!({"alias": item.alias, "stock": item.stock}))
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into()),
        )
        .await?;
    }
    Ok(snapshot(db).await?)
}

pub async fn backfill(db: &Db, mode: &str) -> Result<Value, CatalogError> {
    if mode != "pending" && mode != "all" {
        return Err(CatalogError::Bad("mode 需为 pending 或 all"));
    }
    let rules = vocabulary(db).await?;
    let names = stock_names(db).await?;
    let excluded = exclusions(db).await?;
    let names: Vec<String> = names
        .into_iter()
        .filter(|name| !excluded.contains(name))
        .collect();
    let aliases = aliases(db).await?;
    let sql = if mode == "pending" {
        "SELECT id, title, content FROM posts WHERE tags IS NULL OR tags = '' ORDER BY id"
    } else {
        "SELECT id, title, content FROM posts ORDER BY id"
    };
    let rows = sqlx::query(sql).fetch_all(db.pool()).await?;
    let mut processed = 0i64;
    let mut tagged = 0i64;
    for row in rows {
        let id: i64 = row.get("id");
        let title: String = row.get("title");
        let content: String = row.get("content");
        let tags = label(&title, &content, &rules, &names, &aliases);
        if !tags.is_empty() {
            tagged += 1;
        }
        sqlx::query("UPDATE posts SET tags = ? WHERE id = ?")
            .bind(serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into()))
            .bind(id)
            .execute(db.pool())
            .await?;
        processed += 1;
    }
    Ok(json!({"processed": processed, "tagged": tagged}))
}

pub async fn maintain(db: &Db, backfill_mode: &str) -> Result<Value, CatalogError> {
    if !matches!(backfill_mode, "none" | "pending" | "all") {
        return Err(CatalogError::Bad("backfill 需为 none、pending 或 all"));
    }
    if MAINTAINING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(CatalogError::Conflict("标签维护正在进行"));
    }
    let _guard = MaintainGuard;
    maintain_locked(db, backfill_mode).await
}

struct MaintainGuard;

impl Drop for MaintainGuard {
    fn drop(&mut self) {
        MAINTAINING.store(false, Ordering::Release);
    }
}

async fn maintain_locked(db: &Db, backfill_mode: &str) -> Result<Value, CatalogError> {
    let names = stock_names(db).await?;
    let before = aliases(db).await?;
    let (kept, purged) = reconcile(&before, &names);
    if purged.len() != before.len() - kept.len() || kept.len() != before.len() {
        db.set_setting(
            "stock_aliases",
            &serde_json::to_string(
                &kept
                    .iter()
                    .map(|item| json!({"alias": item.alias, "stock": item.stock}))
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into()),
        )
        .await?;
    }
    let filled = if backfill_mode == "none" {
        None
    } else {
        let mut filled = backfill(db, backfill_mode).await?;
        filled["mode"] = json!(backfill_mode);
        Some(filled)
    };
    let at = chrono_like_now();
    let summary = json!({
        "at": at,
        "cleaned": 0,
        "added_aliases": [],
        "added_stock_names": [],
        "removed_stock_names": [],
        "purged_aliases": purged.iter().map(|item| json!({"alias": item.alias, "stock": item.stock})).collect::<Vec<_>>(),
        "seeded_aliases": [],
        "purged": 0,
        "llm_used": false,
        "llm_model": "",
        "error": Value::Null,
        "backfill": filled,
    });
    db.set_setting("tag_maintain_last", &summary.to_string())
        .await?;
    let mut result = summary;
    if result
        .get("backfill")
        .and_then(|item| item.as_object())
        .is_none()
        && backfill_mode == "none"
    {
        result["backfill"] = Value::Null;
    }
    Ok(result)
}

fn label(
    title: &str,
    content: &str,
    rules: &[Rule],
    names: &[String],
    aliases: &[Alias],
) -> Vec<String> {
    let text = format!("{title} {content}");
    let folded = text.to_lowercase();
    let mut tags = Vec::new();
    for rule in rules {
        if rule
            .keywords
            .iter()
            .any(|keyword| !keyword.is_empty() && folded.contains(&keyword.to_lowercase()))
        {
            tags.push(rule.tag.clone());
        }
        if tags.len() == TOPIC_MAX {
            break;
        }
    }
    let mut stocks = Vec::new();
    for (name, _code) in stock_marks(&text) {
        push_unique(&mut stocks, name);
        if stocks.len() == STOCK_MAX {
            break;
        }
    }
    if stocks.len() < STOCK_MAX {
        let mut terms: Vec<(String, String)> = names
            .iter()
            .map(|name| (name.clone(), name.clone()))
            .collect();
        terms.extend(
            aliases
                .iter()
                .map(|item| (item.alias.clone(), item.stock.clone())),
        );
        terms.sort_by_key(|(term, _)| std::cmp::Reverse(term.chars().count()));
        for (term, official) in terms {
            if term.chars().count() < 2 || !folded.contains(&term.to_lowercase()) {
                continue;
            }
            push_unique(&mut stocks, official);
            if stocks.len() == STOCK_MAX {
                break;
            }
        }
    }
    for stock in stocks {
        push_unique(&mut tags, stock);
    }
    tags
}

fn stock_marks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let Some(end) = text[index + 1..].find('$') else {
            break;
        };
        let mark = &text[index + 1..index + 1 + end];
        index += end + 2;
        let Some((name, code)) = mark.rsplit_once('(').and_then(|(name, rest)| {
            rest.strip_suffix(')')
                .map(|code| (name.trim(), code.trim()))
        }) else {
            continue;
        };
        let upper = code.to_ascii_uppercase();
        if !(upper.starts_with("SH") || upper.starts_with("SZ") || upper.starts_with("BJ"))
            || !upper[2..].chars().all(|ch| ch.is_ascii_digit())
        {
            continue;
        }
        let mut name = name.to_string();
        while name.chars().count() > 1 && matches!(name.chars().next(), Some('N' | 'C' | 'U' | 'W'))
        {
            name = name.chars().skip(1).collect();
        }
        if !name.is_empty() {
            out.push((name, upper));
        }
    }
    out
}

fn reconcile(aliases: &[Alias], names: &[String]) -> (Vec<Alias>, Vec<Alias>) {
    let mut kept = Vec::new();
    let mut purged = Vec::new();
    let mut seen = Vec::new();
    for item in aliases {
        if seen.contains(&item.alias) {
            continue;
        }
        seen.push(item.alias.clone());
        if acceptable(item, names) {
            kept.push(item.clone());
        } else {
            purged.push(item.clone());
        }
    }
    (kept, purged)
}

fn acceptable(item: &Alias, names: &[String]) -> bool {
    if item.alias.is_empty()
        || item.stock.is_empty()
        || item.alias == item.stock
        || !names.contains(&item.stock)
    {
        return false;
    }
    if item.alias.len() <= 2 && item.alias.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return false;
    }
    if names
        .iter()
        .any(|name| name != &item.alias && name.to_lowercase().contains(&item.alias.to_lowercase()))
    {
        return false;
    }
    true
}

pub async fn discover<F, Fut>(db: &Db, mut ask: F) -> Result<usize, CatalogError>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    if MAINTAINING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(CatalogError::Conflict("标签维护正在进行"));
    }
    let _guard = MaintainGuard;
    let mut names = stock_names(db).await?;
    let mut kept = aliases(db).await?;
    let topics: Vec<String> = vocabulary(db)
        .await?
        .into_iter()
        .map(|rule| rule.tag)
        .collect();
    let excluded = exclusions(db).await?;
    let rows = sqlx::query("SELECT title, content FROM posts ORDER BY id DESC LIMIT 5000")
        .fetch_all(db.pool())
        .await?;
    let mut seen = Vec::new();
    let mut marks = Vec::new();
    for row in rows {
        let text = format!(
            "{} {}",
            row.get::<String, _>("title"),
            row.get::<String, _>("content")
        );
        for (name, code) in stock_marks(&text) {
            if !equity_code(&code) || seen.iter().any(|item: &(String, String)| item.0 == name) {
                continue;
            }
            seen.push((name.clone(), code));
            if names.iter().any(|item| item == &name)
                || kept
                    .iter()
                    .any(|item| item.alias == name || item.stock == name)
                || topics.iter().any(|item| item == &name)
                || excluded.iter().any(|item| item == &name)
            {
                continue;
            }
            marks.push(seen.last().cloned().unwrap());
            if marks.len() == 64 {
                break;
            }
        }
    }
    if marks.is_empty() {
        return Ok(0);
    }
    let mut added = 0;
    for batch in marks.chunks(8) {
        let payload = serde_json::to_string(
            &batch
                .iter()
                .map(|(name, code)| json!({"name": name, "code": code}))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into());
        let answer = ask(payload).await.map_err(CatalogError::Invalid)?;
        let parsed = parse_resolutions(&answer)
            .ok_or_else(|| CatalogError::Invalid("LLM 标记解析无有效输出".into()))?;
        for (name, official, is_alias) in parsed {
            if topics.iter().any(|item| item == &official)
                || excluded.iter().any(|item| item == &official)
                || !equity_name(&official)
            {
                continue;
            }
            if !names.iter().any(|item| item == &official) && names.len() < NAME_MAX {
                names.push(official.clone());
            }
            if is_alias && !name.is_empty() && name != official && kept.len() < ALIAS_MAX {
                let item = Alias {
                    alias: name,
                    stock: official,
                };
                if acceptable(&item, &names) && !kept.iter().any(|old| old.alias == item.alias) {
                    kept.push(item);
                    added += 1;
                }
            }
        }
    }
    db.set_setting(
        "stock_names",
        &serde_json::to_string(&names).unwrap_or_else(|_| "[]".into()),
    )
    .await?;
    db.set_setting(
        "stock_aliases",
        &serde_json::to_string(
            &kept
                .iter()
                .map(|item| json!({"alias": item.alias, "stock": item.stock}))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into()),
    )
    .await?;
    Ok(added)
}

fn equity_code(code: &str) -> bool {
    let Some(digits) = code.get(2..) else {
        return false;
    };
    match code.get(..2).unwrap_or("") {
        "SH" => digits.starts_with("60") || digits.starts_with("68"),
        "SZ" => digits.starts_with("00") || digits.starts_with("30"),
        "BJ" => digits.starts_with(['4', '8', '9']),
        _ => false,
    }
}

fn equity_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !["指数", "ETF", "etf", "基金", "板块"]
            .iter()
            .any(|marker| name.contains(marker))
}

fn parse_resolutions(raw: &str) -> Option<Vec<(String, String, bool)>> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    let rows = serde_json::from_str::<Vec<Value>>(raw.get(start..=end)?).ok()?;
    let mut out = Vec::new();
    for row in rows {
        let official = row
            .get("official")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let is_alias = row
            .get("is_alias")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !official.is_empty() {
            out.push((name, official, is_alias));
        }
    }
    Some(out)
}

fn push_unique(tags: &mut Vec<String>, tag: String) {
    if !tag.is_empty() && !tags.contains(&tag) {
        tags.push(tag);
    }
}

async fn stats(db: &Db) -> Result<Value, sqlx::Error> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS n,
                COALESCE(SUM(CASE WHEN tags != '' THEN 1 ELSE 0 END), 0) AS processed,
                COALESCE(SUM(CASE WHEN tags != '' AND tags != '[]' THEN 1 ELSE 0 END), 0) AS tagged
         FROM posts",
    )
    .fetch_one(db.pool())
    .await?;
    let total: i64 = row.get("n");
    let processed: i64 = row.get("processed");
    let tagged: i64 = row.get("tagged");
    Ok(
        json!({"total": total, "processed": processed, "tagged": tagged, "pending": total - processed}),
    )
}

async fn vocabulary(db: &Db) -> Result<Vec<Rule>, sqlx::Error> {
    let Some(raw) = db.setting("tag_vocabulary").await? else {
        return Ok(Vec::new());
    };
    let Ok(rows) = serde_json::from_str::<Vec<Value>>(&raw) else {
        return Ok(Vec::new());
    };
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let tag = row
                .get("tag")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let keywords = row
                .get("keywords")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                        .filter(|text| !text.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if tag.is_empty() {
                None
            } else {
                Some(Rule { tag, keywords })
            }
        })
        .collect())
}

async fn stock_names(db: &Db) -> Result<Vec<String>, sqlx::Error> {
    json_strings(db, "stock_names").await
}

async fn exclusions(db: &Db) -> Result<Vec<String>, sqlx::Error> {
    json_strings(db, "stock_names_excluded").await
}

async fn json_strings(db: &Db, key: &str) -> Result<Vec<String>, sqlx::Error> {
    let Some(raw) = db.setting(key).await? else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str::<Vec<Value>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
        .filter(|text| !text.is_empty())
        .collect())
}

async fn aliases(db: &Db) -> Result<Vec<Alias>, sqlx::Error> {
    let Some(raw) = db.setting("stock_aliases").await? else {
        return Ok(Vec::new());
    };
    let Ok(rows) = serde_json::from_str::<Vec<Value>>(&raw) else {
        return Ok(Vec::new());
    };
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let alias = row
                .get("alias")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let stock = row
                .get("stock")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if alias.is_empty() || stock.is_empty() {
                None
            } else {
                Some(Alias { alias, stock })
            }
        })
        .collect())
}

fn parse_rules(value: &Value) -> Result<Vec<Rule>, CatalogError> {
    let rows = value
        .as_array()
        .ok_or(CatalogError::Bad("词表格式不正确"))?;
    if rows.len() > VOCAB_MAX {
        return Err(CatalogError::Bad("词表最多 30 个标签"));
    }
    let mut out = Vec::new();
    for row in rows {
        let tag = row.get("tag").and_then(Value::as_str).unwrap_or("").trim();
        if tag.is_empty() || tag.chars().count() > 20 {
            return Err(CatalogError::Bad("标签名不能为空且不超过 20 字"));
        }
        let keywords = row
            .get("keywords")
            .and_then(Value::as_array)
            .ok_or(CatalogError::Bad("每个标签都要有关键词"))?;
        let keywords: Vec<String> = keywords
            .iter()
            .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
            .filter(|text| !text.is_empty())
            .take(20)
            .collect();
        if keywords.is_empty() {
            return Err(CatalogError::Bad("每个标签都要有关键词"));
        }
        if !out.iter().any(|rule: &Rule| rule.tag == tag) {
            out.push(Rule {
                tag: tag.to_string(),
                keywords,
            });
        }
    }
    Ok(out)
}

fn parse_names(value: &Value) -> Result<Vec<String>, CatalogError> {
    let rows = value
        .as_array()
        .ok_or(CatalogError::Bad("股票名格式不正确"))?;
    if rows.len() > NAME_MAX {
        return Err(CatalogError::Bad("常用股票最多 400 个"));
    }
    let mut out = Vec::new();
    for row in rows {
        let name = row.as_str().unwrap_or("").trim();
        if !name.is_empty() && !out.iter().any(|item: &String| item == name) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

fn parse_aliases(value: &Value) -> Result<Vec<Alias>, CatalogError> {
    let rows = value
        .as_array()
        .ok_or(CatalogError::Bad("别名格式不正确"))?;
    if rows.len() > ALIAS_MAX {
        return Err(CatalogError::Bad("别名最多 200 条"));
    }
    let mut out = Vec::new();
    for row in rows {
        let alias = row
            .get("alias")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let stock = row
            .get("stock")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if alias.is_empty() || stock.is_empty() {
            continue;
        }
        if !out.iter().any(|item: &Alias| item.alias == alias) {
            out.push(Alias {
                alias: alias.to_string(),
                stock: stock.to_string(),
            });
        }
    }
    Ok(out)
}

fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|item| item.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let time = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    if m <= 2 {
        y += 1;
    }
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backfill_uses_names_marks_and_aliases() {
        let path = std::env::temp_dir().join(format!(
            "vpush-tags-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("tag_vocabulary", r#"[{"tag":"宏观","keywords":["降息"]}]"#)
            .await
            .unwrap();
        db.set_setting("stock_names", r#"["宁德时代","贵州茅台"]"#)
            .await
            .unwrap();
        db.set_setting(
            "stock_aliases",
            r#"[{"alias":"宁王","stock":"宁德时代"},{"alias":"茅台","stock":"贵州茅台"}]"#,
        )
        .await
        .unwrap();
        let kol = db
            .add_kol("weibo", "甲", "1", None, false, false, false)
            .await
            .unwrap();
        let pending = db
            .insert_post(
                kol,
                "a",
                "宁王看好，央行可能降息",
                "2026-07-01 00:00:00",
                "",
            )
            .await
            .unwrap();
        let marked = db
            .insert_post(
                kol,
                "b",
                "$贵州茅台(SH600519)$ 组合 $某某(ZH123)$",
                "2026-07-01 00:00:00",
                "",
            )
            .await
            .unwrap();
        let done = db
            .insert_post(kol, "c", "宁德时代", "2026-07-01 00:00:00", "[\"旧\"]")
            .await
            .unwrap();
        let first = backfill(&db, "pending").await.unwrap();
        assert_eq!(first["processed"], 2);
        assert_eq!(first["tagged"], 2);
        let pending_tags: String = sqlx::query_scalar("SELECT tags FROM posts WHERE id = ?")
            .bind(pending)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(pending_tags.contains("宏观") && pending_tags.contains("宁德时代"));
        let marked_tags: String = sqlx::query_scalar("SELECT tags FROM posts WHERE id = ?")
            .bind(marked)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(marked_tags.contains("贵州茅台") && !marked_tags.contains("某某"));
        let kept: String = sqlx::query_scalar("SELECT tags FROM posts WHERE id = ?")
            .bind(done)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(kept, "[\"旧\"]");
        let maintained = maintain(&db, "all").await.unwrap();
        assert_eq!(maintained["llm_used"], false);
        assert_eq!(maintained["purged_aliases"][0]["alias"], "茅台");
        assert_eq!(maintained["backfill"]["processed"], 3);
        let redone: String = sqlx::query_scalar("SELECT tags FROM posts WHERE id = ?")
            .bind(done)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(redone.contains("宁德时代"));
        assert!(matches!(
            maintain(&db, "later").await,
            Err(CatalogError::Bad("backfill 需为 none、pending 或 all"))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn discover_asks_only_unknown_equity_marks() {
        let path = std::env::temp_dir().join(format!(
            "vpush-marks-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.set_setting("stock_names", r#"["贵州茅台"]"#)
            .await
            .unwrap();
        db.set_setting("tag_vocabulary", r#"[{"tag":"宏观","keywords":["降息"]}]"#)
            .await
            .unwrap();
        db.set_setting("stock_names_excluded", r#"["不是股票"]"#)
            .await
            .unwrap();
        let kol = db
            .add_kol("xueqiu", "甲", "1", None, false, false, false)
            .await
            .unwrap();
        db.insert_post(kol, "a", "$贵州茅台(SH600519)$ $沪深300ETF(SH510300)$ $宏观(SH600000)$ $酱香茅台(SH600519)$ $垃圾指数(SH600001)$ $排除(SH600002)$", "2026-08-01 00:00:00", "").await.unwrap();
        let mut calls = 0;
        let mut waits = 0;
        let added = loop {
            match discover(&db, |marks| {
                calls += 1;
                assert!(marks.contains("酱香茅台"));
                assert!(!marks.contains("贵州茅台"));
                assert!(!marks.contains("沪深300ETF"));
                assert!(!marks.contains("宏观"));
                async {
                    Ok(r#"[{"name":"酱香茅台","official":"贵州茅台","is_alias":true},{"name":"垃圾指数","official":"沪深300指数","is_alias":true},{"name":"排除","official":"不是股票","is_alias":true}]"#.into())
                }
            })
            .await
            {
                Err(CatalogError::Conflict(_)) => {
                    waits += 1;
                    assert!(waits < 50, "标签维护锁没有释放");
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                other => break other,
            }
        }
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(added, 1);
        let raw = db.setting("stock_aliases").await.unwrap().unwrap();
        assert!(raw.contains("酱香茅台"));
        assert!(!raw.contains("垃圾指数"));
        assert!(!raw.contains("不是股票"));
        let mut failed_waits = 0;
        let failed = loop {
            match discover(&db, |_| async { Err("超时".into()) }).await {
                Err(CatalogError::Conflict(_)) => {
                    failed_waits += 1;
                    assert!(failed_waits < 50, "标签维护锁没有释放");
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                other => break other,
            }
        };
        assert!(matches!(failed, Err(CatalogError::Invalid(_))));
        let _ = std::fs::remove_file(&path);
    }
}
