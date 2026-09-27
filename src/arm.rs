//! ARM 采集状态。只探测配置好的拉取地址，不挂载存储。

use std::io::Read;

use serde_json::{json, Value};

use crate::db::Db;

const HOURS: [i64; 3] = [1, 9, 17];

pub async fn admin_status(db: &Db) -> Result<Value, String> {
    let finished = db
        .setting("ima_pure_last_finished_at")
        .await
        .map_err(|err| err.to_string())?;
    let result = json_setting(db, "ima_pure_last_result").await?;
    let groups = json_setting(db, "ima_pure_groups").await?;
    let runtime = json_setting(db, "ima_pure_group_runtime").await?;
    let local = json_setting(db, "ima_local_libraries").await?;
    let (stamp, count) = db
        .ima_latest_batch("local-cicc-research")
        .await
        .map_err(|err| err.to_string())?;
    let mut downloads = Vec::new();
    if result["group_results"]
        .as_array()
        .is_none_or(|rows| rows.is_empty())
    {
        for group in groups.as_array().into_iter().flatten() {
            let gid = group["id"].as_str().unwrap_or("");
            if gid.is_empty() || group["enabled"] == false || gid.starts_with("local-") {
                continue;
            }
            let started = runtime[gid]["last_started_at"].as_i64().unwrap_or(0);
            let finished_at = runtime[gid]["last_finished_at"].as_i64().unwrap_or(0);
            let count = if started > 0 && finished_at >= started {
                db.ima_downloads_between(gid, started, finished_at)
                    .await
                    .map_err(|err| err.to_string())?
            } else {
                0
            };
            downloads.push(count);
        }
    }
    Ok(assemble(
        &finished.unwrap_or_default(),
        &result,
        &groups,
        &runtime,
        &local,
        &stamp,
        count,
        &downloads,
        now_secs(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn assemble(
    finished_raw: &str,
    result: &Value,
    groups: &Value,
    runtime: &Value,
    local: &Value,
    cicc_stamp: &str,
    cicc_count: i64,
    downloads: &[i64],
    now: i64,
) -> Value {
    let finished = finished_raw.trim().parse::<i64>().unwrap_or(0);
    let last_error = clip(
        result["last_error"]
            .as_str()
            .filter(|text| !text.is_empty())
            .or_else(|| result["discovery_error"].as_str())
            .unwrap_or(""),
        200,
    );
    let mut libraries = libraries(groups, runtime, result, downloads);
    libraries.push(cicc_row(cicc_stamp, cicc_count, &cicc_name(local)));
    json!({
        "pull": pull_status(),
        "last_finished_at": finished,
        "next_run_at": next_shanghai(now),
        "downloaded": result["downloaded"].as_i64().unwrap_or(0),
        "failed": result["failed"].as_i64().unwrap_or(0),
        "groups": result["succeeded_groups"].as_i64().unwrap_or(0),
        "last_error": last_error,
        "libraries": libraries,
    })
}

pub fn pull_status() -> Value {
    pull_for(std::env::var("IMA_PULL_URL").unwrap_or_default().trim())
}

fn pull_for(raw: &str) -> Value {
    if raw.is_empty() {
        return json!({"configured": false, "ok": false, "status": "unconfigured", "circuit_open": false});
    }
    let health = health_url(raw);
    if health.is_none() {
        return json!({"configured": true, "ok": false, "status": "bad url", "circuit_open": false});
    }
    match probe(health.unwrap()) {
        Ok((code, body)) if code == 200 && body.starts_with("ok") => {
            json!({"configured": true, "ok": true, "status": body, "circuit_open": false})
        }
        Ok((code, body)) => {
            json!({"configured": true, "ok": false, "status": if body.is_empty() { code.to_string() } else { body }, "circuit_open": false})
        }
        Err(err) => json!({"configured": true, "ok": false, "status": err, "circuit_open": false}),
    }
}

fn health_url(raw: &str) -> Option<&str> {
    let ok = raw.starts_with("https://") || raw.starts_with("http://");
    if !ok || raw.contains('@') || raw.contains(' ') {
        return None;
    }
    Some(raw)
}

fn probe(raw: &str) -> Result<(u16, String), String> {
    let url = if let Some(base) = raw.strip_suffix("/pull") {
        format!("{base}/healthz")
    } else {
        format!("{}/healthz", raw.trim_end_matches('/'))
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(2))
        .timeout_read(std::time::Duration::from_secs(2))
        .redirects(0)
        .build();
    match agent.get(&url).call() {
        Ok(resp) => {
            let code = resp.status();
            let mut buf = [0u8; 32];
            let n = resp.into_reader().read(&mut buf).unwrap_or(0);
            let body = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            Ok((code, body))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let mut buf = [0u8; 32];
            let n = resp.into_reader().read(&mut buf).unwrap_or(0);
            let body = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            Ok((
                code,
                if body.is_empty() {
                    code.to_string()
                } else {
                    body
                },
            ))
        }
        Err(err) => Err(err.to_string()),
    }
}

fn libraries(groups: &Value, runtime: &Value, result: &Value, downloads: &[i64]) -> Vec<Value> {
    if let Some(stored) = result["group_results"]
        .as_array()
        .filter(|rows| !rows.is_empty())
    {
        return stored
            .iter()
            .filter_map(|item| {
                item.as_object().map(|_| {
                    library_row(
                        item["id"].as_str().unwrap_or(""),
                        item["name"].as_str().unwrap_or(""),
                        item["downloaded"].as_i64().unwrap_or(0),
                        item["failed"].as_i64().unwrap_or(0),
                        runtime[item["id"].as_str().unwrap_or("")]["last_finished_at"]
                            .as_i64()
                            .unwrap_or(0),
                        item["error"].as_str().unwrap_or(""),
                    )
                })
            })
            .collect();
    }
    let failed: Vec<&str> = result["failed_groups"]
        .as_array()
        .map(|rows| rows.iter().filter_map(|item| item.as_str()).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut seen = 0;
    for group in groups.as_array().into_iter().flatten() {
        let gid = group["id"].as_str().unwrap_or("");
        if gid.is_empty() || group["enabled"] == false || gid.starts_with("local-") {
            continue;
        }
        let downloaded = downloads.get(seen).copied().unwrap_or(0);
        seen += 1;
        let err = result["group_errors"][gid].as_str().unwrap_or("");
        rows.push(library_row(
            gid,
            group["name"].as_str().unwrap_or(""),
            downloaded,
            if failed.contains(&gid) { 1 } else { 0 },
            runtime[gid]["last_finished_at"].as_i64().unwrap_or(0),
            err,
        ));
    }
    rows
}

fn library_row(
    id: &str,
    name: &str,
    downloaded: i64,
    failed: i64,
    finished: i64,
    error: &str,
) -> Value {
    json!({
        "id": id,
        "name": clip(if name.is_empty() { id } else { name }, 80),
        "downloaded": downloaded,
        "failed": failed,
        "finished_at": finished,
        "error": clip(error, 200),
    })
}

fn cicc_row(stamp: &str, count: i64, name: &str) -> Value {
    json!({
        "id": "local-cicc-research",
        "name": clip(if name.is_empty() { "中金" } else { name }, 80),
        "downloaded": count.max(0),
        "failed": 0,
        "finished_at": parse_stamp(stamp),
        "error": "",
    })
}

fn cicc_name(local: &Value) -> String {
    local["libraries"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|item| item["slug"] == "cicc-research")
                .and_then(|item| item["name"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "中金".into())
}

pub(crate) fn iso_utc(ts: i64) -> String {
    let ts = ts.max(0);
    let days = ts.div_euclid(86400);
    let sod = ts.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    )
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year as i32, month as u32, day as u32)
}

pub fn next_shanghai(now: i64) -> i64 {
    let shanghai = now + 8 * 3600;
    let day = shanghai.div_euclid(86400);
    let sod = shanghai.rem_euclid(86400);
    for hour in HOURS {
        if sod < hour * 3600 {
            return day * 86400 + hour * 3600 - 8 * 3600;
        }
    }
    (day + 1) * 86400 + HOURS[0] * 3600 - 8 * 3600
}

fn parse_stamp(text: &str) -> i64 {
    let text = text.trim();
    if text.len() < 19 {
        return 0;
    }
    let year: i64 = text[0..4].parse().unwrap_or(0);
    let month: i64 = text[5..7].parse().unwrap_or(0);
    let day: i64 = text[8..10].parse().unwrap_or(0);
    let hour: i64 = text[11..13].parse().unwrap_or(0);
    let minute: i64 = text[14..16].parse().unwrap_or(0);
    let second: i64 = text[17..19].parse().unwrap_or(0);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return 0;
    }
    let days = days_from_civil(year, month as u32, day as u32);
    let utc = days * 86400 + hour * 3600 + minute * 60 + second;
    let rest = text[19..].trim();
    if rest.is_empty() || rest == "Z" {
        return utc;
    }
    let sign = rest.as_bytes().first().copied();
    if sign != Some(b'+') && sign != Some(b'-') || rest.len() < 6 {
        return utc;
    }
    let oh: i64 = rest[1..3].parse().unwrap_or(0);
    let om: i64 = rest[4..6].parse().unwrap_or(0);
    let offset = oh * 3600 + om * 60;
    if sign == Some(b'+') {
        utc - offset
    } else {
        utc + offset
    }
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    (era * 146097 + doe as i64) - 719468
}

fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

async fn json_setting(db: &Db, key: &str) -> Result<Value, String> {
    let raw = db
        .setting(key)
        .await
        .map_err(|err| err.to_string())?
        .unwrap_or_default();
    Ok(serde_json::from_str(&raw).unwrap_or(Value::Null))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_libraries_and_unconfigured_pull() {
        assert_eq!(next_shanghai(1_700_000_000), 1_700_010_000);
        let result = json!({
            "downloaded": 3,
            "failed": 1,
            "succeeded_groups": 1,
            "last_error": "超时",
            "group_results": [{"id": "kb1", "name": "宏观", "downloaded": 3, "failed": 1, "error": "x"}]
        });
        let runtime = json!({"kb1": {"last_finished_at": 50}});
        let local = json!({"libraries": [{"slug": "cicc-research", "name": "中金研究"}]});
        let status = assemble(
            "42",
            &result,
            &json!([]),
            &runtime,
            &local,
            "2020-01-01T00:00:00+00:00",
            4,
            &[],
            1_700_000_000,
        );
        assert_eq!(status["pull"]["configured"], false);
        assert_eq!(status["last_finished_at"], 42);
        assert_eq!(status["downloaded"], 3);
        assert_eq!(status["libraries"][0]["name"], "宏观");
        assert_eq!(status["libraries"][0]["finished_at"], 50);
        assert_eq!(status["libraries"][1]["id"], "local-cicc-research");
        assert_eq!(status["libraries"][1]["name"], "中金研究");
        assert_eq!(status["libraries"][1]["downloaded"], 4);
        assert_eq!(status["libraries"][1]["finished_at"], 1_577_836_800);
        let groups = json!([
            {"id": "kb2", "name": "策略", "enabled": true},
            {"id": "local-skip", "name": "跳过", "enabled": true},
            {"id": "kb3", "name": "停用", "enabled": false}
        ]);
        let fallback = json!({"failed_groups": ["kb2"], "group_errors": {"kb2": "断了"}});
        let rows = libraries(
            &groups,
            &json!({"kb2": {"last_finished_at": 9}}),
            &fallback,
            &[2],
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["downloaded"], 2);
        assert_eq!(rows[0]["failed"], 1);
        assert_eq!(rows[0]["error"], "断了");
        assert!(health_url("").is_none());
        assert!(health_url("file:///tmp/x").is_none());
        assert!(health_url("https://user@arm.example/pull").is_none());
        assert_eq!(
            health_url("https://arm.example/pull"),
            Some("https://arm.example/pull")
        );
    }
}
