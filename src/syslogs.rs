//! 管理页系统日志：内存里保留最近 2000 行，新的在前。
//! DEBUG 只匹配 DEBUG；其余级别包含更严重的行。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const RING_SIZE: usize = 2000;

fn ring() -> &'static Mutex<VecDeque<String>> {
    static RING: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_SIZE)))
}

pub struct SysLayer;

pub fn layer() -> SysLayer {
    SysLayer
}

impl<S: Subscriber> Layer<S> for SysLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        let meta = event.metadata();
        record(&format_line(level_name(meta.level()), meta.target(), &message));
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            if self.0.is_empty() {
                self.0.push_str(&format!("{value:?}"));
            }
            return;
        }
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0.push_str(&format!("{}={value:?}", field.name()));
    }
}

pub fn record(line: &str) {
    let mut ring = ring().lock().unwrap_or_else(|err| err.into_inner());
    if ring.len() == RING_SIZE {
        ring.pop_front();
    }
    ring.push_back(line.to_string());
}

pub fn recent(limit: i64, level: &str, q: &str) -> Result<Vec<String>, &'static str> {
    let limit = limit.clamp(10, 2000) as usize;
    let want = level.trim().to_ascii_uppercase();
    let minimum = match want.as_str() {
        "" => None,
        "DEBUG" => Some(10),
        "INFO" => Some(20),
        "WARNING" => Some(30),
        "ERROR" => Some(40),
        "CRITICAL" => Some(50),
        _ => return Err("level 需为 DEBUG/INFO/WARNING/ERROR/CRITICAL"),
    };
    let lines = ring().lock().unwrap_or_else(|err| err.into_inner()).iter().cloned().collect::<Vec<_>>();
    let mut matched = Vec::new();
    for line in lines {
        if let Some(minimum) = minimum {
            let Some(rank) = line_rank(&line) else { continue };
            if want == "DEBUG" {
                if rank != 10 {
                    continue;
                }
            } else if rank < minimum {
                continue;
            }
        }
        if !q.is_empty() && !line.to_ascii_lowercase().contains(&q.to_ascii_lowercase()) {
            continue;
        }
        matched.push(line);
    }
    let start = matched.len().saturating_sub(limit);
    let mut newest = matched[start..].to_vec();
    newest.reverse();
    Ok(newest)
}

fn line_rank(line: &str) -> Option<i32> {
    match line.split_whitespace().nth(2)? {
        "DEBUG" => Some(10),
        "INFO" => Some(20),
        "WARNING" => Some(30),
        "ERROR" => Some(40),
        "CRITICAL" => Some(50),
        _ => None,
    }
}

fn level_name(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::ERROR => "ERROR",
        tracing::Level::WARN => "WARNING",
        tracing::Level::INFO => "INFO",
        tracing::Level::DEBUG => "DEBUG",
        tracing::Level::TRACE => "TRACE",
    }
}

fn format_line(level: &str, target: &str, message: &str) -> String {
    format!("{} {level} {target} [main] {message}", timestamp())
}

fn timestamp() -> String {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let days = elapsed.as_secs() / 86_400;
    let seconds = elapsed.as_secs() % 86_400;
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60,
        elapsed.subsec_millis()
    )
}

fn civil_date(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = days_since_epoch as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097) as u64;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 { month_part + 3 } else { month_part - 9 };
    let year = year_of_era as i64 + era * 400 + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_exact_and_newer_lines_come_first() {
        let marker = format!("syslog-unit-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        for (level, text) in [("DEBUG", "debug line"), ("INFO", "info line"), ("WARNING", "warn line"), ("ERROR", "error line")] {
            record(&format!("2026-08-11 10:00:00.000 {level} app.t [t] {text} {marker}"));
        }
        assert_eq!(recent(200, "DEBUG", &marker).unwrap().len(), 1);
        assert_eq!(recent(200, "INFO", &marker).unwrap().len(), 3);
        assert_eq!(recent(200, "WARNING", &marker).unwrap().len(), 2);
        assert_eq!(recent(200, "ERROR", &marker).unwrap().len(), 1);
        assert_eq!(recent(200, "BOGUS", &marker).unwrap_err(), "level 需为 DEBUG/INFO/WARNING/ERROR/CRITICAL");
        let found = recent(200, "ERROR", &marker).unwrap();
        assert!(found[0].contains("error line"));
        assert!(recent(200, "", "missing-marker-not-written").unwrap().is_empty());
    }
}
