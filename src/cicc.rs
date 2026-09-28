//! 中金采集控制。只在归档目录里读写命令文件，不连存储机。
//! 没有可用的 status.json 时，若设置了 `CICC_LAB_LOG_DIR`，用 ARM 宿主机同步日志判断是否过期。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const MODES: &[&str] = &[
    "incr", "year", "all", "stop", "compress", "schedule", "settings", "backup",
];
const STALE_SECS: i64 = 300;
pub const CATEGORIES: &[&str] = &[
    "宏观经济",
    "市场策略",
    "全球研究",
    "行业研究",
    "公司研究",
    "量化及ESG",
    "大宗商品",
    "外汇研究",
    "固定收益",
    "中金研究院",
    "其他",
];

#[derive(Debug)]
pub enum CiccError {
    Isolated,
    Bad(&'static str),
    Invalid(String),
}

pub struct Control {
    ctrl: PathBuf,
    mounts: Option<String>,
}

pub fn from_env() -> Option<Control> {
    let root = std::env::var("IMA_ARCHIVE_ROOT").ok()?;
    let root = root.trim();
    if root.is_empty() {
        return None;
    }
    Some(Control::new(Path::new(root), None))
}

impl Control {
    pub fn new(archive_root: &Path, mounts: Option<String>) -> Self {
        Self {
            ctrl: archive_root.join("local").join(".cicc"),
            mounts,
        }
    }

    fn isolated(&self) -> bool {
        let text = self
            .mounts
            .clone()
            .or_else(|| fs::read_to_string("/proc/mounts").ok());
        let Some(text) = text else { return false };
        is_nfs(&self.ctrl, &text)
    }

    pub fn status(&self) -> Value {
        if self.isolated() {
            return json!({"available": false, "stale": true, "reason": "isolated"});
        }
        if let Some(mut data) = self.read_status_file() {
            let ts = data.get("ts").and_then(|value| value.as_i64()).unwrap_or(0);
            let stale = now_secs() - ts > STALE_SECS;
            data.insert("available".into(), json!(true));
            data.insert("stale".into(), json!(stale));
            return Value::Object(data);
        }
        match lab_config_from_env() {
            Some(cfg) => lab_status(&cfg, now_secs()),
            None => json!({"available": true, "stale": true}),
        }
    }

