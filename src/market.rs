//! 时间线右侧的指数报价。数据来自腾讯行情，缓存 30 秒；冷缓存同步取，失败不编造价格。

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

const QUOTE_URL: &str = "https://qt.gtimg.cn/q=";
const DAY: &[(&str, &str)] = &[
    ("sh000001", "上证指数"),
    ("sz399001", "深证成指"),
    ("sh000688", "科创50"),
    ("sz399006", "创业板"),
    ("hkHSI", "恒生指数"),
    ("hkHSTECH", "恒生科技"),
];
const NIGHT: &[(&str, &str)] = &[
    ("us.INX", "标普 500 指数"),
    ("us.IXIC", "纳斯达克指数"),
    ("us.NDX", "纳斯达克 100"),
    ("us.DJI", "道琼斯指数"),
    ("usSOXX", "SOXX"),
    ("usYINN", "YINN"),
];

#[derive(Clone)]
struct Point {
    time: String,
    minute: i64,
    price: f64,
}

#[derive(Clone)]
struct Intraday {
    date: String,
    duration: i64,
    points: Vec<Point>,
}

#[derive(Clone)]
struct Item {
    symbol: String,
    name: String,
    price: Option<f64>,
    previous_close: Option<f64>,
    change: Option<f64>,
    percent: Option<f64>,
    quoted_at: Option<String>,
    stale: bool,
    intraday: Option<Intraday>,
    intraday_stale: bool,
}

struct Slot {
    items: Vec<Item>,
    at: Option<Instant>,
    refreshing: bool,
}

