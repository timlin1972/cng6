//! World clock plugin: 多個城市的時間、和本地的時差、時間換算。
//!
//! 城市存在設定 `worldclock.zones`(名稱@時區;…, 預設空的)。加入城市用和天氣相同的
//! 地點搜尋, 再用 Open-Meteo 依經緯度查出時區(台灣的地點直接用 Asia/Taipei)。
//! 時差與日光節約時間由 chrono-tz 計算; 網頁上的時鐘由瀏覽器自己走。

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use ratatui::Frame;
use ratatui::layout::Rect;
use serde::Serialize;

use crate::plugin::{Action, Ctx, Plugin, complete_from_help};
use crate::plugins::weather::api as geo;
use crate::webui::{Card, WebMeta};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Zone {
    pub name: String,
    /// IANA 時區, 例如 Asia/Tokyo
    pub tz: String,
}

impl Zone {
    fn parse(s: &str) -> Result<Self, String> {
        let (name, tz) = s
            .rsplit_once('@')
            .ok_or_else(|| format!("'{s}' 格式應為 名稱@時區"))?;
        let name = name.trim();
        let tz = tz.trim();
        if name.is_empty() || name.contains([';', '@']) {
            return Err(format!("'{s}' 名稱不合法"));
        }
        tz.parse::<Tz>()
            .map_err(|_| format!("'{tz}' 不是時區名稱"))?;
        Ok(Self {
            name: name.to_string(),
            tz: tz.to_string(),
        })
    }

    fn tz(&self) -> Tz {
        self.tz.parse().unwrap_or(Tz::UTC)
    }
}

fn parse_zones(s: &str) -> Result<Vec<Zone>, String> {
    s.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(Zone::parse)
        .collect()
}

fn zones_setting(list: &[Zone]) -> String {
    list.iter()
        .map(|z| format!("{}@{}", z.name, z.tz))
        .collect::<Vec<_>>()
        .join(";")
}

/// 這台機器的時區(/etc/timezone), 讀不到時用 UTC
pub fn local_tz() -> Tz {
    std::fs::read_to_string("/etc/timezone")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(Tz::UTC)
}

