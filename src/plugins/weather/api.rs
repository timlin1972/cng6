//! Open-Meteo: 天氣預報與地點搜尋(免費, 不需要 API key)
//!
//! - 預報: https://api.open-meteo.com/v1/forecast
//! - 地點: https://geocoding-api.open-meteo.com/v1/search
//!
//! 天氣警示依預報數值判斷(參考中央氣象署的門檻), 不是氣象署發布的正式警特報。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::webui::Status;

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Location {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

impl Location {
    /// 設定檔格式: 名稱@緯度,經度
    pub fn parse(s: &str) -> Result<Self, String> {
        let (name, pos) = s
            .rsplit_once('@')
            .ok_or_else(|| format!("'{s}' 格式應為 名稱@緯度,經度"))?;
        let (lat, lon) = pos
            .split_once(',')
            .ok_or_else(|| format!("'{s}' 格式應為 名稱@緯度,經度"))?;
        let lat: f64 = lat
            .trim()
            .parse()
            .map_err(|_| format!("'{lat}' 不是緯度"))?;
        let lon: f64 = lon
            .trim()
            .parse()
            .map_err(|_| format!("'{lon}' 不是經度"))?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(format!("'{s}' 經緯度超出範圍"));
        }
        let name = name.trim();
        if name.is_empty() || name.contains([';', '@']) {
            return Err(format!("'{s}' 名稱不合法"));
        }
        Ok(Self {
            name: name.to_string(),
            lat,
            lon,
        })
    }

    pub fn to_setting(&self) -> String {
        format!("{}@{:.4},{:.4}", self.name, self.lat, self.lon)
    }
}

/// 設定值: 多個地點以 ; 分隔, 空字串表示沒有地點
pub fn parse_locations(s: &str) -> Result<Vec<Location>, String> {
    let list: Vec<Location> = s
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(Location::parse)
        .collect::<Result<_, _>>()?;
    Ok(list)
}

pub fn locations_setting(list: &[Location]) -> String {
    list.iter()
        .map(Location::to_setting)
        .collect::<Vec<_>>()
        .join(";")
}

// ---------------------------------------------------------------- 地點

/// 台灣 22 縣市(線上搜尋中文名稱不穩定, 先查這張表)
const TAIWAN: &[(&str, f64, f64)] = &[
    ("臺北市", 25.0375, 121.5637),
    ("新北市", 25.0120, 121.4650),
    ("桃園市", 24.9937, 121.3010),
    ("臺中市", 24.1477, 120.6736),
    ("臺南市", 22.9997, 120.2270),
    ("高雄市", 22.6273, 120.3014),
    ("基隆市", 25.1276, 121.7392),
    ("新竹市", 24.8138, 120.9675),
    ("新竹縣", 24.8387, 121.0177),
    ("苗栗縣", 24.5602, 120.8214),
    ("彰化縣", 24.0518, 120.5161),
    ("南投縣", 23.9609, 120.9719),
    ("雲林縣", 23.7092, 120.4313),
    ("嘉義市", 23.4801, 120.4491),
    ("嘉義縣", 23.4518, 120.2555),
    ("屏東縣", 22.5519, 120.5487),
    ("宜蘭縣", 24.7021, 121.7378),
    ("花蓮縣", 23.9871, 121.6015),
    ("臺東縣", 22.7583, 121.1444),
    ("澎湖縣", 23.5711, 119.5793),
    ("金門縣", 24.4493, 118.3767),
    ("連江縣", 26.1602, 119.9517),
];

fn builtin(query: &str) -> Option<Location> {
    let norm = |s: &str| s.replace('台', "臺");
    let q = norm(query.trim());
    let (name, lat, lon) = TAIWAN.iter().find(|(n, ..)| {
        *n == q || n.trim_end_matches(['市', '縣']) == q.trim_end_matches(['市', '縣'])
    })?;
    Some(Location {
        name: name.to_string(),
        lat: *lat,
        lon: *lon,
    })
}