struct Civil {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

enum Start {
    Cold,
    Warm,
    Idle,
}

pub async fn snapshot(group: &str) -> Value {
    let now = now_unix();
    let group = if group == "auto" {
        default_group(now)
    } else {
        group
    };
    let (start, previous) = begin(group);
    match start {
        Start::Cold => {
            let fetched = tokio::task::spawn_blocking({
                let previous = previous.clone();
                let group = group.to_string();
                move || fetch_group(&group, &previous)
            })
            .await
            .unwrap_or(previous);
            finish(group, fetched);
        }
        Start::Warm => {
            let previous = previous.clone();
            let group = group.to_string();
            tokio::spawn(async move {
                let fetched = tokio::task::spawn_blocking({
                    let group = group.clone();
                    move || fetch_group(&group, &previous)
                })
                .await
                .unwrap_or_default();
                if fetched.is_empty() {
                    mark_stale(&group);
                } else {
                    finish(&group, fetched);
                }
            });
        }
        Start::Idle => {}
    }
    render(group, &current(group), now)
}

fn begin(group: &str) -> (Start, Vec<Item>) {
    let mut slots = slots().lock().unwrap_or_else(|err| err.into_inner());
    let slot = slots.entry(group.to_string()).or_insert_with(|| Slot {
        items: Vec::new(),
        at: None,
        refreshing: false,
    });
    let start = if slot.refreshing {
        Start::Idle
    } else if slot.at.is_none() {
        Start::Cold
    } else if slot
        .at
        .is_some_and(|at| at.elapsed() >= Duration::from_secs(30))
    {
        Start::Warm
    } else {
        Start::Idle
    };
    if !matches!(start, Start::Idle) {
        slot.refreshing = true;
        if slot.items.is_empty() {
            slot.items = placeholders(group);
        }
    }
    (start, slot.items.clone())
}

fn finish(group: &str, items: Vec<Item>) {
    let mut slots = slots().lock().unwrap_or_else(|err| err.into_inner());
    let slot = slots.entry(group.to_string()).or_insert_with(|| Slot {
        items: Vec::new(),
        at: None,
        refreshing: false,
    });
    slot.items = items;
    slot.at = Some(Instant::now());
    slot.refreshing = false;
}

fn mark_stale(group: &str) {
    let mut slots = slots().lock().unwrap_or_else(|err| err.into_inner());
    if let Some(slot) = slots.get_mut(group) {
        for item in &mut slot.items {
            item.stale = true;
        }
        slot.at = Some(Instant::now());
        slot.refreshing = false;
    }
}

fn current(group: &str) -> Vec<Item> {
    let slots = slots().lock().unwrap_or_else(|err| err.into_inner());
    slots
        .get(group)
        .map(|slot| slot.items.clone())
        .unwrap_or_else(|| placeholders(group))
}

fn slots() -> &'static Mutex<HashMap<String, Slot>> {
    static SLOTS: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn placeholders(group: &str) -> Vec<Item> {
    symbols(group)
        .iter()
        .map(|(symbol, name)| Item {
            symbol: (*symbol).to_string(),
            name: (*name).to_string(),
            price: None,
            previous_close: None,
            change: None,
            percent: None,
            quoted_at: None,
            stale: true,
            intraday: None,
            intraday_stale: true,
        })
        .collect()
}

fn fetch_group(group: &str, previous: &[Item]) -> Vec<Item> {
    let names = symbols(group);
    let url = format!(
        "{QUOTE_URL}{}",
        names
            .iter()
            .map(|(symbol, _)| *symbol)
            .collect::<Vec<_>>()
            .join(",")
    );
    let fresh = http_text(&url).and_then(|text| parse_quotes(&text, group).ok());
    let mut items = Vec::new();
    for (symbol, name) in names {
        let mut item = previous
            .iter()
            .find(|item| item.symbol == *symbol)
            .cloned()
            .unwrap_or_else(|| placeholder(symbol, name));
        item.name = (*name).to_string();
        if let Some(quote) = fresh
            .as_ref()
            .and_then(|rows| rows.iter().find(|row| row.symbol == *symbol))
        {
            item.price = Some(quote.price);
            item.previous_close = Some(quote.previous_close);
            item.change = Some(quote.change);
            item.percent = Some(quote.percent);
            item.quoted_at = Some(quote.quoted_at.clone());
            item.stale = false;
        } else {
            item.stale = true;
        }
        items.push(item);
    }
    let Some(fresh) = fresh else {
        return items;
    };
    let symbols = fresh
        .iter()
        .map(|row| row.symbol.clone())
        .collect::<Vec<_>>();
    let intradays = std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for symbol in &symbols {
            jobs.push(scope.spawn(|| {
                let payload = http_text(&minute_url(symbol))
                    .and_then(|text| serde_json::from_str(&text).ok());
                (
                    symbol.clone(),
                    payload.and_then(|value| parse_intraday(&value, symbol)),
                )
            }));
        }
        jobs.into_iter()
            .map(|job| job.join().unwrap_or_else(|_| (String::new(), None)))
            .collect::<Vec<_>>()
    });
    for (symbol, intraday) in intradays {
        let Some(item) = items.iter_mut().find(|item| item.symbol == symbol) else {
            continue;
        };
        let quote_date = item
            .quoted_at
            .as_deref()
            .unwrap_or("")
            .get(..10)
            .unwrap_or("");
        let valid = intraday
            .as_ref()
            .is_some_and(|series| series.date == quote_date);
        if valid {
            item.intraday = intraday;
        } else if item
            .intraday
            .as_ref()
            .is_some_and(|series| series.date != quote_date)
        {
            item.intraday = None;
        }
        item.intraday_stale = !valid;
    }
    items
}

fn placeholder(symbol: &str, name: &str) -> Item {
    Item {
        symbol: symbol.to_string(),
        name: name.to_string(),
        price: None,
        previous_close: None,
        change: None,
        percent: None,
        quoted_at: None,
        stale: true,
        intraday: None,
        intraday_stale: true,
    }
}

fn minute_url(symbol: &str) -> String {
    let market = if symbol.starts_with("us") {
        "UsMinute"
    } else {
        "minute"
    };
    format!(
        "https://web.ifzq.gtimg.cn/appstock/app/{market}/query?code={}",
        minute_code(symbol)
    )
}

