//! 從一封信找出電子發票號碼(內文、HTML、PDF 附件)。
//!
//! 號碼是兩個大寫英文字母 + 8 位數字(例如 AB-12345678), 但訂單編號也可能長這樣, 所以要有上下文:
//! - 前面有「發票號碼」「發票編號」「發票字軌」(不是「折讓」)
//! - 或旁邊有期別(例如「115年09-10月」, 電子發票證明聯 PDF 的格式)
//!
//! 期別: 信裡寫的期別 → 發票日期 → 收到信的日期。

use std::sync::LazyLock;

use chrono::{NaiveDate, TimeZone};
use mail_parser::MessageParser;
use regex::Regex;

use super::lottery::Period;

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// 例如 AB12345678
    pub number: String,
    pub period: Period,
    pub date: Option<NaiveDate>,
    pub amount: Option<u64>,
}

/// 財政部「雲端發票中獎通知」裡的一筆
#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    /// 可能被遮住一部分, 例如 CU8****435
    pub number: String,
    pub period: Period,
    /// 例如「雲端發票專屬五百元獎」「六獎」
    pub prize: String,
    pub amount: u32,
}

#[derive(Debug, Default)]
pub struct Scanned {
    pub subject: String,
    pub from: String,
    pub invoices: Vec<Found>,
    /// 財政部的中獎通知: 中獎的發票(可能是悠遊卡等其他載具的, 不一定有 email)
    pub wins: Vec<Win>,
    /// 中獎通知寫著「已啟用自動匯款」: 獎金會自動匯入, 不用自己領
    pub auto_deposit: bool,
    /// 看起來是發票的信(主旨或內容提到發票、有 PDF), 但找不到號碼
    pub likely: bool,
}

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^A-Za-z0-9])([A-Z]{2})\s*[-－]?\s*(\d{8})(?:[^0-9]|$)").unwrap());
static LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"發\s*票\s*(?:號\s*碼|編\s*號|字\s*軌)").unwrap());
static PERIOD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{2,3})\s*年\s*(\d{1,2})\s*[-~－～]\s*(\d{1,2})\s*月").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:發票日期|開立日期|開立時間|開立日|交易日期|日期)\s*[:：]?\s*(\d{3,4})\s*[/\-.年]\s*(\d{1,2})\s*[/\-.月]\s*(\d{1,2})").unwrap()
});
/// 中獎通知的表格一列: 發票號碼 | 中獎獎別 | 中獎金額(號碼中間 4 碼會被遮住, 例如 CU8****435)
static WIN_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([A-Z]{2})[-－]?([\d*]{8})[\s|]*([^\s|\d*][^\s|]{0,19}獎)[\s|]*([\d,]{2,12})").unwrap());
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:總計|總金額|發票金額|應付金額|合計|金額|Total)\s*[:：]?\s*(?:NT\$|NTD|TWD|\$|新台幣|新臺幣)?\s*([\d,]{1,12})").unwrap()
});

/// s 在 byte 位置 i 往前 n 個字元的位置
fn back(s: &str, i: usize, n: usize) -> usize {
    s[..i].char_indices().rev().nth(n.saturating_sub(1)).map_or(0, |(j, _)| j)
}

/// s 在 byte 位置 i 往後 n 個字元的位置
fn fwd(s: &str, i: usize, n: usize) -> usize {
    s[i..].char_indices().nth(n).map_or(s.len(), |(j, _)| i + j)
}

fn to_date(y: &str, m: &str, d: &str) -> Option<NaiveDate> {
    let mut y: i32 = y.parse().ok()?;
    if y < 1911 {
        y += 1911; // 民國年
    }
    NaiveDate::from_ymd_opt(y, m.parse().ok()?, d.parse().ok()?)
}