#[derive(Deserialize)]
struct GeoResult {
    id: Option<u64>,
    name: String,
    latitude: f64,
    longitude: f64,
    country_code: Option<String>,
    admin2: Option<String>,
    admin1: Option<String>,
}

#[derive(Deserialize)]
struct GeoResponse {
    #[serde(default)]
    results: Vec<GeoResult>,
}

/// 依地點編號取得指定語言的名稱(國外城市用英文顯示)
fn geocode_by_id(id: u64, lang: &str) -> Result<GeoResult, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    agent
        .get("https://geocoding-api.open-meteo.com/v1/get")
        .query("id", id.to_string())
        .query("language", lang)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())
}

fn geocode_once(query: &str, lang: &str) -> Result<Vec<GeoResult>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    let res: GeoResponse = agent
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query("name", query)
        .query("count", "10")
        .query("language", lang)
        .call()
        .map_err(|e| format!("搜尋地點失敗: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("搜尋地點失敗: {e}"))?;
    Ok(res.results)
}

/// 找到的地點
pub struct Found {
    pub loc: Location,
    /// 所在位置的說明(顯示用), 例如「臺北市, 臺灣」
    pub place: String,
    /// 名稱重複時改用的完整名稱, 例如「基隆市中山區」
    pub full_name: Option<String>,
}

#[derive(Deserialize)]
struct NomAddress {
    city: Option<String>,
    county: Option<String>,
}

#[derive(Deserialize)]
struct NomResult {
    name: String,
    lat: String,
    lon: String,
    addresstype: Option<String>,
    display_name: String,
    address: Option<NomAddress>,
}

/// OpenStreetMap Nominatim, 只搜台灣: 可以處理「台北市中山區」這種寫法
fn nominatim_tw(query: &str, exact: bool) -> Result<Option<Found>, String> {
    nominatim(query, exact, Some("tw"), "zh-TW")
}

/// OpenStreetMap Nominatim
/// - 使用規定: 每秒最多一次、要帶程式名稱; 這裡只有加入城市時才會呼叫
/// - exact: 只接受名稱完全相同的行政區(用來試「新店區」這種補上後綴的名稱)
/// - country: 限定國家; lang: 回傳名稱的語言
fn nominatim(
    query: &str,
    exact: bool,
    country: Option<&str>,
    lang: &str,
) -> Result<Option<Found>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    let mut req = agent
        .get("https://nominatim.openstreetmap.org/search")
        .header("User-Agent", "cng6-weather/0.1")
        .query("q", query)
        .query("format", "jsonv2")
        .query("addressdetails", "1")
        .query("limit", "5")
        .query("accept-language", lang);
    if let Some(c) = country {
        req = req.query("countrycodes", c);
    }
    let results: Vec<NomResult> = req
        .call()
        .map_err(|e| format!("搜尋地點失敗: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("搜尋地點失敗: {e}"))?;
    // 優先選行政區(區、鄉、鎮、市), 不要選到車站、店家
    const AREA: &[&str] = &[
        "city",
        "town",
        "suburb",
        "village",
        "county",
        "municipality",
        "city_district",
        "district",
        "borough",
        "quarter",
        "state",
    ];
    let is_area = |r: &&NomResult| r.addresstype.as_deref().is_some_and(|t| AREA.contains(&t));
    // 只接受行政區: 店家、車站等地點(例如名叫「韓首爾」的餐廳)不算,
    // 找不到時交給 Open-Meteo 搜尋全球
    let pick = results
        .iter()
        .find(|r| is_area(r) && (!exact || r.name == query));
    let Some(r) = pick else { return Ok(None) };
    let (Ok(lat), Ok(lon)) = (r.lat.parse::<f64>(), r.lon.parse::<f64>()) else {
        return Ok(None);
    };
    let city = r
        .address
        .as_ref()
        .and_then(|a| a.city.clone().or_else(|| a.county.clone()));
    let full_name = city
        .filter(|c| *c != r.name)
        .map(|c| format!("{c}{}", r.name));
    // display_name 很長, 只留後面幾段(行政區、縣市、國家)
    let parts: Vec<&str> = r.display_name.split(", ").collect();
    let place = parts[parts.len().saturating_sub(3)..].join(", ");
    Ok(Some(Found {
        loc: Location {
            name: r.name.clone(),
            lat,
            lon,
        },
        place,
        full_name,
    }))
}

/// 找地點: 內建縣市 → Nominatim(台灣, 可以寫「台北市中山區」) → Open-Meteo(全球)
pub fn find_location(query: &str) -> Result<Found, String> {
    if let Some(loc) = builtin(query) {
        return Ok(Found {
            loc,
            place: "內建縣市".into(),
            full_name: None,
        });
    }
    let cjk = query
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    if cjk {
        // 沒有寫行政區後綴時, 先試「區、市、鎮、鄉」: 「新店」要找新北市新店區,
        // 不是宜蘭的新店; Nominatim 規定每秒最多一次, 每次之間等一下
        let has_suffix = query.ends_with(['區', '市', '縣', '鎮', '鄉']);
        if !has_suffix {
            for s in ["區", "市", "鎮", "鄉"] {
                if let Some(found) = nominatim_tw(&format!("{query}{s}"), true)? {
                    return Ok(found);
                }
                std::thread::sleep(Duration::from_millis(1100));
            }
        }
        if let Some(found) = nominatim_tw(query, false)? {
            return Ok(found);
        }
    }
    match find_open_meteo(query) {
        Ok((loc, place)) => Ok(Found {
            loc,
            place,
            full_name: None,
        }),
        // Open-Meteo 沒有收錄的中文地名(例如「首爾」): 用 Nominatim 搜全球, 名稱用英文
        Err(e) if cjk => {
            std::thread::sleep(Duration::from_millis(1100));
            nominatim(query, false, None, "en")?.ok_or(e)
        }
        Err(e) => Err(e),
    }
}

/// Open-Meteo 地點搜尋(優先台灣; 中文名稱先補上「區、市、鎮、鄉」)
fn find_open_meteo(query: &str) -> Result<(Location, String), String> {
    let mut first_other = None;
    // 中文且沒有行政區後綴時, 先試「區、市、鎮、鄉」: 「新店」要找新北市新店區,
    // 不是宜蘭的新店村
    let cjk = query
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    let has_suffix = query.ends_with(['區', '市', '縣', '鎮', '鄉']);
    let mut queries: Vec<String> = Vec::new();
    if cjk && !has_suffix {
        queries.extend(["區", "市", "鎮", "鄉"].map(|s| format!("{query}{s}")));
    }
    queries.push(query.to_string());
    for q in queries {
        let results = geocode_once(&q, "zh")?;
        if let Some(r) = results
            .iter()
            .find(|r| r.country_code.as_deref() == Some("TW"))
        {
            let place = [r.admin1.as_deref(), r.admin2.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            return Ok((
                Location {
                    name: r.name.clone(),
                    lat: r.latitude,
                    lon: r.longitude,
                },
                place,
            ));
        }
        if first_other.is_none() {
            first_other = results.into_iter().next().map(|r| (r, q.clone()));
        }
    }
    match first_other {
        Some((r, q)) => {
            // 國外城市用英文名稱顯示: 用英文再查一次, 依地點編號對應
            let _ = q;
            let r =
                r.id.and_then(|id| geocode_by_id(id, "en").ok())
                    .unwrap_or(r);
            Ok((
                Location {
                    name: r.name.clone(),
                    lat: r.latitude,
                    lon: r.longitude,
                },
                [r.admin1, r.country_code]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
        }
        None => Err(format!(
            "找不到「{query}」, 可以改用英文名稱, 或 weather add <名稱> <緯度> <經度>"
        )),
    }
}

// ---------------------------------------------------------------- 預報

#[derive(Debug, Clone, Serialize)]
pub struct Current {
    pub time: String,
    pub temp: f64,
    pub feels: f64,
    pub code: u8,
    pub is_day: bool,
    pub precip: f64,
    pub wind: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hour {
    pub time: String,
    pub temp: f64,
    /// 降雨機率 %
    pub prob: f64,
    /// 雨量 mm
    pub precip: f64,
    pub code: u8,
    pub is_day: bool,
    /// 陣風 km/h
    pub gust: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Day {
    pub date: String,
    pub code: u8,
    pub max: f64,
    pub min: f64,
    pub prob: f64,
    pub precip: f64,
    pub gust: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub status: Status,
    /// 例如「大雨」
    pub title: String,
    /// 例如「未來 24 小時雨量 85 mm」
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Forecast {
    pub current: Current,
    /// 從目前時刻開始的 48 小時
    pub hourly: Vec<Hour>,
    pub daily: Vec<Day>,
    pub alerts: Vec<Alert>,
}

#[derive(Deserialize)]
struct RawCurrent {
    time: String,
    temperature_2m: f64,
    apparent_temperature: f64,
    weather_code: u8,
    is_day: u8,
    precipitation: f64,
    wind_speed_10m: f64,
}

#[derive(Deserialize)]
struct RawHourly {
    time: Vec<String>,
    temperature_2m: Vec<Option<f64>>,
    precipitation_probability: Vec<Option<f64>>,
    precipitation: Vec<Option<f64>>,
    weather_code: Vec<Option<u8>>,
    is_day: Vec<Option<u8>>,
    wind_gusts_10m: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct RawDaily {
    time: Vec<String>,
    weather_code: Vec<Option<u8>>,
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
    precipitation_probability_max: Vec<Option<f64>>,
    precipitation_sum: Vec<Option<f64>>,
    wind_gusts_10m_max: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct Raw {
    current: RawCurrent,
    hourly: RawHourly,
    daily: RawDaily,
}

pub fn fetch(loc: &Location) -> Result<Forecast, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    let raw: Raw = agent
        .get("https://api.open-meteo.com/v1/forecast")
        .query("latitude", loc.lat.to_string())
        .query("longitude", loc.lon.to_string())
        .query(
            "current",
            "temperature_2m,apparent_temperature,weather_code,is_day,precipitation,wind_speed_10m",
        )
        .query(
            "hourly",
            "temperature_2m,precipitation_probability,precipitation,weather_code,is_day,wind_gusts_10m",
        )
        .query(
            "daily",
            "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,precipitation_sum,wind_gusts_10m_max",
        )
        .query("timezone", "auto")
        .query("forecast_days", "7")
        .call()
        .map_err(|e| format!("取得天氣失敗: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("天氣資料格式錯誤: {e}"))?;

    let c = raw.current;
    let current = Current {
        time: c.time,
        temp: c.temperature_2m,
        feels: c.apparent_temperature,
        code: c.weather_code,
        is_day: c.is_day == 1,
        precip: c.precipitation,
        wind: c.wind_speed_10m,
    };
    // 逐小時: 從目前這個小時開始, 取 48 小時
    let now_hour = current.time.get(..13).unwrap_or("").to_string();
    let h = raw.hourly;
    let start = h
        .time
        .iter()
        .position(|t| t.get(..13).unwrap_or("") >= now_hour.as_str())
        .unwrap_or(0);
    let hourly: Vec<Hour> = (start..h.time.len().min(start + 48))
        .map(|i| Hour {
            time: h.time[i].clone(),
            temp: h.temperature_2m[i].unwrap_or(f64::NAN),
            prob: h.precipitation_probability[i].unwrap_or(0.0),
            precip: h.precipitation[i].unwrap_or(0.0),
            code: h.weather_code[i].unwrap_or(0),
            is_day: h.is_day[i] == Some(1),
            gust: h.wind_gusts_10m[i].unwrap_or(0.0),
        })
        .collect();
    let d = raw.daily;
    let daily: Vec<Day> = (0..d.time.len())
        .map(|i| Day {
            date: d.time[i].clone(),
            code: d.weather_code[i].unwrap_or(0),
            max: d.temperature_2m_max[i].unwrap_or(f64::NAN),
            min: d.temperature_2m_min[i].unwrap_or(f64::NAN),
            prob: d.precipitation_probability_max[i].unwrap_or(0.0),
            precip: d.precipitation_sum[i].unwrap_or(0.0),
            gust: d.wind_gusts_10m_max[i].unwrap_or(0.0),
        })
        .collect();
    let alerts = alerts(&hourly, Thresholds::for_location(loc));
    Ok(Forecast {
        current,
        hourly,
        daily,
        alerts,
    })
}

/// 溫度警示的門檻(°C): (注意, 警告)
pub struct Thresholds {
    pub heat: (f64, f64),
    pub cold: (f64, f64),
}

impl Thresholds {
    /// 台灣: 參考中央氣象署(高溫 36/38, 低溫 10/6)
    pub const TAIWAN: Self = Self {
        heat: (36.0, 38.0),
        cold: (10.0, 6.0),
    };
    /// 國外: 各地氣候差很多, 只在比較極端時提醒
    pub const ABROAD: Self = Self {
        heat: (38.0, 41.0),
        cold: (-10.0, -20.0),
    };

    pub fn for_location(loc: &Location) -> &'static Self {
        if is_taiwan(loc.lat, loc.lon) {
            &Self::TAIWAN
        } else {
            &Self::ABROAD
        }
    }
}

/// 台灣本島與澎湖、金門、馬祖
pub fn is_taiwan(lat: f64, lon: f64) -> bool {
    (21.8..=26.5).contains(&lat) && (118.0..=122.2).contains(&lon)
}

/// 依未來 24 小時的預報判斷警示; 溫度依地點用不同門檻, 雨、風、雷雨的門檻相同
pub fn alerts(hourly: &[Hour], th: &Thresholds) -> Vec<Alert> {
    let next: Vec<&Hour> = hourly.iter().take(24).collect();
    if next.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let max = next.iter().map(|h| h.temp).fold(f64::MIN, f64::max);
    let min = next.iter().map(|h| h.temp).fold(f64::MAX, f64::min);
    let rain: f64 = next.iter().map(|h| h.precip).sum();
    let peak_rain = next.iter().map(|h| h.precip).fold(0.0, f64::max);
    let gust = next.iter().map(|h| h.gust).fold(0.0, f64::max);

    if max >= th.heat.0 {
        out.push(Alert {
            status: if max >= th.heat.1 {
                Status::Critical
            } else {
                Status::Warning
            },
            title: "高溫".into(),
            detail: format!("未來 24 小時最高 {max:.0}°C"),
        });
    }
    if min <= th.cold.0 {
        out.push(Alert {
            status: if min <= th.cold.1 {
                Status::Critical
            } else {
                Status::Warning
            },
            title: "低溫".into(),
            detail: format!("未來 24 小時最低 {min:.0}°C"),
        });
    }
    if rain >= 80.0 || peak_rain >= 40.0 {
        out.push(Alert {
            status: if rain >= 200.0 {
                Status::Critical
            } else {
                Status::Warning
            },
            title: if rain >= 200.0 { "豪雨" } else { "大雨" }.into(),
            detail: format!("未來 24 小時雨量 {rain:.0} mm, 最大時雨量 {peak_rain:.0} mm"),
        });
    }
    if gust >= 62.0 {
        out.push(Alert {
            status: if gust >= 89.0 {
                Status::Critical
            } else {
                Status::Warning
            },
            title: "強風".into(),
            detail: format!("陣風最高 {gust:.0} km/h ({} 級)", beaufort(gust)),
        });
    }
    if let Some(h) = next.iter().find(|h| (95..=99).contains(&h.code)) {
        out.push(Alert {
            status: Status::Warning,
            title: "雷雨".into(),
            detail: format!("{} 起可能有雷雨", h.time.get(11..16).unwrap_or("")),
        });
    }
    out
}

/// 風速(km/h) → 蒲福風級
fn beaufort(kmh: f64) -> u8 {
    const LIMITS: [f64; 12] = [
        1.0, 6.0, 12.0, 20.0, 29.0, 39.0, 50.0, 62.0, 75.0, 89.0, 103.0, 118.0,
    ];
    LIMITS.iter().position(|&l| kmh < l).unwrap_or(12) as u8
}

/// WMO 天氣代碼 → 中文描述
pub fn describe(code: u8) -> &'static str {
    match code {
        0 => "晴",
        1 => "大致晴朗",
        2 => "局部多雲",
        3 => "陰",
        45 | 48 => "霧",
        51 => "毛毛雨",
        53 => "毛毛雨",
        55 => "濃毛毛雨",
        56 | 57 => "凍毛毛雨",
        61 => "小雨",
        63 => "中雨",
        65 => "大雨",
        66 | 67 => "凍雨",
        71 => "小雪",
        73 => "中雪",
        75 => "大雪",
        77 => "雪粒",
        80 => "陣雨",
        81 => "較強陣雨",
        82 => "強陣雨",
        85 | 86 => "陣雪",
        95 => "雷雨",
        96 | 99 => "雷雨伴有冰雹",
        _ => "未知",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        let l = parse_locations("新店區@24.96,121.5389; 臺北市@25.0375,121.5637").unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "新店區");
        assert_eq!(
            locations_setting(&l),
            "新店區@24.9600,121.5389;臺北市@25.0375,121.5637"
        );
        assert!(parse_locations("").unwrap().is_empty());
        assert!(parse_locations(" ; ").unwrap().is_empty());
        assert!(parse_locations("abc").is_err());
        assert!(parse_locations("x@100,0").is_err());
    }

    #[test]
    fn taiwan_area() {
        assert!(is_taiwan(25.06, 121.53)); // 台北
        assert!(is_taiwan(24.45, 118.38)); // 金門
        assert!(is_taiwan(26.16, 119.95)); // 馬祖
        assert!(!is_taiwan(35.69, 139.69)); // Tokyo
        assert!(!is_taiwan(51.44, 5.48)); // Eindhoven
        assert!(!is_taiwan(22.32, 114.17)); // 香港
    }

    #[test]
    fn builtin_names() {
        assert_eq!(builtin("台北").unwrap().name, "臺北市");
        assert_eq!(builtin("臺北市").unwrap().name, "臺北市");
        assert_eq!(builtin("花蓮").unwrap().name, "花蓮縣");
        assert!(builtin("新店").is_none());
    }

    #[test]
    fn alert_thresholds() {
        let hour = |temp: f64, precip: f64, gust: f64, code: u8| Hour {
            time: "2026-01-01T03:00".into(),
            temp,
            prob: 0.0,
            precip,
            code,
            is_day: true,
            gust,
        };
        let tw = &Thresholds::TAIWAN;
        assert!(alerts(&[hour(25.0, 0.0, 10.0, 0)], tw).is_empty());
        let a = alerts(&[hour(37.0, 45.0, 70.0, 95)], tw);
        let titles: Vec<_> = a.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["高溫", "大雨", "強風", "雷雨"]);
        assert_eq!(a[0].status, Status::Warning);
        let cold = alerts(&[hour(5.0, 0.0, 0.0, 0)], tw);
        assert_eq!(cold[0].status, Status::Critical);
        // 國外: 4°C、37°C 都很普通, 不警示; 雨、風、雷雨照樣警示
        let abroad = &Thresholds::ABROAD;
        assert!(alerts(&[hour(4.0, 0.0, 0.0, 0)], abroad).is_empty());
        assert!(alerts(&[hour(37.0, 0.0, 0.0, 0)], abroad).is_empty());
        assert_eq!(
            alerts(&[hour(-12.0, 0.0, 0.0, 0)], abroad)[0].status,
            Status::Warning
        );
        assert_eq!(
            alerts(&[hour(-21.0, 0.0, 0.0, 0)], abroad)[0].status,
            Status::Critical
        );
        assert_eq!(alerts(&[hour(10.0, 45.0, 0.0, 0)], abroad)[0].title, "大雨");
        assert_eq!(beaufort(62.0), 8);
    }
}