fn minute_code(symbol: &str) -> &'static str {
    match symbol {
        "usSOXX" => "usSOXX.OQ",
        "usYINN" => "usYINN.AM",
        "sh000001" => "sh000001",
        "sz399001" => "sz399001",
        "sh000688" => "sh000688",
        "sz399006" => "sz399006",
        "hkHSI" => "hkHSI",
        "hkHSTECH" => "hkHSTECH",
        "us.INX" => "us.INX",
        "us.IXIC" => "us.IXIC",
        "us.NDX" => "us.NDX",
        "us.DJI" => "us.DJI",
        _ => "",
    }
}

fn http_text(url: &str) -> Option<String> {
    http_text_with_referer(url, "")
}

pub(crate) fn http_text_with_referer(url: &str, referer: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(8))
        .build();
    let resp = agent.get(url).set("Referer", referer).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().read_to_end(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| *b as char).collect())
}

struct Quote {
    symbol: String,
    price: f64,
    previous_close: f64,
    change: f64,
    percent: f64,
    quoted_at: String,
}

fn parse_quotes(text: &str, group: &str) -> Result<Vec<Quote>, ()> {
    let records = quote_records(text);
    let mut items = Vec::new();
    for (symbol, _) in symbols(group) {
        let Some(fields) = records.get(*symbol) else {
            continue;
        };
        let parts = fields.split('~').collect::<Vec<_>>();
        if parts.len() < 33 || parts[2] != expected_code(symbol) {
            continue;
        }
        let Some(price) = finite(parts[3]) else {
            continue;
        };
        let Some(previous) = finite(parts[4]) else {
            continue;
        };
        let Some(change) = finite(parts[31]) else {
            continue;
        };
        let Some(percent) = finite(parts[32]) else {
            continue;
        };
        if price.min(previous) <= 0.0 {
            continue;
        }
        let Some(quoted_at) = exchange_time(symbol, parts[30]) else {
            continue;
        };
        items.push(Quote {
            symbol: (*symbol).to_string(),
            price,
            previous_close: previous,
            change,
            percent,
            quoted_at,
        });
    }
    if items.is_empty() {
        Err(())
    } else {
        Ok(items)
    }
}

pub(crate) fn quote_records(text: &str) -> HashMap<&str, &str> {
    let mut out = HashMap::new();
    for chunk in text.split("v_") {
        let Some((symbol, rest)) = chunk.split_once("=\"") else {
            continue;
        };
        let Some(value) = rest.split_once("\";") else {
            continue;
        };
        if !symbol.is_empty() && !value.0.contains(['\r', '\n']) {
            out.insert(symbol, value.0);
        }
    }
    out
}

fn expected_code(symbol: &str) -> &str {
    &minute_code(symbol)[2..]
}

fn finite(text: &str) -> Option<f64> {
    let value = text.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

fn exchange_time(symbol: &str, raw: &str) -> Option<String> {
    let civil = if symbol.starts_with("us") {
        parse_separated(raw, '-', ' ')?
    } else if symbol.starts_with("hk") {
        parse_separated(raw, '/', ' ')?
    } else {
        parse_compact(raw)?
    };
    let offset = if symbol.starts_with("us") {
        ny_offset_wall(&civil)
    } else {
        8 * 60
    };
    Some(format_iso(&civil, offset))
}

fn parse_compact(raw: &str) -> Option<Civil> {
    if raw.len() != 14 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Civil {
        year: raw[0..4].parse().ok()?,
        month: raw[4..6].parse().ok()?,
        day: raw[6..8].parse().ok()?,
        hour: raw[8..10].parse().ok()?,
        minute: raw[10..12].parse().ok()?,
        second: raw[12..14].parse().ok()?,
    })
}

fn parse_separated(raw: &str, date_sep: char, date_time_sep: char) -> Option<Civil> {
    let (date, time) = raw.split_once(date_time_sep)?;
    let mut date = date.split(date_sep);
    let mut time = time.split(':');
    Some(Civil {
        year: date.next()?.parse().ok()?,
        month: date.next()?.parse().ok()?,
        day: date.next()?.parse().ok()?,
        hour: time.next()?.parse().ok()?,
        minute: time.next()?.parse().ok()?,
        second: time.next()?.parse().ok()?,
    })
}

