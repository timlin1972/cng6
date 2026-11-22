//! Weather plugin: 多個城市的天氣、逐小時與 7 天預報、天氣警示。
//!
//! 資料來自 Open-Meteo(免費, 不需要 API key), 在背景 thread 定時更新;
//! 網頁伺服器透過 [`Shared`] 讀取。地點存在設定 `weather.locations`(名稱@緯度,經度;…)。

pub mod api;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::Serialize;

use crate::plugin::{Action, Ctx, Plugin, complete_from_help, parse_usize};
use crate::webui::{Button, Card, Status, WebMeta};
use api::{Forecast, Location};

/// 預設沒有城市, 用 weather add 加入
const DEFAULT_LOCATIONS: &str = "";
const DEFAULT_INTERVAL_MIN: u64 = 15;

#[derive(Debug, Clone, Serialize)]
pub struct City {
    #[serde(flatten)]
    pub loc: Location,
    pub forecast: Option<Forecast>,
    /// 最後一次更新的時間(本地時間 HH:MM)
    pub updated: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    cities: RwLock<Vec<City>>,
    version: AtomicU64,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone, Default)]
pub struct Shared(Arc<Inner>);

impl Shared {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cities(&self) -> Vec<City> {
        self.0.cities.read().unwrap().clone()
    }

    pub fn version(&self) -> u64 {
        self.0.version.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.0.version.fetch_add(1, Ordering::Relaxed);
    }

    /// 換成新的地點清單(保留同名地點已經抓到的資料)
    fn set_locations(&self, locs: Vec<Location>) {
        let mut cities = self.0.cities.write().unwrap();
        let old = std::mem::take(&mut *cities);
        *cities = locs
            .into_iter()
            .map(|loc| match old.iter().find(|c| c.loc == loc) {
                Some(c) => c.clone(),
                None => City {
                    loc,
                    forecast: None,
                    updated: None,
                    error: None,
                },
            })
            .collect();
        drop(cities);
        self.bump();
    }

    /// 抓所有地點的天氣(會連網, 在背景 thread 執行)
    fn refresh_all(&self) {
        let locs: Vec<Location> = self.cities().into_iter().map(|c| c.loc).collect();
        for loc in locs {
            let result = api::fetch(&loc);
            let mut cities = self.0.cities.write().unwrap();
            if let Some(c) = cities.iter_mut().find(|c| c.loc == loc) {
                match result {
                    Ok(f) => {
                        c.forecast = Some(f);
                        c.updated = Some(chrono::Local::now().format("%H:%M").to_string());
                        c.error = None;
                    }
                    // 抓不到時保留上一次的資料
                    Err(e) => c.error = Some(e),
                }
            }
            drop(cities);
            self.bump();
        }
    }
}

enum Job {
    Refresh,
    Interval(Duration),
}

type Found = Result<api::Found, String>;

pub struct Weather {
    shared: Shared,
    jobs: Sender<Job>,
    /// 背景搜尋地點的結果
    found_tx: Sender<Found>,
    found_rx: Receiver<Found>,
    locations: Vec<Location>,
    /// 已經通知過的警示(城市 + 標題 + 等級), 只通知新的
    notified: HashSet<String>,
    seen_version: u64,
}

