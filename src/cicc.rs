//! 中金采集控制。只在归档目录里读写命令文件，不连存储机。

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
        let Ok(text) = fs::read_to_string(self.ctrl.join("status.json")) else {
            return json!({"available": true, "stale": true});
        };
        let Ok(mut data) = serde_json::from_str::<serde_json::Map<String, Value>>(&text) else {
            return json!({"available": true, "stale": true});
        };
        let ts = data.get("ts").and_then(|value| value.as_i64()).unwrap_or(0);
        let stale = now_secs() - ts > STALE_SECS;
        data.insert("available".into(), json!(true));
        data.insert("stale".into(), json!(stale));
        Value::Object(data)
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
}