fn parse_intraday(payload: &Value, symbol: &str) -> Option<Intraday> {
    let data = &payload["data"][minute_code(symbol)]["data"];
    let raw_date = data["date"].as_str()?;
    let date = parse_compact(&format!("{raw_date}000000"))?;
    if raw_date.len() != 8 {
        return None;
    }
    let date = format!("{:04}-{:02}-{:02}", date.year, date.month, date.day);
    let us = symbol.starts_with("us");
    let hk = symbol.starts_with("hk");
    let end = if us || hk { 960 } else { 900 };
    let lunch = if hk { 720 } else { 690 };
    let duration = end - 570 - if us { 0 } else { 780 - lunch };
    let mut by_time = HashMap::<String, Point>::new();
    for row in data["data"].as_array().into_iter().flatten() {
        let Some(text) = row.as_str() else { continue };
        let fields = text.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 2 {
            continue;
        }
        let clock = fields[0];
        if clock.len() != 4 || !clock.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(hour) = clock[0..2].parse::<i64>() else {
            continue;
        };
        let Ok(minute_part) = clock[2..4].parse::<i64>() else {
            continue;
        };
        let minute = hour * 60 + minute_part;
        let Some(price) = finite(fields[1]) else {
            continue;
        };
        if price <= 0.0 || !(570..=end).contains(&minute) {
            continue;
        }
        if !us && lunch < minute && minute < 780 {
            continue;
        }
        let offset = minute - 570 - if !us && minute >= 780 { 780 - lunch } else { 0 };
        let time = format!("{:02}:{:02}", hour, minute_part);
        by_time.insert(
            time.clone(),
            Point {
                time,
                minute: offset,
                price,
            },
        );
    }
    let mut points = by_time.into_values().collect::<Vec<_>>();
    points.sort_by(|a, b| a.time.cmp(&b.time));
    if points.is_empty() {
        None
    } else {
        Some(Intraday {
            date,
            duration,
            points,
        })
    }
}

fn render(group: &str, items: &[Item], now: i64) -> Value {
    let rows = items
        .iter()
        .map(|item| item_json(item, now))
        .collect::<Vec<_>>();
    json!({
        "group": group,
        "items": rows,
        "stale": rows.iter().any(|row| row["stale"] == true),
    })
}

fn item_json(item: &Item, now: i64) -> Value {
    let status = quote_status(&item.symbol, item.quoted_at.as_deref(), now);
    let mut lagging = false;
    if status == "trading" {
        if let (Some(quoted), Some(series)) = (&item.quoted_at, &item.intraday) {
            if let Some(last) = series.points.last() {
                let point = format!(
                    "{}T{}:00{}",
                    series.date,
                    last.time,
                    quoted.get(19..).unwrap_or("+08:00")
                );
                if let (Some(quoted_at), Some(last_at)) =
                    (parse_iso_unix(quoted), parse_iso_unix(&point))
                {
                    lagging = quoted_at - last_at > 180;
                }
            }
        }
    }
    let mut row = Map::new();
    row.insert("symbol".into(), json!(item.symbol));
    row.insert("name".into(), json!(item.name));
    if let Some(price) = item.price {
        row.insert("price".into(), json!(price));
        row.insert("previous_close".into(), json!(item.previous_close));
        row.insert("change".into(), json!(item.change));
        row.insert("percent".into(), json!(item.percent));
        row.insert("quoted_at".into(), json!(item.quoted_at));
    }
    row.insert("stale".into(), json!(item.stale));
    row.insert("status".into(), json!(status));
    if let Some(series) = &item.intraday {
        row.insert(
            "intraday".into(),
            json!({
                "date": series.date,
                "duration": series.duration,
                "points": series.points.iter().map(|point| json!({
                    "time": point.time,
                    "minute": point.minute,
                    "price": point.price,
                })).collect::<Vec<_>>(),
            }),
        );
    }
    row.insert(
        "intraday_stale".into(),
        json!(item.intraday_stale || item.stale || lagging),
    );
    Value::Object(row)
}