/// 一段文字裡的發票
pub fn find_in_text(text: &str, received: NaiveDate) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for c in NUMBER.captures_iter(text) {
        let m = c.get(1).unwrap();
        let start = m.start();
        let end = c.get(2).unwrap().end();
        let before = &text[back(text, start, 30)..start];
        let before_compact: String = before.chars().filter(|c| !c.is_whitespace()).collect();
        let labelled = LABEL.find_iter(&before_compact).last().is_some_and(|l| {
            // 「折讓發票號碼」不能兌獎
            !before_compact[..l.start()].ends_with("折讓")
        });
        let near = &text[back(text, start, 60)..fwd(text, end, 40)];
        let period_near = PERIOD.captures(near);
        if !labelled && period_near.is_none() {
            continue;
        }
        let around = &text[back(text, start, 300)..fwd(text, end, 300)];
        let date = DATE.captures(around).and_then(|d| to_date(&d[1], &d[2], &d[3]));
        let period = period_near
            .and_then(|p| Period::new(p[1].parse().ok()?, p[2].parse().ok()?))
            .or_else(|| date.map(Period::of))
            .unwrap_or_else(|| Period::of(received));
        let after = &text[end..fwd(text, end, 400)];
        let amount = AMOUNT
            .captures(after)
            .or_else(|| AMOUNT.captures(around))
            .and_then(|a| a[1].replace(',', "").parse::<u64>().ok())
            .filter(|a| *a > 0);
        let number = format!("{}{}", &c[1], &c[2]);
        if let Some(f) = out.iter_mut().find(|f| f.number == number) {
            // 同一張出現好幾次: 補上缺的資料
            f.date = f.date.or(date);
            f.amount = f.amount.or(amount);
            continue;
        }
        out.push(Found {
            number,
            period,
            date,
            amount,
        });
    }
    out
}

/// PDF → 文字(用 poppler 的 pdftotext; 沒有安裝就略過)
fn pdf_text(data: &[u8]) -> Option<String> {
    let dir = std::env::temp_dir().join(format!("cng6-inv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{:x}.pdf", data.len() ^ data.iter().take(64).map(|b| *b as usize).sum::<usize>()));
    std::fs::write(&path, data).ok()?;
    let out = std::process::Command::new("pdftotext")
        .args(["-layout", "-q"])
        .arg(&path)
        .arg("-")
        .stdin(std::process::Stdio::null())
        .output();
    let _ = std::fs::remove_file(&path);
    let out = out.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 原始的標頭(沒有標示編碼的 8 位元標頭, 台灣常見的是 Big5): 解碼成亂碼時用
fn raw_header_big5(raw: &[u8], name: &str) -> Option<String> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").or_else(|| raw.windows(2).position(|w| w == b"\n\n"))?;
    let head = &raw[..end];
    let lower: Vec<u8> = head.iter().map(|b| b.to_ascii_lowercase()).collect();
    let key = format!("\n{}:", name.to_ascii_lowercase());
    let start = lower.windows(key.len()).position(|w| w == key.as_bytes())? + key.len();
    let mut v = Vec::new();
    let mut i = start;
    while i < head.len() {
        // 下一行不是空白開頭: 標頭結束
        if head[i] == b'\n' && head.get(i + 1).is_none_or(|c| *c != b' ' && *c != b'\t') {
            break;
        }
        if head[i] != b'\r' && head[i] != b'\n' {
            v.push(head[i]);
        }
        i += 1;
    }
    let (s, _, bad) = encoding_rs::BIG5.decode(&v);
    (!bad).then(|| s.trim().to_string())
}