    fn read_status_file(&self) -> Option<serde_json::Map<String, Value>> {
        let text = fs::read_to_string(self.ctrl.join("status.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn trigger(
        &self,
        mode: &str,
        actor: &str,
        extra: Option<Value>,
    ) -> Result<Value, CiccError> {
        if !MODES.contains(&mode) {
            return Err(CiccError::Invalid(format!("未知操作：{mode}")));
        }
        if self.isolated() {
            return Err(CiccError::Isolated);
        }
        let cmds = self.ctrl.join("commands");
        fs::create_dir_all(&cmds).map_err(|_| CiccError::Bad("命令目录不可写"))?;
        let id = command_id();
        let name = format!("{}-{mode}-{}.json", now_millis(), &id[..8]);
        let mut payload = serde_json::Map::new();
        if let Some(Value::Object(extra)) = extra {
            payload.extend(extra);
        }
        let envelope =
            json!({"id": id, "mode": mode, "actor": actor, "ts": now_secs(), "payload": payload});
        let tmp = cmds.join(format!(".tmp.{}.{}", std::process::id(), &id[..8]));
        fs::write(&tmp, envelope.to_string()).map_err(|_| CiccError::Bad("命令写入失败"))?;
        fs::rename(&tmp, cmds.join(name)).map_err(|_| CiccError::Bad("命令写入失败"))?;
        Ok(json!({"queued": mode}))
    }

    pub fn read_schedule(&self) -> Value {
        if self.isolated() {
            return json!({"time": "03:00", "schedule_enabled": false});
        }
        let data = self.status();
        let time = data["storage"]["schedule"]["time"]
            .as_str()
            .unwrap_or("03:00");
        json!({"time": if time_ok(time) { time } else { "03:00" }, "schedule_enabled": self.schedule_enabled()})
    }

    pub fn set_schedule(&self, enabled: bool) -> Result<Value, CiccError> {
        if self.isolated() {
            return Err(CiccError::Isolated);
        }
        fs::create_dir_all(&self.ctrl).map_err(|_| CiccError::Bad("命令目录不可写"))?;
        let flag = self.ctrl.join("incremental.enabled");
        if enabled {
            let tmp = self.ctrl.join(".enabled.tmp");
            fs::write(&tmp, "1").map_err(|_| CiccError::Bad("开关写入失败"))?;
            fs::rename(tmp, &flag).map_err(|_| CiccError::Bad("开关写入失败"))?;
        } else {
            let _ = fs::remove_file(flag);
        }
        Ok(json!({"schedule_enabled": enabled}))
    }

    pub fn set_schedule_time(&self, time_of_day: &str, actor: &str) -> Result<Value, CiccError> {
        if !time_ok(time_of_day) {
            return Err(CiccError::Bad("时间格式应为 HH:mm（00:00-23:59）"));
        }
        self.trigger("schedule", actor, Some(json!({"time": time_of_day})))
    }

    pub fn set_settings(
        &self,
        categories: &[String],
        keywords: &[String],
        actor: &str,
    ) -> Result<Value, CiccError> {
        self.trigger(
            "settings",
            actor,
            Some(json!({"categories": categories, "keywords": keywords})),
        )
    }

    fn schedule_enabled(&self) -> bool {
        !self.isolated() && self.ctrl.join("incremental.enabled").exists()
    }
}

pub fn clean_lists(
    categories: &[String],
    keywords: &[String],
) -> Result<(Vec<String>, Vec<String>), CiccError> {
    let mut cats = Vec::new();
    for item in categories {
        let item = item.trim();
        if !item.is_empty() && !cats.iter().any(|kept: &String| kept == item) {
            cats.push(item.to_string());
        }
    }
    let unknown: Vec<&str> = cats
        .iter()
        .map(String::as_str)
        .filter(|item| !CATEGORIES.contains(item))
        .collect();
    if !unknown.is_empty() {
        return Err(CiccError::Invalid(format!(
            "未知品类：{}",
            unknown.join("、")
        )));
    }
    let mut words = Vec::new();
    for item in keywords {
        let item = item.trim();
        if !item.is_empty() && !words.iter().any(|kept: &String| kept == item) {
            words.push(item.to_string());
        }
    }
    Ok((cats, words))
}

pub fn time_ok(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.bytes().all(|b| b.is_ascii_digit())
        && minute.bytes().all(|b| b.is_ascii_digit())
        && hour.parse::<u8>().is_ok_and(|h| h <= 23)
        && minute.parse::<u8>().is_ok_and(|m| m <= 59)
}

fn is_nfs(path: &Path, mounts: &str) -> bool {
    let raw = path.to_string_lossy();
    let mut best = String::new();
    let mut kind = String::new();
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let Some(_) = parts.next() else { continue };
        let Some(point) = parts.next() else { continue };
        let Some(fstype) = parts.next() else { continue };
        let point = point.replace("\\040", " ").replace("\\011", "\t");
        let point = if point == "/" {
            "/".to_string()
        } else {
            point.trim_end_matches('/').to_string()
        };
        let hit = raw == point
            || raw.starts_with(&(point.clone() + "/"))
            || (point == "/" && raw.starts_with('/'));
        if hit && point.len() >= best.len() {
            best = point;
            kind = fstype.to_string();
        }
    }
    matches!(kind.as_str(), "nfs" | "nfs3" | "nfs4")
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn command_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).ok();
    hex::encode(bytes)
}

const LAB_STALE_HOURS: i64 = 36;
const SHANGHAI_OFFSET: i64 = 8 * 3600;

struct LabConfig {
    log_dir: PathBuf,
    manifest: Option<PathBuf>,
    stale_secs: i64,
}

enum ManifestAge {
    Off,
    Missing,
    Secs(i64),
}

struct HostLog {
    filename: String,
    last_run: Option<String>,
    last_rc: Option<i64>,
    age_secs: Option<i64>,
    readable: bool,
}

struct LogStamp {
    iso: String,
    unix: i64,
}

struct DoneLine {
    rc: i64,
    iso: String,
    unix: i64,
}

fn lab_config_from_env() -> Option<LabConfig> {
    let log_dir = nonempty_env("CICC_LAB_LOG_DIR")?;
    let manifest = nonempty_env("CICC_LAB_MANIFEST").map(PathBuf::from);
    let hours = nonempty_env("CICC_LAB_STALE_HOURS")
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|hours| *hours > 0)
        .unwrap_or(LAB_STALE_HOURS);
    Some(LabConfig {
        log_dir: PathBuf::from(log_dir),
        manifest,
        stale_secs: hours.saturating_mul(3600),
    })
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn lab_status(cfg: &LabConfig, now: i64) -> Value {
    let log = newest_host_log(&cfg.log_dir).map(|path| observe_log(&path, now));
    let manifest = manifest_age(cfg.manifest.as_deref(), now);
    let stale = lab_is_stale(log.as_ref(), manifest, cfg.stale_secs);
    let (last_run, last_rc, name, age_secs) = match log {
        Some(log) => (log.last_run, log.last_rc, Some(log.filename), log.age_secs),
        None => (None, None, None, None),
    };
    json!({
        "available": true,
        "stale": stale,
        "source": "arm-lab",
        "lab": {
            "last_run": last_run,
            "last_rc": last_rc,
            "log": name,
            "age_secs": age_secs
        }
    })
}

fn lab_is_stale(log: Option<&HostLog>, manifest: ManifestAge, threshold: i64) -> bool {
    let Some(log) = log else {
        return true;
    };
    if !log.readable || log.last_rc.is_some_and(|rc| rc != 0) {
        return true;
    }
    let age_stale = match log.age_secs {
        Some(age) => age > threshold,
        None => true,
    };
    if age_stale {
        return true;
    }
    match manifest {
        ManifestAge::Off => false,
        ManifestAge::Missing => true,
        ManifestAge::Secs(age) => age > threshold,
    }
}

fn newest_host_log(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(i64, String, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let os_name = entry.file_name();
        let name = os_name.to_string_lossy();
        if !name.starts_with("cicc-host-sync-") || !name.ends_with(".log") {
            continue;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(key) = filename_epoch(&name).or_else(|| mtime_secs(&path)) else {
            continue;
        };
        let replace = match &best {
            None => true,
            Some((prev_key, prev_name, _)) => {
                key > *prev_key || (key == *prev_key && name.as_ref() > prev_name.as_str())
            }
        };
        if replace {
            best = Some((key, name.into_owned(), path));
        }
    }
    best.map(|(_, _, path)| path)
}

fn filename_epoch(name: &str) -> Option<i64> {
    let inner = name.strip_prefix("cicc-host-sync-")?.strip_suffix(".log")?;
    if inner.len() != 15 || inner.as_bytes().get(8) != Some(&b'-') {
        return None;
    }
    let ymd = &inner[..8];
    let hms = &inner[9..];
    if !ymd.bytes().all(|byte| byte.is_ascii_digit())
        || !hms.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let year: i32 = ymd[0..4].parse().ok()?;
    let month: u32 = ymd[4..6].parse().ok()?;
    let day: u32 = ymd[6..8].parse().ok()?;
    let hour: u32 = hms[0..2].parse().ok()?;
    let minute: u32 = hms[2..4].parse().ok()?;
    let second: u32 = hms[4..6].parse().ok()?;
    let local = civil_to_unix(year, month, day, hour, minute, second)?;
    Some(local - SHANGHAI_OFFSET)
}

fn observe_log(path: &Path, now: i64) -> HostLog {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mtime_age = mtime_secs(path).map(|secs| age_between(now, secs));
    let Some(text) = read_log_edges(path) else {
        return HostLog {
            filename,
            last_run: None,
            last_rc: None,
            age_secs: mtime_age,
            readable: false,
        };
    };
    let (start, done) = parse_host_log(&text);
    let (last_run, last_unix, last_rc) = if let Some(done) = done {
        (Some(done.iso), Some(done.unix), Some(done.rc))
    } else if let Some(start) = start {
        (Some(start.iso), Some(start.unix), None)
    } else {
        (None, None, None)
    };
    HostLog {
        filename,
        last_run,
        last_rc,
        age_secs: last_unix.map(|unix| age_between(now, unix)).or(mtime_age),
        readable: true,
    }
}

fn parse_host_log(text: &str) -> (Option<LogStamp>, Option<DoneLine>) {
    let mut start = None;
    let mut done = None;
    for line in text.lines() {
        let line = line.trim();
        if start.is_none() {
            if let Some(rest) = line.strip_prefix("start ") {
                start = parse_iso_token(rest);
            }
        }
        if let Some(rest) = line.strip_prefix("done rc=") {
            if let Some(parsed) = parse_done(rest) {
                done = Some(parsed);
            }
        }
    }
    (start, done)
}

fn parse_done(rest: &str) -> Option<DoneLine> {
    let rest = rest.trim();
    let (rc, iso) = rest.split_once(char::is_whitespace)?;
    let rc = rc.parse().ok()?;
    let stamp = parse_iso_token(iso)?;
    Some(DoneLine {
        rc,
        iso: stamp.iso,
        unix: stamp.unix,
    })
}

fn parse_iso_token(text: &str) -> Option<LogStamp> {
    let token = text.split_whitespace().next()?.trim();
    let (body, offset) = split_zone(token)?;
    let body = match body.split_once('.') {
        Some((head, _)) => head,
        None => body,
    };
    let idx = body.find(['T', 't'])?;
    let date = &body[..idx];
    let time = &body[idx + 1..];
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: u32 = time_parts.next()?.parse().ok()?;
    let minute: u32 = time_parts.next()?.parse().ok()?;
    let second: u32 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some() {
        return None;
    }
    let local = civil_to_unix(year, month, day, hour, minute, second)?;
    Some(LogStamp {
        iso: token.to_string(),
        unix: local - offset,
    })
}

fn split_zone(token: &str) -> Option<(&str, i64)> {
    let t = token.find(['T', 't'])?;
    let time = &token[t + 1..];
    if let Some(rel) = time.find(['+', '-']) {
        let at = t + 1 + rel;
        let offset = parse_offset(&token[at..])?;
        return Some((&token[..at], offset));
    }
    if token.ends_with(['Z', 'z']) {
        return Some((&token[..token.len() - 1], 0));
    }
    None
}

fn parse_offset(raw: &str) -> Option<i64> {
    let (sign, rest) = raw.split_at_checked(1)?;
    let sign = match sign {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let (hour, minute) = if let Some((hour, minute)) = rest.split_once(':') {
        (hour, minute)
    } else if rest.len() == 4 {
        (&rest[..2], &rest[2..])
    } else if rest.len() == 2 {
        (rest, "00")
    } else {
        return None;
    };
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    Some(sign * (hour * 3600 + minute * 60))
}

fn read_log_edges(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const EDGE: u64 = 16 * 1024;
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return Some(String::new());
    }
    let head_n = EDGE.min(len) as usize;
    let mut head = vec![0u8; head_n];
    file.read_exact(&mut head).ok()?;
    let mut text = String::from_utf8_lossy(&head).into_owned();
    if len > EDGE {
        let tail_n = EDGE.min(len) as usize;
        file.seek(SeekFrom::End(-(tail_n as i64))).ok()?;
        let mut tail = vec![0u8; tail_n];
        file.read_exact(&mut tail).ok()?;
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&tail));
    }
    Some(text)
}