fn quote_status(symbol: &str, quoted_at: Option<&str>, now: i64) -> &'static str {
    let us = symbol.starts_with("us");
    let local = if us { to_ny(now) } else { to_cn(now) };
    let minute = local.hour * 60 + local.minute;
    let end = if symbol.starts_with("hk") || us {
        960
    } else {
        900
    };
    let lunch = if symbol.starts_with("hk") { 720 } else { 690 };
    if us && is_us_holiday(local.year, local.month, local.day) {
        return "holiday";
    }
    if weekday(local.year, local.month, local.day) >= 5 || minute < 570 || minute >= end {
        return "closed";
    }
    if !us && lunch < minute && minute < 780 {
        return "break";
    }
    let Some(quoted) = quoted_at.and_then(parse_iso_unix) else {
        return "unavailable";
    };
    let age = now - quoted;
    if (0..=180).contains(&age) {
        "trading"
    } else {
        "delayed"
    }
}

fn default_group(now: i64) -> &'static str {
    let hour = to_cn(now).hour;
    if (8..20).contains(&hour) {
        "day"
    } else {
        "night"
    }
}

fn symbols(group: &str) -> &'static [(&'static str, &'static str)] {
    if group == "night" {
        NIGHT
    } else {
        DAY
    }
}

fn is_us_holiday(year: i32, month: u32, day: u32) -> bool {
    let mut days = Vec::new();
    for check_year in [year - 1, year, year + 1] {
        for (holiday_month, holiday_day) in [(1, 1), (7, 4), (12, 25)] {
            days.push(observed(check_year, holiday_month, holiday_day));
        }
    }
    if year >= 2022 {
        days.push(observed(year, 6, 19));
    }
    days.push(nth_date(year, 1, 0, 3));
    days.push(nth_date(year, 2, 0, 3));
    days.push(last_date(year, 5, 0));
    days.push(nth_date(year, 9, 0, 1));
    days.push(nth_date(year, 11, 3, 4));
    days.push(shift(easter(year), -2));
    days.contains(&(year, month, day))
}

fn observed(year: i32, month: u32, day: u32) -> (i32, u32, u32) {
    match weekday(year, month, day) {
        5 => shift((year, month, day), -1),
        6 => shift((year, month, day), 1),
        _ => (year, month, day),
    }
}

fn nth_date(year: i32, month: u32, weekday_wanted: u32, occurrence: u32) -> (i32, u32, u32) {
    let first = weekday(year, month, 1);
    let delta = (weekday_wanted + 7 - first) % 7;
    let day = 1 + delta + (occurrence - 1) * 7;
    (year, month, day)
}

fn last_date(year: i32, month: u32, weekday_wanted: u32) -> (i32, u32, u32) {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let (year, month, day) = shift((next_year, next_month, 1), -1);
    let back = (weekday(year, month, day) + 7 - weekday_wanted) % 7;
    shift((year, month, day), -(back as i32))
}

fn easter(year: i32) -> (i32, u32, u32) {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let g = (b - (b + 8) / 25 + 1) / 3;
    let h = (19 * a + b - d - g + 15).rem_euclid(30);
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k).rem_euclid(7);
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114).rem_euclid(31) + 1;
    (year, month as u32, day as u32)
}

fn shift(date: (i32, u32, u32), delta: i32) -> (i32, u32, u32) {
    civil_from_days(days_from_civil(date.0, date.1, date.2) + delta as i64)
}

fn weekday(year: i32, month: u32, day: u32) -> u32 {
    (days_from_civil(year, month, day) + 3).rem_euclid(7) as u32
}

fn to_cn(unix: i64) -> Civil {
    from_unix(unix + 8 * 3600)
}