impl Weather {
    pub fn new(shared: Shared) -> Self {
        let (jobs, rx) = mpsc::channel();
        let worker = shared.clone();
        std::thread::spawn(move || {
            let mut interval = Duration::from_secs(DEFAULT_INTERVAL_MIN * 60);
            // 第一次等設定讀完再抓
            let mut wait = Duration::from_secs(2);
            loop {
                match rx.recv_timeout(wait) {
                    Ok(Job::Interval(d)) => {
                        interval = d;
                        wait = interval;
                        continue;
                    }
                    Ok(Job::Refresh) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                // 合併短時間內的多個要求
                while let Ok(job) = rx.try_recv() {
                    if let Job::Interval(d) = job {
                        interval = d;
                    }
                }
                worker.refresh_all();
                wait = interval;
            }
        });
        let (found_tx, found_rx) = mpsc::channel();
        Self {
            shared,
            jobs,
            found_tx,
            found_rx,
            locations: Vec::new(),
            notified: HashSet::new(),
            seen_version: 0,
        }
    }

    fn save_locations(&self, list: &[Location], ctx: &mut Ctx) {
        ctx.push(Action::SetSetting(
            "weather.locations".into(),
            api::locations_setting(list),
        ));
    }

    /// 已經有很接近的地點(約 1 公里內)
    fn near(&self, loc: &Location) -> Option<&Location> {
        self.locations
            .iter()
            .find(|l| (l.lat - loc.lat).abs() < 0.01 && (l.lon - loc.lon).abs() < 0.01)
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.locations.iter().position(|l| l.name == name)
    }

    fn city_names(&self) -> Vec<(String, String)> {
        self.locations
            .iter()
            .map(|l| (l.name.clone(), format!("{:.3}, {:.3}", l.lat, l.lon)))
            .collect()
    }
}

fn temp(v: f64) -> String {
    if v.is_nan() {
        "–".into()
    } else {
        format!("{v:.0}°")
    }
}

impl Plugin for Weather {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "天氣: 多城市、逐小時與 7 天預報、天氣警示"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("now", "所有城市目前的天氣"),
            ("forecast [city]", "7 天預報(預設第一個城市)"),
            ("alerts", "天氣警示"),
            ("add <city>", "加入城市(台灣縣市、鄉鎮區或英文地名)"),
            ("add <name> <lat> <lon>", "用經緯度加入地點"),
            ("remove <city>", "移除城市"),
            ("first <city>", "設為第一個(總覽顯示的主要城市)"),
            ("refresh", "馬上更新天氣"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["forecast" | "remove" | "first"] => self.city_names(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            (
                "locations",
                "城市, 格式 名稱@緯度,經度, 多個用 ; 分隔 (建議用 weather add 加入)",
                DEFAULT_LOCATIONS.into(),
            ),
            (
                "interval",
                "自動更新間隔(分鐘), 5~180",
                DEFAULT_INTERVAL_MIN.to_string(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "locations" => {
                let list = api::parse_locations(value)?;
                self.locations = list.clone();
                self.shared.set_locations(list);
                let _ = self.jobs.send(Job::Refresh);
            }
            "interval" => {
                let min = parse_usize(value, 5, 180)?;
                let _ = self
                    .jobs
                    .send(Job::Interval(Duration::from_secs(min as u64 * 60)));
            }
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let cities = self.shared.cities();
        match args {
            [] | ["now"] | ["forecast", ..] | ["alerts"] if cities.is_empty() => {
                ctx.info("還沒有城市, 用 weather add <城市> 加入, 例如 weather add 台北");
            }
            [] | ["now"] => {
                for c in &cities {
                    match &c.forecast {
                        Some(f) => {
                            let today = f.daily.first();
                            ctx.info(format!(
                                "  {}  {} {}  體感 {}  今天 {}/{}  降雨 {:.0}%{}",
                                c.loc.name,
                                temp(f.current.temp),
                                api::describe(f.current.code),
                                temp(f.current.feels),
                                today.map_or("–".into(), |d| temp(d.max)),
                                today.map_or("–".into(), |d| temp(d.min)),
                                today.map_or(0.0, |d| d.prob),
                                c.updated
                                    .as_ref()
                                    .map_or(String::new(), |u| format!("  ({u} 更新)")),
                            ));
                        }
                        None => ctx.info(format!(
                            "  {}  {}",
                            c.loc.name,
                            c.error.as_deref().unwrap_or("讀取中…")
                        )),
                    }
                    for a in c.forecast.iter().flat_map(|f| &f.alerts) {
                        ctx.warn(format!("    ▲ {}: {}", a.title, a.detail));
                    }
                }
            }
            ["forecast", rest @ ..] => {
                let name = rest.join(" ");
                let city = if name.is_empty() {
                    cities.first()
                } else {
                    cities.iter().find(|c| c.loc.name == name)
                };
                let Some(c) = city else {
                    ctx.error(format!("沒有城市 '{name}', 用 weather add 加入"));
                    return;
                };
                let Some(f) = &c.forecast else {
                    ctx.info(format!(
                        "{}: {}",
                        c.loc.name,
                        c.error.as_deref().unwrap_or("讀取中…")
                    ));
                    return;
                };
                ctx.info(format!("{} 7 天預報", c.loc.name));
                for d in &f.daily {
                    ctx.info(format!(
                        "  {}  {:<6}  {:>4} / {:<4}  降雨 {:>3.0}%  {:.1} mm",
                        d.date.get(5..).unwrap_or(&d.date),
                        api::describe(d.code),
                        temp(d.min),
                        temp(d.max),
                        d.prob,
                        d.precip
                    ));
                }
            }
            ["alerts"] => {
                let mut any = false;
                for c in &cities {
                    for a in c.forecast.iter().flat_map(|f| &f.alerts) {
                        any = true;
                        ctx.warn(format!("  {} {}: {}", c.loc.name, a.title, a.detail));
                    }
                }
                if !any {
                    ctx.info("  目前沒有天氣警示");
                }
            }
            ["add", name, lat, lon] if lat.parse::<f64>().is_ok() && lon.parse::<f64>().is_ok() => {
                match Location::parse(&format!("{name}@{lat},{lon}")) {
                    Ok(loc) => {
                        if self.find(&loc.name).is_some() {
                            ctx.error(format!("已經有 '{}'", loc.name));
                            return;
                        }
                        let mut list = self.locations.clone();
                        list.push(loc);
                        self.save_locations(&list, ctx);
                    }
                    Err(e) => ctx.error(e),
                }
            }
            ["add", rest @ ..] if !rest.is_empty() => {
                let query = rest.join(" ");
                ctx.info(format!("搜尋「{query}」…"));
                // 搜尋會連網, 在背景做, 結果在 on_tick 處理
                let tx = self.found_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(api::find_location(&query));
                });
            }
            ["remove", rest @ ..] if !rest.is_empty() => {
                let name = rest.join(" ");
                match self.find(&name) {
                    None => ctx.error(format!("沒有城市 '{name}'")),
                    Some(i) => {
                        let mut list = self.locations.clone();
                        list.remove(i);
                        self.save_locations(&list, ctx);
                    }
                }
            }
            ["first", rest @ ..] if !rest.is_empty() => {
                let name = rest.join(" ");
                match self.find(&name) {
                    None => ctx.error(format!("沒有城市 '{name}'")),
                    Some(i) => {
                        let mut list = self.locations.clone();
                        let loc = list.remove(i);
                        list.insert(0, loc);
                        self.save_locations(&list, ctx);
                    }
                }
            }
            ["refresh"] => {
                let _ = self.jobs.send(Job::Refresh);
                ctx.info("更新天氣中…");
            }
            _ => ctx.error("用法: 輸入 'help weather' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        // 背景搜尋地點的結果
        while let Ok(res) = self.found_rx.try_recv() {
            match res {
                Ok(api::Found {
                    mut loc,
                    place,
                    full_name,
                }) => {
                    if let Some(l) = self.near(&loc) {
                        ctx.warn(format!("已經有 '{}' (同一個位置)", l.name));
                        continue;
                    }
                    // 名稱重複(例如台北與基隆都有中山區)時改用完整名稱
                    if self.find(&loc.name).is_some()
                        && let Some(full) = full_name
                        && self.find(&full).is_none()
                    {
                        loc.name = full;
                    }
                    if self.find(&loc.name).is_some() {
                        ctx.warn(format!("已經有 '{}'", loc.name));
                        continue;
                    }
                    ctx.info(format!(
                        "找到 {} ({place}) {:.3}, {:.3}",
                        loc.name, loc.lat, loc.lon
                    ));
                    let mut list = self.locations.clone();
                    list.push(loc);
                    self.save_locations(&list, ctx);
                }
                Err(e) => ctx.error(e),
            }
        }
        // 新出現的天氣警示: 在 Message 通知一次
        let v = self.shared.version();
        if v != self.seen_version {
            self.seen_version = v;
            let mut current = HashSet::new();
            for c in self.shared.cities() {
                for a in c.forecast.iter().flat_map(|f| &f.alerts) {
                    let key = format!("{}|{}|{:?}", c.loc.name, a.title, a.status);
                    if !self.notified.contains(&key) {
                        ctx.warn(format!(
                            "weather: {} {}警示 — {}",
                            c.loc.name, a.title, a.detail
                        ));
                    }
                    current.insert(key);
                }
            }
            // 解除的警示下次再出現時會再通知
            self.notified = current;
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("天氣", "weather", "blue"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("weather")
    }

    fn web_card(&self) -> Option<Card> {
        let cities = self.shared.cities();
        let Some(first) = cities.first() else {
            return Some(Card::new().summary("還沒有設定城市").text(
                "用 weather add <城市> 加入, 或在天氣畫面按「＋ 城市」",
                None,
            ));
        };
        let summary = match &first.forecast {
            Some(f) => {
                let today = f.daily.first();
                format!(
                    "{} {} {} · 高 {} 低 {}",
                    first.loc.name,
                    temp(f.current.temp),
                    api::describe(f.current.code),
                    today.map_or("–".into(), |d| temp(d.max)),
                    today.map_or("–".into(), |d| temp(d.min)),
                )
            }
            None => format!(
                "{} · {}",
                first.loc.name,
                first.error.as_deref().unwrap_or("讀取中…")
            ),
        };
        let mut card = Card::new().summary(summary);
        for c in &cities {
            card = match &c.forecast {
                Some(f) => {
                    let today = f.daily.first();
                    card.stat(
                        c.loc.name.clone(),
                        format!("{:.0}", f.current.temp),
                        Some("°C"),
                        None,
                    )
                    .hint(format!(
                        "{} · {}/{} · 降雨 {:.0}%",
                        api::describe(f.current.code),
                        today.map_or("–".into(), |d| temp(d.max)),
                        today.map_or("–".into(), |d| temp(d.min)),
                        today.map_or(0.0, |d| d.prob)
                    ))
                }
                None => card
                    .stat(c.loc.name.clone(), "–", None, Some(Status::Off))
                    .hint(c.error.clone().unwrap_or_else(|| "讀取中…".into())),
            };
        }
        let mut worst = None;
        for c in &cities {
            for a in c.forecast.iter().flat_map(|f| &f.alerts) {
                worst = worst.max(Some(a.status));
                card = card.text(
                    format!("{} {}: {}", c.loc.name, a.title, a.detail),
                    Some(a.status),
                );
            }
        }
        if let Some(st) = worst {
            card = card.status(st);
        }
        if let Some(u) = &first.updated {
            card = card.text(format!("{u} 更新 · 資料來源 Open-Meteo"), None);
        }
        Some(card.buttons(vec![Button::new("立即更新", "/weather refresh")]))
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}