/// 依經緯度查時區: 台灣直接用 Asia/Taipei, 其他用 Open-Meteo
fn timezone_at(lat: f64, lon: f64) -> Result<String, String> {
    if geo::is_taiwan(lat, lon) {
        return Ok("Asia/Taipei".into());
    }
    #[derive(serde::Deserialize)]
    struct R {
        timezone: String,
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent();
    let r: R = agent
        .get("https://api.open-meteo.com/v1/forecast")
        .query("latitude", lat.to_string())
        .query("longitude", lon.to_string())
        .query("timezone", "auto")
        .query("forecast_days", "1")
        .call()
        .map_err(|e| format!("查詢時區失敗: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("查詢時區失敗: {e}"))?;
    Ok(r.timezone)
}

/// 找城市的時區: 可以直接寫時區名稱(Europe/London), 或城市名稱
fn find_zone(query: &str) -> Result<(Zone, String), String> {
    if query.contains('/')
        && let Ok(tz) = query.parse::<Tz>()
    {
        let name = query.rsplit('/').next().unwrap_or(query).replace('_', " ");
        return Ok((
            Zone {
                name,
                tz: tz.name().to_string(),
            },
            "時區名稱".into(),
        ));
    }
    let found = geo::find_location(query)?;
    let tz = timezone_at(found.loc.lat, found.loc.lon)?;
    Ok((
        Zone {
            name: found.loc.name,
            tz,
        },
        found.place,
    ))
}

/// 和本地的時差(分鐘)
fn diff_minutes(now: DateTime<Utc>, tz: Tz, local: Tz) -> i32 {
    let a = tz
        .offset_from_utc_datetime(&now.naive_utc())
        .fix()
        .local_minus_utc();
    let b = local
        .offset_from_utc_datetime(&now.naive_utc())
        .fix()
        .local_minus_utc();
    (a - b) / 60
}

fn fmt_diff(min: i32) -> String {
    if min == 0 {
        return "同本地".into();
    }
    let sign = if min > 0 { "+" } else { "-" };
    let m = min.abs();
    if m % 60 == 0 {
        format!("{sign}{} 小時", m / 60)
    } else {
        format!("{sign}{}:{:02}", m / 60, m % 60)
    }
}

const WEEK: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];

/// 「明天」「昨天」或空字串(和本地日期比較)
fn day_word(t: &DateTime<Tz>, local: &DateTime<Tz>) -> &'static str {
    match t
        .date_naive()
        .signed_duration_since(local.date_naive())
        .num_days()
    {
        1 => "明天",
        -1 => "昨天",
        _ => "",
    }
}

/// 一行文字: 時間、日期、時差
fn line(name: &str, tz: Tz, now: DateTime<Utc>, local: Tz) -> String {
    let t = now.with_timezone(&tz);
    let l = now.with_timezone(&local);
    let day = day_word(&t, &l);
    format!(
        "{name}  {}  {}/{} (週{})  {}{}",
        t.format("%H:%M"),
        t.month(),
        t.day(),
        WEEK[t.weekday().num_days_from_monday() as usize],
        fmt_diff(diff_minutes(now, tz, local)),
        if day.is_empty() {
            String::new()
        } else {
            format!("  {day}")
        }
    )
}

/// 和網頁伺服器共用的狀態
#[derive(Clone, Default)]
pub struct Shared(Arc<RwLock<Vec<Zone>>>);

impl Shared {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn zones(&self) -> Vec<Zone> {
        self.0.read().unwrap().clone()
    }
}

type Found = Result<(Zone, String), String>;

pub struct WorldClock {
    shared: Shared,
    zones: Vec<Zone>,
    found_tx: Sender<Found>,
    found_rx: Receiver<Found>,
}

impl WorldClock {
    pub fn new(shared: Shared) -> Self {
        let (found_tx, found_rx) = mpsc::channel();
        Self {
            shared,
            zones: Vec::new(),
            found_tx,
            found_rx,
        }
    }

    fn save(&self, list: &[Zone], ctx: &mut Ctx) {
        ctx.push(Action::SetSetting(
            "worldclock.zones".into(),
            zones_setting(list),
        ));
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.zones.iter().position(|z| z.name == name)
    }
}

impl Plugin for WorldClock {
    fn name(&self) -> &str {
        "worldclock"
    }

    fn description(&self) -> &str {
        "世界時間: 多個城市的時間與時差"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("now", "各城市現在的時間"),
            (
                "at <HH:MM> [city]",
                "換算: 本地(或某城市) HH:MM 時, 各城市是幾點",
            ),
            (
                "add <city>",
                "加入城市(台灣: xx市xx區, 國外: 英文), 或時區名稱如 Europe/London",
            ),
            ("remove <city>", "移除城市"),
            ("first <city>", "移到最前面"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["remove" | "first"] | ["at", _] => self
                .zones
                .iter()
                .map(|z| (z.name.clone(), z.tz.clone()))
                .collect(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![(
            "zones",
            "城市, 格式 名稱@時區, 多個用 ; 分隔 (建議用 worldclock add 加入)",
            String::new(),
        )]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "zones" => {
                let list = parse_zones(value)?;
                self.zones = list.clone();
                *self.shared.0.write().unwrap() = list;
                Ok(())
            }
            _ => Err(format!("不支援的設定 '{key}'")),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let local = local_tz();
        match args {
            [] | ["now"] => {
                let now = Utc::now();
                ctx.info(format!(
                    "  {}",
                    line(&format!("本地 ({})", local.name()), local, now, local)
                ));
                for z in &self.zones {
                    ctx.info(format!("  {}", line(&z.name, z.tz(), now, local)));
                }
                if self.zones.is_empty() {
                    ctx.info(
                        "  還沒有城市, 用 worldclock add <城市> 加入, 例如 worldclock add Tokyo",
                    );
                }
            }
            ["at", time, rest @ ..] => {
                let Ok(t) = NaiveTime::parse_from_str(time, "%H:%M") else {
                    ctx.error("時間格式應為 HH:MM, 例如 worldclock at 15:00");
                    return;
                };
                // 以本地或指定城市的「今天」那個時間為基準
                let base = if rest.is_empty() {
                    None
                } else {
                    let name = rest.join(" ");
                    match self.zones.iter().find(|z| z.name == name) {
                        Some(z) => Some(z.clone()),
                        None => {
                            ctx.error(format!("沒有城市 '{name}'"));
                            return;
                        }
                    }
                };
                let base_tz = base.as_ref().map_or(local, Zone::tz);
                let today = Utc::now().with_timezone(&base_tz).date_naive();
                let Some(at) = base_tz.from_local_datetime(&today.and_time(t)).earliest() else {
                    ctx.error("這個時間在當地不存在(日光節約時間切換)");
                    return;
                };
                let now = at.with_timezone(&Utc);
                let base_name = base.as_ref().map_or("本地".to_string(), |z| z.name.clone());
                ctx.info(format!("{base_name} {time} 時:"));
                ctx.info(format!("  {}", line("本地", local, now, local)));
                for z in &self.zones {
                    ctx.info(format!("  {}", line(&z.name, z.tz(), now, local)));
                }
            }
            ["add", rest @ ..] if !rest.is_empty() => {
                let query = rest.join(" ");
                ctx.info(format!("搜尋「{query}」…"));
                let tx = self.found_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(find_zone(&query));
                });
            }
            ["remove", rest @ ..] if !rest.is_empty() => {
                let name = rest.join(" ");
                match self.find(&name) {
                    None => ctx.error(format!("沒有城市 '{name}'")),
                    Some(i) => {
                        let mut list = self.zones.clone();
                        list.remove(i);
                        self.save(&list, ctx);
                    }
                }
            }
            ["first", rest @ ..] if !rest.is_empty() => {
                let name = rest.join(" ");
                match self.find(&name) {
                    None => ctx.error(format!("沒有城市 '{name}'")),
                    Some(i) => {
                        let mut list = self.zones.clone();
                        let z = list.remove(i);
                        list.insert(0, z);
                        self.save(&list, ctx);
                    }
                }
            }
            _ => ctx.error("用法: 輸入 'help worldclock' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        while let Ok(res) = self.found_rx.try_recv() {
            match res {
                Ok((zone, place)) => {
                    if self.find(&zone.name).is_some() {
                        ctx.warn(format!("已經有 '{}'", zone.name));
                        continue;
                    }
                    ctx.info(format!("找到 {} ({place}) 時區 {}", zone.name, zone.tz));
                    let mut list = self.zones.clone();
                    list.push(zone);
                    self.save(&list, ctx);
                }
                Err(e) => ctx.error(e),
            }
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("世界時間", "clock", "indigo"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("worldclock")
    }

    fn web_card(&self) -> Option<Card> {
        let local = local_tz();
        let now = Utc::now();
        let l = now.with_timezone(&local);
        if self.zones.is_empty() {
            return Some(Card::new().summary("還沒有設定城市").text(
                "用 worldclock add <城市> 加入, 或在世界時間畫面按「＋ 城市」",
                None,
            ));
        }
        let summary = self
            .zones
            .iter()
            .take(3)
            .map(|z| format!("{} {}", z.name, now.with_timezone(&z.tz()).format("%H:%M")))
            .collect::<Vec<_>>()
            .join(" · ");
        let mut card = Card::new()
            .summary(summary)
            .stat("本地", l.format("%H:%M").to_string(), None, None)
            .hint(format!(
                "{}/{} (週{})",
                l.month(),
                l.day(),
                WEEK[l.weekday().num_days_from_monday() as usize]
            ));
        for z in &self.zones {
            let t = now.with_timezone(&z.tz());
            let day = day_word(&t, &l);
            card = card
                .stat(z.name.clone(), t.format("%H:%M").to_string(), None, None)
                .hint(format!(
                    "{}{}",
                    fmt_diff(diff_minutes(now, z.tz(), local)),
                    if day.is_empty() {
                        String::new()
                    } else {
                        format!(" · {day}")
                    }
                ));
        }
        Some(card)
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_roundtrip() {
        let z = parse_zones("Tokyo@Asia/Tokyo; Eindhoven@Europe/Amsterdam").unwrap();
        assert_eq!(z.len(), 2);
        assert_eq!(
            zones_setting(&z),
            "Tokyo@Asia/Tokyo;Eindhoven@Europe/Amsterdam"
        );
        assert!(parse_zones("").unwrap().is_empty());
        assert!(parse_zones("X@Not/AZone").is_err());
    }

    #[test]
    fn differences() {
        // 2026-01-15 冬天: 台北 +8, 阿姆斯特丹 +1 → -7 小時; 夏天(7 月)日光節約 → -6 小時
        let winter = Utc.with_ymd_and_hms(2026, 1, 15, 4, 0, 0).unwrap();
        let summer = Utc.with_ymd_and_hms(2026, 7, 15, 4, 0, 0).unwrap();
        let tpe: Tz = "Asia/Taipei".parse().unwrap();
        let ams: Tz = "Europe/Amsterdam".parse().unwrap();
        let ind: Tz = "Asia/Kolkata".parse().unwrap();
        assert_eq!(fmt_diff(diff_minutes(winter, ams, tpe)), "-7 小時");
        assert_eq!(fmt_diff(diff_minutes(summer, ams, tpe)), "-6 小時");
        assert_eq!(fmt_diff(diff_minutes(winter, ind, tpe)), "-2:30");
        assert_eq!(fmt_diff(diff_minutes(winter, tpe, tpe)), "同本地");
    }

    #[test]
    fn tz_name_query() {
        let (z, _) = find_zone("Europe/London").unwrap();
        assert_eq!(z.name, "London");
        assert_eq!(z.tz, "Europe/London");
        let (z, _) = find_zone("America/New_York").unwrap();
        assert_eq!(z.name, "New York");
    }
}