fn to_ny(unix: i64) -> Civil {
    let est = from_unix(unix - 5 * 3600);
    let offset = if dst_from_est(&est) {
        -4 * 3600
    } else {
        -5 * 3600
    };
    from_unix(unix + offset)
}

fn dst_from_est(civil: &Civil) -> bool {
    let start = nth_date(civil.year, 3, 6, 2).2;
    let end = nth_date(civil.year, 11, 6, 1).2;
    let key = wall_key(civil.month, civil.day, civil.hour, civil.minute);
    key >= wall_key(3, start, 2, 0) && key < wall_key(11, end, 1, 0)
}

fn ny_offset_wall(civil: &Civil) -> i32 {
    let start = nth_date(civil.year, 3, 6, 2).2;
    let end = nth_date(civil.year, 11, 6, 1).2;
    let key = wall_key(civil.month, civil.day, civil.hour, civil.minute);
    if key >= wall_key(3, start, 2, 0) && key < wall_key(11, end, 2, 0) {
        -4 * 60
    } else {
        -5 * 60
    }
}

fn wall_key(month: u32, day: u32, hour: u32, minute: u32) -> u32 {
    month * 1_000_000 + day * 10_000 + hour * 100 + minute
}

fn from_unix(local: i64) -> Civil {
    let days = local.div_euclid(86400);
    let sod = local.rem_euclid(86400) as u32;
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: sod / 3600,
        minute: (sod % 3600) / 60,
        second: sod % 60,
    }
}

fn format_iso(civil: &Civil, offset_min: i32) -> String {
    let sign = if offset_min >= 0 { '+' } else { '-' };
    let abs = offset_min.abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        civil.year,
        civil.month,
        civil.day,
        civil.hour,
        civil.minute,
        civil.second,
        abs / 60,
        abs % 60
    )
}

fn parse_iso_unix(text: &str) -> Option<i64> {
    if text.len() < 19 {
        return None;
    }
    let civil = Civil {
        year: text[0..4].parse().ok()?,
        month: text[5..7].parse().ok()?,
        day: text[8..10].parse().ok()?,
        hour: text[11..13].parse().ok()?,
        minute: text[14..16].parse().ok()?,
        second: text[17..19].parse().ok()?,
    };
    let offset = if text.len() >= 25 {
        let sign = if text.as_bytes().get(19) == Some(&b'-') {
            -1
        } else {
            1
        };
        let hour: i64 = text[20..22].parse().ok()?;
        let minute: i64 = text[23..25].parse().ok()?;
        sign * (hour * 60 + minute) * 60
    } else {
        8 * 3600
    };
    Some(civil_unix(&civil) - offset)
}

fn civil_unix(civil: &Civil) -> i64 {
    days_from_civil(civil.year, civil.month, civil.day) * 86400
        + civil.hour as i64 * 3600
        + civil.minute as i64 * 60
        + civil.second as i64
}

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn mainland_quote_time(raw: &str) -> Option<(String, i64)> {
    let civil = parse_compact(raw)?;
    if !(1..=12).contains(&civil.month)
        || !(1..=31).contains(&civil.day)
        || civil.hour > 23
        || civil.minute > 59
        || civil.second > 59
    {
        return None;
    }
    let unix = civil_unix(&civil);
    let normalized = from_unix(unix);
    if (civil.year, civil.month, civil.day) != (normalized.year, normalized.month, normalized.day) {
        return None;
    }
    Some((format_iso(&civil, 480), unix - 8 * 3600))
}

