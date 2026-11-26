//! 統一發票的期別、開獎與領獎日期、對獎。
//!
//! - 每兩個月一期(1-2、3-4…11-12 月), 單數月 25 日開獎(11-12 月那期是隔年 1/25)
//! - 領獎期間: 開獎的隔月 6 日起 3 個月(例如 11/25 開獎: 12/6 ~ 隔年 3/5)
//! - 中獎號碼: 財政部的 RSS(最近 6 期), 二獎到六獎是頭獎的末 7~3 碼

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

pub const RSS_URL: &str = "https://invoice.etax.nat.gov.tw/invoice.xml";

/// 一期: 民國年 + 開始的月份(單數), 例如 115 年 9-10 月 = (115, 9)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Period {
    pub roc: i32,
    pub month: u32,
}

impl Period {
    /// 西元日期 → 期別
    pub fn of(d: NaiveDate) -> Self {
        let m = d.month();
        Self {
            roc: d.year() - 1911,
            month: if m % 2 == 1 { m } else { m - 1 },
        }
    }

    pub fn new(roc: i32, month: u32) -> Option<Self> {
        (roc > 0 && (1..=12).contains(&month)).then(|| Self {
            roc,
            month: if month % 2 == 1 { month } else { month - 1 },
        })
    }

    /// 例如 "11509"
    pub fn key(&self) -> String {
        format!("{}{:02}", self.roc, self.month)
    }

    pub fn parse_key(s: &str) -> Option<Self> {
        if s.len() < 4 {
            return None;
        }
        let (y, m) = s.split_at(s.len() - 2);
        Self::new(y.parse().ok()?, m.parse().ok()?)
    }

    /// 例如 "115年09-10月"
    pub fn label(&self) -> String {
        format!("{}年{:02}-{:02}月", self.roc, self.month, self.month + 1)
    }

    /// 開獎日: 下一期第一個月的 25 日
    pub fn draw_date(&self) -> NaiveDate {
        let (y, m) = add_months(self.roc + 1911, self.month, 2);
        NaiveDate::from_ymd_opt(y, m, 25).expect("日期")
    }

    /// 領獎期間 (第一天, 最後一天)
    pub fn claim(&self) -> (NaiveDate, NaiveDate) {
        let (y, m) = add_months(self.roc + 1911, self.month, 3);
        let (y2, m2) = add_months(self.roc + 1911, self.month, 6);
        (
            NaiveDate::from_ymd_opt(y, m, 6).expect("日期"),
            NaiveDate::from_ymd_opt(y2, m2, 5).expect("日期"),
        )
    }
}

fn add_months(y: i32, m: u32, n: u32) -> (i32, u32) {
    let t = (m - 1 + n) as i32;
    (y + t / 12, (t % 12) as u32 + 1)
}

/// 一期的中獎號碼
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Winning {
    pub special: String,
    pub grand: String,
    pub first: Vec<String>,
}

/// 中的獎: (獎名, 獎金)
pub fn check(number8: &str, w: &Winning) -> Option<(&'static str, u32)> {
    if number8.len() != 8 {
        return None;
    }
    if number8 == w.special {
        return Some(("特別獎", 10_000_000));
    }
    if number8 == w.grand {
        return Some(("特獎", 2_000_000));
    }
    const TIERS: [(usize, &str, u32); 6] = [
        (8, "頭獎", 200_000),
        (7, "二獎", 40_000),
        (6, "三獎", 10_000),
        (5, "四獎", 4_000),
        (4, "五獎", 1_000),
        (3, "六獎", 200),
    ];
    for (n, name, amount) in TIERS {
        if w.first.iter().any(|f| f.len() == 8 && number8[8 - n..] == f[8 - n..]) {
            return Some((name, amount));
        }
    }
    None
}

/// 解析財政部的 RSS: 每一期的中獎號碼
pub fn parse_rss(xml: &str) -> Vec<(Period, Winning)> {
    let title_re = regex::Regex::new(r"(\d{2,3})年\s*(\d{1,2})\s*[~～-]\s*\d{1,2}月").unwrap();
    let num = |label: &str, desc: &str| -> Vec<String> {
        let re = regex::Regex::new(&format!(r"{label}[：:]\s*([\d、,，\s]+)")).unwrap();
        re.captures(desc)
            .map(|c| {
                c[1].split(['、', ',', '，', ' '])
                    .map(str::trim)
                    .filter(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for item in xml.split("<item>").skip(1) {
        let Some(t) = title_re.captures(item) else { continue };
        let Some(p) = Period::new(t[1].parse().unwrap_or(0), t[2].parse().unwrap_or(0)) else {
            continue;
        };
        let w = Winning {
            special: num("特別獎", item).into_iter().next().unwrap_or_default(),
            grand: num("特獎", item).into_iter().next().unwrap_or_default(),
            first: num("頭獎", item),
        };
        if w.special.len() == 8 && w.grand.len() == 8 && !w.first.is_empty() {
            out.push((p, w));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_and_dates() {
        let p = Period::of(NaiveDate::from_ymd_opt(2026, 10, 3).unwrap());
        assert_eq!(p, Period { roc: 115, month: 9 });
        assert_eq!(p.key(), "11509");
        assert_eq!(Period::parse_key("11509"), Some(p));
        assert_eq!(p.label(), "115年09-10月");
        assert_eq!(p.draw_date(), NaiveDate::from_ymd_opt(2026, 11, 25).unwrap());
        // 財政部公告: 115 年 9-10 月, 領獎期間 115/12/6 ~ 116/3/5
        assert_eq!(
            p.claim(),
            (NaiveDate::from_ymd_opt(2026, 12, 6).unwrap(), NaiveDate::from_ymd_opt(2027, 3, 5).unwrap())
        );
        // 11-12 月那期: 隔年 1/25 開獎
        let p = Period::of(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap());
        assert_eq!(p.draw_date(), NaiveDate::from_ymd_opt(2026, 1, 25).unwrap());
    }

    #[test]
    fn prizes() {
        let w = Winning {
            special: "97606522".into(),
            grand: "76160702".into(),
            first: vec!["02096005".into(), "28745017".into(), "76740352".into()],
        };
        assert_eq!(check("97606522", &w), Some(("特別獎", 10_000_000)));
        assert_eq!(check("76160702", &w), Some(("特獎", 2_000_000)));
        assert_eq!(check("28745017", &w), Some(("頭獎", 200_000)));
        assert_eq!(check("18745017", &w), Some(("二獎", 40_000)));
        assert_eq!(check("11111005", &w), Some(("六獎", 200)));
        assert_eq!(check("11110352", &w), Some(("五獎", 1_000)));
        assert_eq!(check("12345678", &w), None);
    }

    #[test]
    fn rss() {
        let xml = r#"<rss><channel><item><title><![CDATA[115年 09~10月]]></title>
        <description><![CDATA[<p>特別獎：97606522</p><p>特獎：76160702</p><p>頭獎：02096005、28745017、76740352</p>]]></description></item>
        <item><title><![CDATA[114年 11~12月]]></title><description><![CDATA[<p>特別獎：97023797</p><p>特獎：00507588</p><p>頭獎：92377231、05232592、78125249</p>]]></description></item></channel></rss>"#;
        let v = parse_rss(xml);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, Period { roc: 115, month: 9 });
        assert_eq!(v[0].1.first.len(), 3);
        assert_eq!(v[1].1.grand, "00507588");
    }
}