fn manifest_age(path: Option<&Path>, now: i64) -> ManifestAge {
    let Some(path) = path else {
        return ManifestAge::Off;
    };
    match mtime_secs(path) {
        Some(secs) => ManifestAge::Secs(age_between(now, secs)),
        None => ManifestAge::Missing,
    }
}

fn mtime_secs(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|dur| dur.as_secs() as i64)
}

fn age_between(now: i64, then: i64) -> i64 {
    now.saturating_sub(then).max(0)
}

fn civil_to_unix(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<i64> {
    let dim = days_in_month(year, month)?;
    if !(1970..=2100).contains(&year)
        || !(1..=dim).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap(year) { 29 } else { 28 }),
        _ => None,
    }
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u32;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era) * 146097 + i64::from(doe) - 719468
}

#[cfg(test)]
fn civil_from_days(mut z: i64) -> (i32, u32, u32) {
    z += 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
fn unix_to_parts(unix: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = unix.div_euclid(86400);
    let sod = unix.rem_euclid(86400) as u32;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, sod / 3600, (sod % 3600) / 60, sod % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "vpush-cicc-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn local_command_is_queued_and_remote_archive_is_refused() {
        let root = temp_root();
        let ctl = Control::new(&root, Some("".into()));
        fs::create_dir_all(root.join("local/.cicc")).unwrap();
        fs::write(
            root.join("local/.cicc/status.json"),
            r#"{"ts":1,"storage":{"schedule":{"time":"24:99"}}}"#,
        )
        .unwrap();
        let stale = ctl.status();
        assert_eq!(stale["available"], true);
        assert_eq!(stale["stale"], true);
        assert_eq!(ctl.read_schedule()["time"], "03:00");
        let queued = ctl.trigger("incr", "admin", None).unwrap();
        assert_eq!(queued["queued"], "incr");
        let file = fs::read_dir(root.join("local/.cicc/commands"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let saved: Value = serde_json::from_str(&fs::read_to_string(file.path()).unwrap()).unwrap();
        assert_eq!(saved["mode"], "incr");
        assert_eq!(saved["actor"], "admin");
        assert!(ctl.trigger("nope", "admin", None).is_err());
        assert_eq!(ctl.set_schedule(true).unwrap()["schedule_enabled"], true);
        assert_eq!(ctl.read_schedule()["schedule_enabled"], true);
        ctl.set_schedule_time("09:30", "admin").unwrap();
        assert_eq!(ctl.set_schedule(false).unwrap()["schedule_enabled"], false);
        assert!(ctl.set_schedule_time("24:00", "admin").is_err());
        let (cats, words) = clean_lists(
            &["宏观经济".into(), "宏观经济".into(), "  ".into()],
            &["黄金".into(), "黄金".into()],
        )
        .unwrap();
        assert_eq!(cats, vec!["宏观经济"]);
        assert_eq!(words, vec!["黄金"]);
        assert!(clean_lists(&["不存在".into()], &[]).is_err());
        let remote = Control::new(
            Path::new("/mnt/nas"),
            Some("host /mnt/nas nfs4 rw 0 0\n".into()),
        );
        assert!(remote.trigger("stop", "admin", None).is_err());
        assert_eq!(remote.status()["reason"], "isolated");
        let _ = fs::remove_dir_all(&root);
    }

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(String, Option<String>)>,
    }

    static LAB_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl EnvGuard {
        fn apply(pairs: &[(&str, Option<&str>)]) -> Self {
            let lock = LAB_ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            let saved = pairs
                .iter()
                .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
                .collect();
            for (key, value) in pairs {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn shanghai_iso(unix: i64) -> String {
        let (year, month, day, hour, minute, second) = unix_to_parts(unix + SHANGHAI_OFFSET);
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+08:00")
    }

    fn stamp_name(unix: i64) -> String {
        let (year, month, day, hour, minute, second) = unix_to_parts(unix + SHANGHAI_OFFSET);
        format!("cicc-host-sync-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.log")
    }

    fn write_host_log(dir: &Path, when: i64, rc: i64) -> String {
        let name = stamp_name(when);
        let body = format!(
            "start {} days=3 dry_run=0\nnoise token secret\ndone rc={rc} {}\n",
            shanghai_iso(when - 46),
            shanghai_iso(when)
        );
        fs::write(dir.join(&name), body).unwrap();
        name
    }

    fn touch_mtime(path: &Path, unix: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(unix))
            .unwrap();
    }

    fn lab_env(dir: &Path, manifest: Option<&Path>, hours: Option<&str>) -> EnvGuard {
        EnvGuard::apply(&[
            ("CICC_LAB_LOG_DIR", Some(dir.to_str().unwrap())),
            ("CICC_LAB_MANIFEST", manifest.and_then(|path| path.to_str())),
            ("CICC_LAB_STALE_HOURS", hours),
        ])
    }

    #[test]
    fn fresh_rc0_log_is_not_stale() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let when = now_secs() - 90;
        let name = write_host_log(&logs, when, 0);
        let _guard = lab_env(&logs, None, None);
        let ctl = Control::new(&root, Some(String::new()));
        let body = ctl.status();
        assert_eq!(body["available"], true);
        assert_eq!(body["stale"], false);
        assert_eq!(body["source"], "arm-lab");
        assert!(body.get("ts").is_none());
        assert_eq!(body["lab"]["log"], name);
        assert_eq!(body["lab"]["last_rc"], 0);
        assert_eq!(body["lab"]["last_run"], shanghai_iso(when));
        let age = body["lab"]["age_secs"].as_i64().unwrap();
        assert!((85..=120).contains(&age), "{age}");
        let rendered = body.to_string();
        assert!(!rendered.contains("dry_run"));
        assert!(!rendered.contains("noise"));
        assert!(!rendered.contains(logs.to_str().unwrap()));
        assert_eq!(ctl.read_schedule()["time"], "03:00");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn old_log_is_stale() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let when = now_secs() - 37 * 3600;
        let name = write_host_log(&logs, when, 0);
        let _guard = lab_env(&logs, None, None);
        let body = Control::new(&root, Some(String::new())).status();
        assert_eq!(body["stale"], true);
        assert_eq!(body["source"], "arm-lab");
        assert!(body.get("ts").is_none());
        assert_eq!(body["lab"]["log"], name);
        assert_eq!(body["lab"]["last_rc"], 0);
        let age = body["lab"]["age_secs"].as_i64().unwrap();
        assert!(age > 36 * 3600, "{age}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nonzero_rc_is_stale() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let name = write_host_log(&logs, now_secs() - 90, 2);
        let _guard = lab_env(&logs, None, None);
        let body = Control::new(&root, Some(String::new())).status();
        assert_eq!(body["available"], true);
        assert_eq!(body["stale"], true);
        assert_eq!(body["source"], "arm-lab");
        assert!(body.get("ts").is_none());
        assert_eq!(body["lab"]["last_rc"], 2);
        assert_eq!(body["lab"]["log"], name);
        let age = body["lab"]["age_secs"].as_i64().unwrap();
        assert!(age < 36 * 3600, "{age}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_log_is_stale() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let _guard = lab_env(&logs, None, None);
        let body = Control::new(&root, Some(String::new())).status();
        assert_eq!(
            body,
            json!({
                "available": true,
                "stale": true,
                "source": "arm-lab",
                "lab": {"last_run": null, "last_rc": null, "log": null, "age_secs": null}
            })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unset_lab_env_keeps_fallback() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        write_host_log(&logs, now_secs() - 90, 0);
        let _guard = EnvGuard::apply(&[
            ("CICC_LAB_LOG_DIR", None),
            ("CICC_LAB_MANIFEST", None),
            ("CICC_LAB_STALE_HOURS", None),
        ]);
        let body = Control::new(&root, Some(String::new())).status();
        assert_eq!(body, json!({"available": true, "stale": true}));
        drop(_guard);
        let _blank = EnvGuard::apply(&[
            ("CICC_LAB_LOG_DIR", Some("  ")),
            ("CICC_LAB_MANIFEST", Some(logs.to_str().unwrap())),
            ("CICC_LAB_STALE_HOURS", Some("1")),
        ]);
        let blank = Control::new(&root, Some(String::new())).status();
        assert_eq!(blank, json!({"available": true, "stale": true}));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn status_json_wins_over_lab_logs_and_isolated_still_wins() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(root.join("local/.cicc")).unwrap();
        fs::create_dir_all(&logs).unwrap();
        write_host_log(&logs, now_secs() - 40 * 3600, 7);
        let now = now_secs();
        fs::write(
            root.join("local/.cicc/status.json"),
            format!(r#"{{"ts":{now},"marker":"archive"}}"#),
        )
        .unwrap();
        let _guard = lab_env(&logs, None, None);
        let body = Control::new(&root, Some(String::new())).status();
        assert_eq!(body["stale"], false);
        assert_eq!(body["marker"], "archive");
        assert!(body.get("source").is_none());
        assert!(body.get("ts").is_some());
        fs::write(root.join("local/.cicc/status.json"), "not-json").unwrap();
        let fallen = Control::new(&root, Some(String::new())).status();
        assert_eq!(fallen["source"], "arm-lab");
        assert_eq!(fallen["stale"], true);
        assert!(fallen.get("ts").is_none());
        let remote = Control::new(
            Path::new("/mnt/nas"),
            Some("host /mnt/nas nfs4 rw 0 0\n".into()),
        );
        assert_eq!(remote.status()["reason"], "isolated");
        assert!(remote.status().get("source").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_sample_log_and_threshold_is_exclusive() {
        assert_eq!(
            parse_iso_token("2026-09-28T03:00:48+08:00").unwrap().unix,
            1_790_535_648
        );
        assert_eq!(shanghai_iso(1_790_535_648), "2026-09-28T03:00:48+08:00");
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let name = "cicc-host-sync-20260928-030002.log";
        let mut body = String::from("start 2026-09-28T03:00:02+08:00 days=3 dry_run=0\n");
        body.push_str(&"x".repeat(40_000));
        body.push_str("\ndone rc=0 2026-09-28T03:00:48+08:00\n");
        fs::write(logs.join(name), body).unwrap();
        let cfg = LabConfig {
            log_dir: logs,
            manifest: None,
            stale_secs: 36 * 3600,
        };
        let done = 1_790_535_648;
        let fresh = lab_status(&cfg, done + 36 * 3600);
        assert_eq!(fresh["stale"], false);
        assert_eq!(fresh["lab"]["age_secs"], 36 * 3600);
        assert_eq!(fresh["lab"]["last_run"], "2026-09-28T03:00:48+08:00");
        assert_eq!(fresh["lab"]["last_rc"], 0);
        assert_eq!(fresh["lab"]["log"], name);
        assert!(fresh.get("ts").is_none());
        assert!(!fresh.to_string().contains("dry_run"));
        assert!(!fresh.to_string().contains(&"x".repeat(8)));
        let stale = lab_status(&cfg, done + 36 * 3600 + 1);
        assert_eq!(stale["stale"], true);
        assert_eq!(stale["lab"]["age_secs"], 36 * 3600 + 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn newest_log_prefers_filename_timestamp_then_mtime() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let stamped = logs.join("cicc-host-sync-20260928-030002.log");
        fs::write(
            &stamped,
            "start 2026-09-28T03:00:02+08:00 days=1 dry_run=0\ndone rc=0 2026-09-28T03:00:48+08:00\n",
        )
        .unwrap();
        touch_mtime(&stamped, 1_600_000_000);
        let custom = logs.join("cicc-host-sync-custom.log");
        fs::write(
            &custom,
            "start 2024-01-01T00:00:00+08:00 days=1 dry_run=0\ndone rc=1 2024-01-01T00:00:01+08:00\n",
        )
        .unwrap();
        touch_mtime(&custom, 1_700_000_000);
        let cfg = LabConfig {
            log_dir: logs.clone(),
            manifest: None,
            stale_secs: 36 * 3600,
        };
        let body = lab_status(&cfg, 1_790_535_648 + 60);
        assert_eq!(body["lab"]["log"], "cicc-host-sync-20260928-030002.log");
        assert_eq!(body["lab"]["last_rc"], 0);
        let _ = fs::remove_dir_all(&logs);
        fs::create_dir_all(&logs).unwrap();
        let older = logs.join("cicc-host-sync-alpha.log");
        let newer = logs.join("cicc-host-sync-beta.log");
        fs::write(
            &older,
            "start 2026-09-28T03:00:02+08:00 x\ndone rc=1 2026-09-28T03:00:48+08:00\n",
        )
        .unwrap();
        fs::write(
            &newer,
            "start 2026-09-28T03:00:02+08:00 x\ndone rc=0 2026-09-28T03:00:48+08:00\n",
        )
        .unwrap();
        touch_mtime(&older, 1_700_000_000);
        touch_mtime(&newer, 1_800_000_000);
        let cfg = LabConfig {
            log_dir: logs,
            manifest: None,
            stale_secs: 36 * 3600,
        };
        let body = lab_status(&cfg, 1_790_535_648 + 60);
        assert_eq!(body["lab"]["log"], "cicc-host-sync-beta.log");
        assert_eq!(body["stale"], false);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn manifest_mtime_is_a_second_stale_signal() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("cicc-host-sync-20260928-030002.log"),
            "start 2026-09-28T03:00:02+08:00 days=3 dry_run=0\ndone rc=0 2026-09-28T03:00:48+08:00\n",
        )
        .unwrap();
        let now = 1_790_535_648 + 60;
        let cfg = LabConfig {
            log_dir: logs.clone(),
            manifest: None,
            stale_secs: 36 * 3600,
        };
        assert_eq!(lab_status(&cfg, now)["stale"], false);
        let manifest = root.join("compress_state_cicc.json");
        let missing = LabConfig {
            log_dir: logs.clone(),
            manifest: Some(manifest.clone()),
            stale_secs: 36 * 3600,
        };
        assert_eq!(lab_status(&missing, now)["stale"], true);
        fs::write(&manifest, "{}").unwrap();
        touch_mtime(&manifest, (now - 60) as u64);
        assert_eq!(lab_status(&missing, now)["stale"], false);
        touch_mtime(&manifest, (now - (36 * 3600 + 1)) as u64);
        let aged = lab_status(&missing, now);
        assert_eq!(aged["stale"], true);
        assert_eq!(aged["lab"]["age_secs"], 60);
        assert_eq!(aged["lab"]["last_rc"], 0);
        fs::write(
            logs.join("cicc-host-sync-20260928-030003.log"),
            "start 2026-09-28T03:00:02+08:00 days=3 dry_run=0\ndone rc=1 2026-09-28T03:00:48+08:00\n",
        )
        .unwrap();
        touch_mtime(&manifest, now as u64);
        let failed = lab_status(&missing, now);
        assert_eq!(failed["stale"], true);
        assert_eq!(failed["lab"]["last_rc"], 1);
        assert_eq!(failed["lab"]["log"], "cicc-host-sync-20260928-030003.log");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_hours_env_defaults_and_changes_threshold() {
        let root = temp_root();
        let logs = root.join("logs");
        fs::create_dir_all(&logs).unwrap();
        write_host_log(&logs, now_secs() - 2 * 3600, 0);
        let _guard = EnvGuard::apply(&[
            ("CICC_LAB_LOG_DIR", Some(logs.to_str().unwrap())),
            ("CICC_LAB_MANIFEST", None),
            ("CICC_LAB_STALE_HOURS", Some("0")),
        ]);
        assert_eq!(
            lab_config_from_env().unwrap().stale_secs,
            LAB_STALE_HOURS * 3600
        );
        let within_default = Control::new(&root, Some(String::new())).status();
        assert_eq!(within_default["stale"], false);
        drop(_guard);
        let _one = EnvGuard::apply(&[
            ("CICC_LAB_LOG_DIR", Some(logs.to_str().unwrap())),
            ("CICC_LAB_MANIFEST", None),
            ("CICC_LAB_STALE_HOURS", Some("1")),
        ]);
        assert_eq!(lab_config_from_env().unwrap().stale_secs, 3600);
        let tightened = Control::new(&root, Some(String::new())).status();
        assert_eq!(tightened["stale"], true);
        assert!(tightened.get("ts").is_none());
        drop(_one);
        let _bad = EnvGuard::apply(&[
            ("CICC_LAB_LOG_DIR", Some(logs.to_str().unwrap())),
            ("CICC_LAB_MANIFEST", None),
            ("CICC_LAB_STALE_HOURS", Some("nope")),
        ]);
        assert_eq!(
            lab_config_from_env().unwrap().stale_secs,
            LAB_STALE_HOURS * 3600
        );
        drop(_bad);
        let _unset = EnvGuard::apply(&[("CICC_LAB_LOG_DIR", None)]);
        assert!(lab_config_from_env().is_none());
        let _ = fs::remove_dir_all(&root);
    }
}