pub(crate) fn mainland_open(now: i64) -> bool {
    let local = now + 8 * 3600;
    let weekday = (local.div_euclid(86400) + 3).rem_euclid(7);
    let second = local.rem_euclid(86400);
    weekday < 5 && ((34200..=41400).contains(&second) || (46800..=54000).contains(&second))
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = (year - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    era as i64 * 146097 + doe as i64 - 719468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
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
mod tests {
    use super::*;

    fn payload(price: &str, timestamp: &str, group: &str) -> String {
        let mut rows = Vec::new();
        for (symbol, name) in symbols(group) {
            let mut fields = vec![String::new(); 33];
            fields[1] = (*name).to_string();
            fields[2] = expected_code(symbol).to_string();
            fields[3] = price.to_string();
            fields[4] = "3942.09".into();
            let formatted = if timestamp != "bad-time"
                && (symbol.starts_with("hk") || symbol.starts_with("us"))
            {
                let civil = parse_compact(timestamp).unwrap();
                if symbol.starts_with("hk") {
                    format!(
                        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
                        civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
                    )
                } else {
                    format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                        civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
                    )
                }
            } else {
                timestamp.to_string()
            };
            fields[30] = formatted;
            fields[31] = "-11.97".into();
            fields[32] = "-0.30".into();
            rows.push(format!("v_{symbol}=\"{}\";", fields.join("~")));
        }
        rows.join("\n")
    }

    fn cn_unix(text: &str) -> i64 {
        parse_iso_unix(&format!("{text}+08:00")).unwrap()
    }

    #[test]
    fn quotes_keep_order_exchange_time_and_drop_bad_rows() {
        let items = parse_quotes(&payload("3930.12", "20260904103000", "day"), "day").unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item.symbol.as_str())
                .collect::<Vec<_>>(),
            symbols("day").iter().map(|row| row.0).collect::<Vec<_>>()
        );
        assert_eq!(items[0].quoted_at, "2026-09-04T10:30:00+08:00");
        assert!((items[0].price - 3930.12).abs() < 1e-9);
        assert!((items[0].percent + 0.30).abs() < 1e-9);
        for sample in [
            "",
            "v_sh000001=\"\";",
            &payload("NaN", "20260904103000", "day"),
            &payload("inf", "20260904103000", "day"),
            &payload("0", "20260904103000", "day"),
            &payload("1", "bad-time", "day"),
        ] {
            assert!(parse_quotes(sample, "day").is_err(), "{sample}");
        }
        let broken = payload("3930.12", "20260904103000", "day").replace("~399001~", "~000001~");
        let items = parse_quotes(&broken, "day").unwrap();
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|item| item.symbol != "sz399001"));
    }

    #[test]
    fn session_status_holidays_and_us_time() {
        let quoted = "2026-09-04T10:30:00+08:00";
        assert_eq!(
            quote_status("sh000001", Some(quoted), cn_unix("2026-09-04T10:31:00")),
            "trading"
        );
        assert_eq!(
            quote_status("sh000001", Some(quoted), cn_unix("2026-09-04T10:34:00")),
            "delayed"
        );
        assert_eq!(
            quote_status("sh000001", Some(quoted), cn_unix("2026-09-04T12:00:00")),
            "break"
        );
        assert_eq!(
            quote_status("sh000001", Some(quoted), cn_unix("2026-09-04T15:00:00")),
            "closed"
        );
        assert_eq!(
            quote_status("sh000001", Some(quoted), cn_unix("2026-09-06T10:00:00")),
            "closed"
        );
        assert_eq!(
            quote_status("sh000001", Some(quoted), cn_unix("2026-09-07T10:00:00")),
            "delayed"
        );
        assert_eq!(
            quote_status(
                "hkHSI",
                Some("2026-09-04T15:30:00+08:00"),
                cn_unix("2026-09-04T15:30:00")
            ),
            "trading"
        );
        assert_eq!(
            quote_status(
                "hkHSI",
                Some("2026-09-04T12:30:00+08:00"),
                cn_unix("2026-09-04T12:30:00")
            ),
            "break"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-09-04T21:30:00+08:00"),
                cn_unix("2026-09-04T21:30:00")
            ),
            "trading"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-12-04T21:30:00+08:00"),
                cn_unix("2026-12-04T21:30:00")
            ),
            "closed"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-12-04T22:30:00+08:00"),
                cn_unix("2026-12-04T22:30:00")
            ),
            "trading"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-09-05T02:00:00+08:00"),
                cn_unix("2026-09-05T02:00:00")
            ),
            "trading"
        );
        let ny = |text: &str| parse_iso_unix(text).unwrap();
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-09-04T16:00:00-04:00"),
                ny("2026-09-07T15:00:00-04:00")
            ),
            "holiday"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-09-04T16:00:00-04:00"),
                ny("2026-09-08T15:00:00-04:00")
            ),
            "delayed"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2021-12-30T16:00:00-05:00"),
                ny("2021-12-31T15:00:00-05:00")
            ),
            "holiday"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2026-05-22T16:00:00-04:00"),
                ny("2026-05-25T15:00:00-04:00")
            ),
            "holiday"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2021-06-17T16:00:00-04:00"),
                ny("2021-06-18T15:00:00-04:00")
            ),
            "delayed"
        );
        assert_eq!(
            quote_status(
                "us.INX",
                Some("2022-06-17T16:00:00-04:00"),
                ny("2022-06-20T15:00:00-04:00")
            ),
            "holiday"
        );
        for day in [
            "2026-04-03",
            "2026-02-16",
            "2026-05-25",
            "2021-07-05",
            "2026-09-07",
            "2026-11-26",
            "2021-12-24",
        ] {
            let civil = parse_compact(&format!("{}000000", day.replace('-', ""))).unwrap();
            assert!(is_us_holiday(civil.year, civil.month, civil.day), "{day}");
        }
        for day in ["2026-04-06", "2026-05-26", "2026-11-27", "2021-12-23"] {
            let civil = parse_compact(&format!("{}000000", day.replace('-', ""))).unwrap();
            assert!(!is_us_holiday(civil.year, civil.month, civil.day), "{day}");
        }
        let night = parse_quotes(&payload("3930.12", "20260904103000", "night"), "night").unwrap();
        assert!(night.iter().all(|item| item.quoted_at.ends_with("-04:00")));
        let winter = parse_quotes(&payload("3930.12", "20261204103000", "night"), "night").unwrap();
        assert!(winter.iter().all(|item| item.quoted_at.ends_with("-05:00")));
        assert_eq!(night[4].symbol, "usSOXX");
        for (hour, group) in [
            (7, "night"),
            (8, "day"),
            (19, "day"),
            (20, "night"),
            (0, "night"),
        ] {
            let text = format!("2026-09-04T{hour:02}:00:00");
            assert_eq!(default_group(cn_unix(&text)), group, "{hour}");
        }
    }

    #[test]
    fn intraday_keeps_exchange_session_points() {
        for (symbol, duration, afternoon, count) in [
            ("sh000001", 240, 120, 2),
            ("hkHSI", 330, 150, 2),
            ("usSOXX", 390, 210, 3),
        ] {
            let data = parse_intraday(&json!({"data": {minute_code(symbol): {"data": {"date": "20260904", "data": [
                "1300 105.25 99999 12345", "0930 100 88888", "0931 NaN 1", "0932 -1 2", "0933 inf 1",
                "bad", "2500 102 1", "0800 103 1", "1831 107 1", "0930 101 88888", "1230 104 1"
            ]}}}}), symbol).unwrap();
            assert_eq!(data.date, "2026-09-04");
            assert_eq!(data.duration, duration);
            assert_eq!(
                (
                    data.points[0].time.as_str(),
                    data.points[0].minute,
                    data.points[0].price
                ),
                ("09:30", 0, 101.0)
            );
            let last = data.points.last().unwrap();
            assert_eq!(
                (last.time.as_str(), last.minute, last.price),
                ("13:00", afternoon, 105.25)
            );
            assert_eq!(data.points.len(), count);
        }
        assert!(parse_intraday(
            &json!({"data": {"hkHSI": {"day": [["2026-09-04", "100"]]}}}),
            "hkHSI"
        )
        .is_none());
        assert!(parse_intraday(
            &json!({"data": {"hkHSI": {"data": {"date": "bad", "data": ["0930 100"]}}}}),
            "hkHSI"
        )
        .is_none());
    }
}