/// 掃描一封信
pub fn scan(raw: &[u8], received: i64) -> Scanned {
    let mut r = Scanned::default();
    let Some(msg) = MessageParser::new().parse(raw) else {
        return r;
    };
    r.subject = msg.subject().unwrap_or("").trim().to_string();
    r.from = msg
        .from()
        .and_then(|l| l.first())
        .map(|a| a.name().or(a.address()).unwrap_or("").to_string())
        .unwrap_or_default();
    if r.subject.contains('\u{FFFD}')
        && let Some(s) = raw_header_big5(raw, "Subject")
    {
        r.subject = s;
    }
    if r.from.contains('\u{FFFD}')
        && let Some(f) = raw_header_big5(raw, "From")
    {
        r.from = f.split('<').next().unwrap_or("").trim().trim_matches('"').to_string();
    }
    let day = chrono::Local
        .timestamp_opt(received, 0)
        .single()
        .map(|t| t.date_naive())
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    let mut texts: Vec<String> = Vec::new();
    let mut has_pdf = false;
    for p in msg.parts() {
        if p.is_multipart() || p.is_message() {
            continue;
        }
        let ct = p
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("")).to_ascii_lowercase())
            .unwrap_or_default();
        let name = p.attachment_name().unwrap_or("").to_lowercase();
        let data = p.decoded();
        if ct == "application/pdf" || name.ends_with(".pdf") || data.starts_with(b"%PDF") {
            has_pdf = true;
            if let Some(t) = pdf_text(&data) {
                texts.push(t);
            }
        } else if ct == "text/html" || name.ends_with(".html") || name.ends_with(".htm") {
            texts.push(mail_parser::html_to_text(&String::from_utf8_lossy(&p.text().map(|t| t.into_owned().into_bytes()).unwrap_or_else(|| data.to_vec()))));
        } else if ct.starts_with("text/") || name.ends_with(".txt") {
            texts.push(p.text().map(|t| t.into_owned()).unwrap_or_else(|| String::from_utf8_lossy(&data).into_owned()));
        }
    }
    for t in &texts {
        for f in find_in_text(t, day) {
            if let Some(old) = r.invoices.iter_mut().find(|x| x.number == f.number) {
                old.date = old.date.or(f.date);
                old.amount = old.amount.or(f.amount);
            } else {
                r.invoices.push(f);
            }
        }
    }
    // 財政部的中獎通知: 表格裡的每一筆都是中獎的發票
    let all_text = texts.join("\n");
    if r.subject.contains("中獎通知") && all_text.contains("中獎獎別") {
        let period = PERIOD
            .captures(&r.subject)
            .or_else(|| PERIOD.captures(&all_text))
            .and_then(|p| Period::new(p[1].parse().ok()?, p[2].parse().ok()?))
            .unwrap_or_else(|| Period::of(day));
        for c in WIN_ROW.captures_iter(&all_text) {
            let number = format!("{}{}", &c[1], &c[2]);
            if r.wins.iter().any(|w| w.number == number) {
                continue;
            }
            r.wins.push(Win {
                number,
                period,
                prize: c[3].to_string(),
                amount: c[4].replace(',', "").parse().unwrap_or(0),
            });
        }
        r.auto_deposit = all_text.contains("自動匯款") && !all_text.contains("未啟用自動匯款");
        // 通知裡的號碼不是一般的發票(上面已經處理)
        r.invoices.retain(|f| !r.wins.iter().any(|w| w.number == f.number));
    }
    if r.invoices.is_empty() && r.wins.is_empty() {
        let all = all_text;
        r.likely = r.subject.contains("發票") || all.contains("電子發票") || LABEL.is_match(&all) || has_pdf;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn labelled_numbers() {
        let t = "您的電子發票已開立。 發票號碼：AB-12345678 發票日期：2026/10/03 總計：NT$1,280 訂單編號 XY98765432";
        let v = find_in_text(t, d(2026, 11, 1));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].number, "AB12345678");
        assert_eq!(v[0].period, Period { roc: 115, month: 9 });
        assert_eq!(v[0].date, Some(d(2026, 10, 3)));
        assert_eq!(v[0].amount, Some(1280));
    }

    #[test]
    fn proof_layout_and_roc_date() {
        // 證明聯 PDF: 期別旁邊就是號碼, 日期是民國年
        let t = "電子發票證明聯\n115年07-08月\nCD-87654321\n2026-08-15 10:00\n總計 350";
        let v = find_in_text(t, d(2026, 11, 1));
        assert_eq!(v[0].number, "CD87654321");
        assert_eq!(v[0].period, Period { roc: 115, month: 7 });
        let t = "發 票 號 碼：EF12345678 開立日期：115/05/20";
        let v = find_in_text(t, d(2026, 11, 1));
        assert_eq!(v[0].period, Period { roc: 115, month: 5 });
    }

    #[test]
    fn rejects() {
        // 沒有上下文的編號、折讓單、9 位數
        assert!(find_in_text("訂單編號 AB12345678", d(2026, 1, 1)).is_empty());
        assert!(find_in_text("折讓發票號碼：AB12345678", d(2026, 1, 1)).is_empty());
        assert!(find_in_text("發票號碼：AB123456789", d(2026, 1, 1)).is_empty());
        // 沒有日期: 用收到信的日期
        let v = find_in_text("發票號碼 GH-11112222", d(2026, 2, 14));
        assert_eq!(v[0].period, Period { roc: 115, month: 1 });
    }

    #[test]
    fn mof_win_notice() {
        let raw = "From: einvoice@einvoice.nat.gov.tw\r\nSubject: =?UTF-8?B?\
6LKh5pS/6YOo6Zu75a2Q55m856Wo5pW05ZCI5pyN5YuZ5bmz5Y+wIOKAkyAxMTXlubQwNy0wOOaciOacn+mbsuerr+eZvOelqOS4reeNjumAmuefpQ==?=\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n<p>\u{60a8}\u{7684}\u{624b}\u{6a5f}\u{689d}\u{78bc}\u{5df2}\u{555f}\u{7528}\u{81ea}\u{52d5}\u{532f}\u{6b3e}</p>\
<table><tr><td>發票號碼</td><td>中獎獎別</td><td>中獎金額</td><td>載具類別</td></tr>\
<tr><td>CU8****435</td><td>雲端發票專屬五百元獎</td><td>500</td><td>悠遊卡</td></tr></table>\r\n";
        let s = scan(raw.as_bytes(), 1_790_000_000);
        assert_eq!(s.wins.len(), 1, "{s:?}");
        assert_eq!(s.wins[0].number, "CU8****435");
        assert_eq!(s.wins[0].prize, "雲端發票專屬五百元獎");
        assert_eq!(s.wins[0].amount, 500);
        assert_eq!(s.wins[0].period, Period { roc: 115, month: 7 });
        assert!(s.auto_deposit);
        assert!(!s.likely);
    }

    #[test]
    fn big5_subject() {
        let (b, _, _) = encoding_rs::BIG5.encode("訂購確認");
        let mut raw = b"From: shop@example.com\r\nSubject: ".to_vec();
        raw.extend_from_slice(&b);
        raw.extend_from_slice(b"\r\n\r\nhi\r\n");
        let s = scan(&raw, 1_790_000_000);
        assert_eq!(s.subject, "訂購確認");
    }

    #[test]
    fn whole_message() {
        let raw = "From: PChome <a@pchome.com.tw>\r\nSubject: 發票開立通知\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>發票號碼：<b>JK-24681357</b></p><p>發票日期：2026-09-30</p>\r\n";
        let s = scan(raw.as_bytes(), 1_790_000_000);
        assert_eq!(s.subject, "發票開立通知");
        assert_eq!(s.invoices.len(), 1);
        assert_eq!(s.invoices[0].period, Period { roc: 115, month: 9 });
        let raw = "From: x@y.com\r\nSubject: 您的電子發票\r\n\r\n請見附件\r\n";
        let s = scan(raw.as_bytes(), 1_790_000_000);
        assert!(s.invoices.is_empty() && s.likely);
    }
}

