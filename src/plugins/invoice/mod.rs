//! Invoice plugin: 從 Gmail 收集電子發票, 開獎後自動對獎。
//!
//! - 搜尋含「發票」的信, 從內文、HTML、PDF 附件找出發票號碼(見 parse.rs)
//! - 第一次掃描全部的信, 之後定時只看新的信
//! - 中獎號碼來自財政部的 RSS(見 lottery.rs); 開獎日當天會一直檢查到號碼出來
//! - 中獎時通知(網頁右上角、message), 領獎期限剩 14 天還沒標成「已領取」再提醒一次
//! - 中獎的自動加到待辦清單(高優先, 到期日是領獎期限); 待辦打勾 = 已領取, 反過來也一樣
//! - 找不到號碼但看起來是發票的信另外列出, 可以手動加入或標成「不是發票」
//! - 資料存在 invoices.json(不進 Git)

pub mod lottery;
pub mod parse;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::plugin::{Ctx, Plugin, complete_from_help};
use crate::plugins::mail;
use crate::plugins::todo;
use crate::plugins::mail::imap::{Job, Op, Out};
use crate::webui::{Card, Status, WebMeta};
use lottery::{Period, Winning};

const FILE: &str = "invoices.json";
/// 搜尋可能有發票的信
const QUERY: &str = "發票";
/// 之後定時檢查新信的範圍
const RECENT_QUERY: &str = "發票 newer_than:3d";
/// 定時檢查新信的間隔
const RESCAN: Duration = Duration::from_secs(10 * 60);
/// 一次讀幾封信
const BATCH: usize = 10;
/// 領獎期限剩幾天時提醒
const REMIND_DAYS: i64 = 14;

/// 一張發票
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// 唯一的 key: 帳號:信件:號碼, 手動加入的是 manual:亂數
    pub key: String,
    #[serde(default)]
    pub account: String,
    /// Gmail 的信件 id(手動加入的可能沒有)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    /// 例如 AB12345678
    pub number: String,
    /// 期別, 例如 "11509"
    pub period: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
    #[serde(default)]
    pub seller: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub received: i64,
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub claimed: bool,
    /// 已經通知過: 1 中獎, 2 期限提醒
    #[serde(default)]
    pub notified: u8,
    /// 中獎時加到待辦清單的那一項
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
    /// 財政部中獎通知寫的獎別與金額(雲端發票專屬獎不在公開的中獎號碼裡)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_prize: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_amount: Option<u32>,
    /// 獎金會自動匯入(載具設定了領獎帳戶)
    #[serde(default)]
    pub auto_deposit: bool,
    /// 信已經被刪除(還沒領的中獎發票會保留, 其他的會從清單移除)
    #[serde(default)]
    pub mail_gone: bool,
}

/// 看起來是發票, 但找不到號碼的信
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unknown {
    pub account: String,
    pub msg: String,
    pub subject: String,
    pub from: String,
    pub received: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    records: Vec<Record>,
    #[serde(default)]
    unknown: Vec<Unknown>,
    /// 每個帳號已經看過的信
    #[serde(default)]
    scanned: BTreeMap<String, BTreeSet<String>>,
    /// 已經完整掃描過的帳號
    #[serde(default)]
    full_done: BTreeSet<String>,
    /// 標成「不是發票」或已刪除的信(帳號:信件)
    #[serde(default)]
    hidden: BTreeSet<String>,
    /// 各期的中獎號碼
    #[serde(default)]
    winning: BTreeMap<String, Winning>,
}

/// 掃描的進度(給網頁)
#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    pub running: bool,
    pub account: String,
    pub done: usize,
    pub total: usize,
    pub last: Option<i64>,
    pub error: Option<String>,
}

/// 一張發票的狀態(給網頁)
#[derive(Debug, Clone, Serialize)]
pub struct View {
    #[serde(flatten)]
    pub record: Record,
    /// pending 未開獎 / waiting 等號碼 / won 中獎 / lost 沒中獎 / expired 已過期
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prize: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prize_amount: Option<u32>,
    pub draw: String,
    pub claim_from: String,
    pub claim_to: String,
    /// 可以刪除這封信(這封信裡的發票都沒中或已過期)
    pub deletable: bool,
}

struct Inner {
    path: PathBuf,
    store: Mutex<Store>,
    progress: Mutex<Progress>,
    /// 要求掃描: Some(true) 全部重新掃描, Some(false) 只看新信
    request: Mutex<Option<bool>>,
    logs: Mutex<Vec<(bool, String)>>,
    started: AtomicBool,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared {
    inner: Arc<Inner>,
    mail: mail::Shared,
    todo: todo::Shared,
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn hid(account: &str, msg: &str) -> String {
    format!("{account}:{msg}")
}

/// 一張發票的狀態
fn evaluate(r: &Record, winning: &BTreeMap<String, Winning>, today: NaiveDate) -> (&'static str, Option<(String, u32)>) {
    let Some(p) = Period::parse_key(&r.period) else {
        return ("waiting", None);
    };
    let (_, claim_to) = p.claim();
    // 財政部通知中獎(例如雲端發票專屬獎)
    if let Some(prize) = &r.notice_prize {
        let won = Some((prize.clone(), r.notice_amount.unwrap_or(0)));
        return (if today > claim_to { "expired" } else { "won" }, won);
    }
    if today < p.draw_date() {
        return ("pending", None);
    }
    let won = winning
        .get(&r.period)
        .and_then(|w| lottery::check(&r.number[2..], w))
        .map(|(n, a)| (n.to_string(), a));
    if today > claim_to {
        return ("expired", won);
    }
    match (winning.get(&r.period), won) {
        (None, _) => ("waiting", None),
        (Some(_), Some(w)) => ("won", Some(w)),
        (Some(_), None) => ("lost", None),
    }
}

impl Shared {
    pub fn new(mail: mail::Shared, todo: todo::Shared) -> Self {
        let path = PathBuf::from(FILE);
        let store = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                path,
                store: Mutex::new(store),
                progress: Mutex::new(Progress::default()),
                request: Mutex::new(None),
                logs: Mutex::new(Vec::new()),
                started: AtomicBool::new(false),
            }),
            mail,
            todo,
        }
    }

    fn log(&self, warn: bool, text: impl Into<String>) {
        self.inner.logs.lock().unwrap().push((warn, format!("invoice: {}", text.into())));
    }

    fn save(&self, s: &Store) {
        if let Ok(data) = serde_json::to_vec_pretty(s) {
            let tmp = self.inner.path.with_extension("json.tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, &self.inner.path);
            }
        }
    }

    pub fn progress(&self) -> Progress {
        self.inner.progress.lock().unwrap().clone()
    }

    pub fn request_scan(&self, full: bool) {
        let mut r = self.inner.request.lock().unwrap();
        *r = Some(full || r.unwrap_or(false));
    }

    /// 全部的發票與狀態(新的期別在前)
    pub fn views(&self) -> Vec<View> {
        let s = self.inner.store.lock().unwrap();
        let t = today();
        let mut v: Vec<View> = s
            .records
            .iter()
            .filter(|r| r.manual || r.msg.as_ref().is_none_or(|m| !s.hidden.contains(&hid(&r.account, m))))
            .map(|r| {
                let (status, won) = evaluate(r, &s.winning, t);
                let p = Period::parse_key(&r.period);
                let (cf, ct) = p.map(|p| p.claim()).unzip();
                View {
                    record: r.clone(),
                    status,
                    prize_amount: won.as_ref().map(|w| w.1),
                    prize: won.map(|w| w.0),
                    draw: p.map(|p| p.draw_date().to_string()).unwrap_or_default(),
                    claim_from: cf.map(|d| d.to_string()).unwrap_or_default(),
                    claim_to: ct.map(|d| d.to_string()).unwrap_or_default(),
                    deletable: false,
                }
            })
            .collect();
        // 一封信裡的發票都沒中(或已過期)才能刪這封信
        let mut ok: BTreeMap<String, bool> = BTreeMap::new();
        for x in &v {
            if let Some(m) = &x.record.msg {
                let d = matches!(x.status, "lost" | "expired");
                let e = ok.entry(hid(&x.record.account, m)).or_insert(true);
                *e = *e && d;
            }
        }
        for x in &mut v {
            x.deletable = !x.record.mail_gone
                && x.record.msg.as_ref().is_some_and(|m| ok.get(&hid(&x.record.account, m)) == Some(&true));
        }
        v.sort_by(|a, b| {
            b.record
                .period
                .parse::<u32>()
                .unwrap_or(0)
                .cmp(&a.record.period.parse::<u32>().unwrap_or(0))
                .then(b.record.received.cmp(&a.record.received))
        });
        v
    }

    pub fn unknown(&self) -> Vec<Unknown> {
        let s = self.inner.store.lock().unwrap();
        let mut v: Vec<Unknown> = s
            .unknown
            .iter()
            .filter(|u| !s.hidden.contains(&hid(&u.account, &u.msg)))
            .cloned()
            .collect();
        v.sort_by_key(|u| std::cmp::Reverse(u.received));
        v
    }

    pub fn periods(&self) -> Vec<(String, Winning)> {
        let s = self.inner.store.lock().unwrap();
        s.winning.iter().rev().map(|(k, w)| (k.clone(), w.clone())).collect()
    }

    /// 手動加入一張發票(可以對應到一封信)
    pub fn add_manual(
        &self,
        number: &str,
        date: &str,
        amount: Option<u64>,
        note: &str,
        account: Option<String>,
        msg: Option<String>,
    ) -> Result<(), String> {
        let n: String = number.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
        if n.len() != 10 || !n[..2].chars().all(|c| c.is_ascii_uppercase()) || !n[2..].chars().all(|c| c.is_ascii_digit()) {
            return Err("發票號碼是兩個英文字母加 8 位數字, 例如 AB-12345678".into());
        }
        let d = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").map_err(|_| "請輸入發票日期".to_string())?;
        let mut s = self.inner.store.lock().unwrap();
        let mut rec = Record {
            key: format!("manual:{}", random_hex()),
            account: account.clone().unwrap_or_default(),
            msg: msg.clone(),
            number: n,
            period: Period::of(d).key(),
            date: Some(d.to_string()),
            amount,
            seller: String::new(),
            subject: note.trim().to_string(),
            received: chrono::Utc::now().timestamp(),
            manual: true,
            claimed: false,
            notified: 0,
            todo: None,
            notice_prize: None,
            notice_amount: None,
            auto_deposit: false,
            mail_gone: false,
        };
        // 從「找不到號碼」的信加入: 帶上那封信的資訊, 移出那個清單
        if let (Some(a), Some(m)) = (&account, &msg)
            && let Some(i) = s.unknown.iter().position(|u| &u.account == a && &u.msg == m)
        {
            let u = s.unknown.remove(i);
            rec.seller = u.from;
            if rec.subject.is_empty() {
                rec.subject = u.subject;
            }
            rec.received = u.received;
        }
        if s.records.iter().any(|r| r.number == rec.number && r.period == rec.period) {
            return Err("這張發票已經在清單裡了".into());
        }
        s.records.push(rec);
        self.save(&s);
        Ok(())
    }

    /// 移除手動加入的發票(不影響信件)
    pub fn remove_manual(&self, key: &str) -> Result<(), String> {
        let mut s = self.inner.store.lock().unwrap();
        let before = s.records.len();
        s.records.retain(|r| !(r.manual && r.key == key));
        if s.records.len() == before {
            return Err("找不到這張發票".into());
        }
        self.save(&s);
        Ok(())
    }

    pub fn set_claimed(&self, key: &str, claimed: bool) -> Result<(), String> {
        let mut s = self.inner.store.lock().unwrap();
        let r = s.records.iter_mut().find(|r| r.key == key).ok_or("找不到這張發票")?;
        r.claimed = claimed;
        let todo_id = r.todo.clone();
        self.save(&s);
        drop(s);
        // 待辦也跟著打勾(或取消)
        if let Some(id) = todo_id {
            let _ = self.todo.update(&id, todo::Patch { done: Some(claimed), ..Default::default() });
        }
        Ok(())
    }

    /// 「不是發票」: 不再列出這封信
    pub fn dismiss(&self, account: &str, msg: &str) {
        let mut s = self.inner.store.lock().unwrap();
        s.hidden.insert(hid(account, msg));
        s.unknown.retain(|u| !(u.account == account && u.msg == msg));
        self.save(&s);
    }

    /// 把信移到 Gmail 的垃圾桶(只能刪沒中獎或已過期的; 找不到號碼的信也可以刪)
    pub async fn delete_mails(&self, mails: Vec<(String, String)>) -> Result<usize, String> {
        let allowed: BTreeSet<String> = {
            let views = self.views();
            let mut ok: BTreeSet<String> = views
                .iter()
                .filter(|v| v.deletable)
                .filter_map(|v| v.record.msg.as_ref().map(|m| hid(&v.record.account, m)))
                .collect();
            for u in self.unknown() {
                ok.insert(hid(&u.account, &u.msg));
            }
            ok
        };
        let mut by_account: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for (a, m) in &mails {
            if !allowed.contains(&hid(a, m)) {
                return Err("這封信裡有中獎或還沒開獎的發票, 不能刪除".into());
            }
            let id: u64 = m.parse().map_err(|_| "信件 id 錯誤".to_string())?;
            by_account.entry(a.clone()).or_default().push(id);
        }
        let mut n = 0;
        for (a, ids) in by_account {
            self.mail.request(&a, Job::Op { ids: ids.clone(), op: Op::Trash }).await?;
            let mut s = self.inner.store.lock().unwrap();
            for id in &ids {
                s.hidden.insert(hid(&a, &id.to_string()));
                s.unknown.retain(|u| !(u.account == a && u.msg == id.to_string()));
            }
            self.save(&s);
            n += ids.len();
        }
        Ok(n)
    }

    /// 背景工作(有郵件帳號時才需要, 但開著也不花什麼)
    fn start(&self) {
        if self.inner.started.swap(true, Ordering::Relaxed) {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("invoice".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    return;
                };
                rt.block_on(me.worker());
            })
            .ok();
    }

    async fn worker(self) {
        let mut last_rss: Option<Instant> = None;
        let mut last_scan: Option<Instant> = None;
        // 啟動後先等郵件連線
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            // 中獎號碼: 平常每 6 小時; 開獎日(單數月 25 日)下午每 15 分鐘, 直到新的一期出來
            let t = chrono::Local::now();
            let draw_day = {
                use chrono::{Datelike, Timelike};
                t.day() == 25 && t.month() % 2 == 1 && t.hour() >= 13
            };
            let latest_missing = {
                let p = Period::of(t.date_naive());
                // 已經開獎、還沒有號碼的最新一期
                let prev = Period::new(if p.month == 1 { p.roc - 1 } else { p.roc }, if p.month == 1 { 11 } else { p.month - 2 });
                prev.is_some_and(|q| t.date_naive() >= q.draw_date() && !self.inner.store.lock().unwrap().winning.contains_key(&q.key()))
            };
            let every = if draw_day || latest_missing { Duration::from_secs(15 * 60) } else { Duration::from_secs(6 * 3600) };
            if last_rss.is_none_or(|l| l.elapsed() > every) {
                last_rss = Some(Instant::now());
                self.fetch_winning().await;
            }
            // 掃描信件
            let req = self.inner.request.lock().unwrap().take();
            let accounts: Vec<String> = self.mail.accounts().into_iter().map(|a| a.name).collect();
            let need_full = accounts.iter().any(|a| !self.inner.store.lock().unwrap().full_done.contains(a));
            if req.is_some() || need_full || last_scan.is_none_or(|l| l.elapsed() > RESCAN) {
                last_scan = Some(Instant::now());
                for a in &accounts {
                    let full = req == Some(true) || !self.inner.store.lock().unwrap().full_done.contains(a);
                    if let Err(e) = self.scan_account(a, full).await {
                        self.inner.progress.lock().unwrap().error = Some(e.clone());
                        self.log(true, format!("[{a}] 掃描失敗: {e}"));
                    }
                }
            }
            self.notify();
            // 每秒看一下有沒有手動要求
            for _ in 0..60 {
                if self.inner.request.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    async fn fetch_winning(&self) {
        let got = tokio::task::spawn_blocking(|| {
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(20)))
                .build()
                .new_agent();
            agent
                .get(lottery::RSS_URL)
                .call()
                .ok()?
                .body_mut()
                .with_config()
                .limit(1024 * 1024)
                .read_to_string()
                .ok()
        })
        .await
        .ok()
        .flatten();
        let Some(xml) = got else { return };
        let list = lottery::parse_rss(&xml);
        let mut s = self.inner.store.lock().unwrap();
        let mut new = Vec::new();
        for (p, w) in list {
            if s.winning.get(&p.key()) != Some(&w) {
                new.push(p.label());
                s.winning.insert(p.key(), w);
            }
        }
        if !new.is_empty() {
            self.save(&s);
            drop(s);
            self.log(false, format!("取得中獎號碼: {}", new.join("、")));
        }
    }

    async fn scan_account(&self, account: &str, full: bool) -> Result<(), String> {
        let q = if full { QUERY } else { RECENT_QUERY };
        let ids = match self.mail.request(account, Job::Search { q: q.into() }).await? {
            Out::Ids(v) => v,
            _ => return Err("回應錯誤".into()),
        };
        // 搜尋結果是空的: 可能是 Gmail 暫時出錯, 不要因此清掉整個清單
        if full && !ids.is_empty() {
            self.prune(account, &ids);
        }
        let todo: Vec<u64> = {
            let s = self.inner.store.lock().unwrap();
            let seen = s.scanned.get(account);
            ids.into_iter()
                .filter(|id| seen.is_none_or(|x| !x.contains(&id.to_string())) && !s.hidden.contains(&hid(account, &id.to_string())))
                .collect()
        };
        {
            let mut p = self.inner.progress.lock().unwrap();
            *p = Progress {
                running: true,
                account: account.to_string(),
                done: 0,
                total: todo.len(),
                last: p.last,
                error: None,
            };
        }
        if full && !todo.is_empty() {
            self.log(false, format!("[{account}] 開始掃描 {} 封可能有發票的信", todo.len()));
        }
        let mut found = 0;
        for chunk in todo.chunks(BATCH) {
            let raws = match self.mail.request(account, Job::Peek { ids: chunk.to_vec() }).await {
                Ok(Out::Raws(v)) => v,
                Ok(_) => Vec::new(),
                Err(e) => {
                    self.inner.progress.lock().unwrap().running = false;
                    return Err(e);
                }
            };
            // 解析(PDF 轉文字會花一點時間, 放到 blocking thread)
            let parsed = tokio::task::spawn_blocking(move || {
                raws.into_iter().map(|(id, ts, raw)| (id, ts, parse::scan(&raw, ts))).collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let mut s = self.inner.store.lock().unwrap();
            for (id, ts, sc) in parsed {
                let msg = id.to_string();
                s.scanned.entry(account.to_string()).or_default().insert(msg.clone());
                for f in &sc.invoices {
                    let key = format!("{account}:{msg}:{}", f.number);
                    if s.records.iter().any(|r| r.key == key || (r.number == f.number && r.period == f.period.key())) {
                        continue;
                    }
                    found += 1;
                    s.records.push(Record {
                        key,
                        account: account.to_string(),
                        msg: Some(msg.clone()),
                        number: f.number.clone(),
                        period: f.period.key(),
                        date: f.date.map(|d| d.to_string()),
                        amount: f.amount,
                        seller: sc.from.clone(),
                        subject: sc.subject.clone(),
                        received: ts,
                        manual: false,
                        claimed: false,
                        notified: 0,
                        todo: None,
                        notice_prize: None,
                        notice_amount: None,
                        auto_deposit: false,
            mail_gone: false,
                    });
                }
                // 財政部的中獎通知: 已經有這張就補上中獎資訊, 沒有就新增一筆
                for w in &sc.wins {
                    found += 1;
                    let pk = w.period.key();
                    match s.records.iter_mut().find(|r| r.period == pk && same_number(&r.number, &w.number)) {
                        Some(r) => {
                            r.notice_prize = Some(w.prize.clone());
                            r.notice_amount = Some(w.amount);
                            r.auto_deposit |= sc.auto_deposit;
                            r.claimed |= sc.auto_deposit;
                        }
                        None => s.records.push(Record {
                            key: format!("{account}:{msg}:{}", w.number),
                            account: account.to_string(),
                            msg: Some(msg.clone()),
                            number: w.number.clone(),
                            period: pk,
                            date: None,
                            amount: None,
                            seller: "財政部中獎通知".into(),
                            subject: sc.subject.clone(),
                            received: ts,
                            manual: false,
                            claimed: sc.auto_deposit,
                            notified: 0,
                            todo: None,
                            notice_prize: Some(w.prize.clone()),
                            notice_amount: Some(w.amount),
                            auto_deposit: sc.auto_deposit,
                            mail_gone: false,
                        }),
                    }
                }
                if sc.invoices.is_empty() && sc.wins.is_empty() && sc.likely && !s.unknown.iter().any(|u| u.account == account && u.msg == msg) {
                    s.unknown.push(Unknown {
                        account: account.to_string(),
                        msg,
                        subject: sc.subject,
                        from: sc.from,
                        received: ts,
                    });
                }
            }
            self.save(&s);
            drop(s);
            self.inner.progress.lock().unwrap().done += chunk.len();
        }
        {
            let mut s = self.inner.store.lock().unwrap();
            if full {
                s.full_done.insert(account.to_string());
            }
            self.save(&s);
        }
        {
            let mut p = self.inner.progress.lock().unwrap();
            p.running = false;
            p.last = Some(chrono::Utc::now().timestamp());
        }
        if full || found > 0 {
            self.log(false, format!("[{account}] 掃描完成, 新增 {found} 張發票"));
        }
        Ok(())
    }

    /// 全部重新掃描時: 信已經不在的(刪除、移到垃圾桶)從清單移除;
    /// 中獎還沒領的保留下來(標成信已刪除), 免得錯過
    fn prune(&self, account: &str, ids: &[u64]) {
        let alive: BTreeSet<String> = ids.iter().map(u64::to_string).collect();
        let t = today();
        let mut s = self.inner.store.lock().unwrap();
        let winning = s.winning.clone();
        let mut removed = 0;
        let mut kept = 0;
        s.records.retain_mut(|r| {
            let Some(m) = &r.msg else { return true };
            if r.account != account || alive.contains(m) {
                return true;
            }
            if r.manual {
                // 手動加入的發票自己保留, 只是信不在了
                r.mail_gone = true;
                return true;
            }
            let (status, won) = evaluate(r, &winning, t);
            if status == "won" && won.is_some() && !r.claimed {
                r.mail_gone = true;
                kept += 1;
                return true;
            }
            removed += 1;
            false
        });
        let before = s.unknown.len();
        s.unknown.retain(|u| u.account != account || alive.contains(&u.msg));
        removed += before - s.unknown.len();
        // 沒找到的信也不算掃描過(之後從垃圾桶救回來會再看一次)
        if let Some(seen) = s.scanned.get_mut(account) {
            seen.retain(|m| alive.contains(m));
        }
        if removed > 0 || kept > 0 {
            self.save(&s);
            drop(s);
            let mut note = format!("[{account}] 信已經不在了: 移除 {removed} 筆");
            if kept > 0 {
                note.push_str(&format!(", 保留 {kept} 張還沒領的中獎發票"));
            }
            self.log(false, note);
        }
    }

    /// 中獎通知、領獎期限提醒、加到待辦清單
    fn notify(&self) {
        let t = today();
        let todos: BTreeMap<String, bool> = self.todo.list().into_iter().map(|i| (i.id, i.done)).collect();
        let mut s = self.inner.store.lock().unwrap();
        let winning = s.winning.clone();
        let mut notes = Vec::new();
        let mut add_todo = Vec::new();
        let mut changed = false;
        for r in s.records.iter_mut() {
            let (status, won) = evaluate(r, &winning, t);
            let Some((prize, amount)) = won else { continue };
            if status != "won" {
                continue;
            }
            // 待辦打勾了 = 已領取
            if let Some(id) = &r.todo
                && todos.get(id) == Some(&true)
                && !r.claimed
            {
                r.claimed = true;
                changed = true;
            }
            // 自動匯入的: 通知一次就好, 不用加待辦
            if r.auto_deposit && r.notified < 1 {
                r.notified = 2;
                changed = true;
                notes.push((r.clone(), format!("發票中獎!{prize} {} 元(會自動匯入)", money(amount as u64))));
                continue;
            }
            if r.claimed {
                continue;
            }
            if r.todo.is_none() {
                add_todo.push((r.key.clone(), r.clone(), prize.clone(), amount));
            }
            let claim_to = Period::parse_key(&r.period).map(|p| p.claim().1);
            let soon = claim_to.is_some_and(|d| (d - t).num_days() <= REMIND_DAYS);
            let what = if r.notified < 1 {
                r.notified = if soon { 2 } else { 1 };
                format!("發票中獎!{prize} {amount} 元")
            } else if r.notified < 2 && soon {
                r.notified = 2;
                format!("發票 {} 元還沒領, {} 截止", amount, claim_to.map(|d| d.to_string()).unwrap_or_default())
            } else {
                continue;
            };
            notes.push((r.clone(), what));
        }
        if notes.is_empty() && add_todo.is_empty() && !changed {
            return;
        }
        self.save(&s);
        drop(s);
        // 中獎的加到待辦清單(整個 cluster 都看得到)
        for (key, r, prize, amount) in add_todo {
            let number = format!("{}-{}", &r.number[..2], &r.number[2..]);
            let who = if r.seller.is_empty() { r.subject.clone() } else { r.seller.clone() };
            let p = Period::parse_key(&r.period);
            let (from, to) = p.map(|p| p.claim()).unzip();
            let title = format!("兌獎: {prize} {} 元 {number}({who})", money(amount as u64));
            let notes = format!(
                "{} 期統一發票\n領獎期間: {} ~ {}\n雲端發票若已設定領獎帳戶會自動匯入, 不用臨櫃領取",
                p.map(|p| p.label()).unwrap_or_default(),
                from.map(|d| d.to_string()).unwrap_or_default(),
                to.map(|d| d.to_string()).unwrap_or_default()
            );
            match self.todo.add(&title, 1, to.map(|d| d.to_string()), &notes) {
                Ok(item) => {
                    let mut s = self.inner.store.lock().unwrap();
                    if let Some(x) = s.records.iter_mut().find(|x| x.key == key) {
                        x.todo = Some(item.id);
                    }
                    self.save(&s);
                }
                Err(e) => self.log(true, format!("無法加到待辦: {e}")),
            }
        }
        for (r, what) in notes {
            let number = format!("{}-{}", &r.number[..2], &r.number[2..]);
            self.log(true, format!("{what}: {number}({})", if r.seller.is_empty() { &r.subject } else { &r.seller }));
            self.mail.push_notice(mail::Notice {
                id: 0,
                account: if r.account.is_empty() { "發票".into() } else { r.account.clone() },
                from: what,
                subject: format!("{number} · {}", if r.seller.is_empty() { &r.subject } else { &r.seller }),
                msg: r.msg.clone().unwrap_or_default(),
                ts: chrono::Utc::now().timestamp(),
            });
        }
    }
}

/// 號碼相同(中獎通知的號碼中間會被遮住: * 可以是任何數字)
fn same_number(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.chars().zip(b.chars()).all(|(x, y)| x == y || x == '*' || y == '*')
}

fn random_hex() -> String {
    let mut b = [0u8; 6];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct Invoice {
    shared: Shared,
}

impl Invoice {
    pub fn new(shared: Shared) -> Self {
        shared.start();
        Self { shared }
    }
}

fn money(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Plugin for Invoice {
    fn name(&self) -> &str {
        "invoice"
    }

    fn description(&self) -> &str {
        "電子發票: 從 Gmail 收集、開獎後自動對獎"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("status", "掃描進度與中獎情況"),
            ("list", "列出發票與對獎結果"),
            ("scan", "檢查新的信"),
            ("scan full", "全部重新掃描"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        complete_from_help(&self.help(), args)
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            [] | ["status"] => {
                let p = self.shared.progress();
                let v = self.shared.views();
                if p.running {
                    ctx.info(format!("  掃描中 [{}] {}/{}", p.account, p.done, p.total));
                }
                let won: Vec<&View> = v.iter().filter(|x| x.status == "won").collect();
                ctx.info(format!(
                    "  共 {} 張發票, 中獎 {} 張(還沒領 {} 張), 未開獎 {} 張, 找不到號碼的信 {} 封",
                    v.len(),
                    won.len(),
                    won.iter().filter(|x| !x.record.claimed).count(),
                    v.iter().filter(|x| matches!(x.status, "pending" | "waiting")).count(),
                    self.shared.unknown().len()
                ));
                if let Some(e) = p.error {
                    ctx.warn(format!("  上次掃描失敗: {e}"));
                }
            }
            ["list"] => {
                for x in self.shared.views() {
                    let st = match x.status {
                        "won" => format!("中獎 {} {} 元", x.prize.as_deref().unwrap_or(""), money(x.prize_amount.unwrap_or(0) as u64)),
                        "lost" => "沒中獎".into(),
                        "pending" => format!("{} 開獎", x.draw),
                        "waiting" => "等中獎號碼".into(),
                        _ => "已過期".into(),
                    };
                    let p = Period::parse_key(&x.record.period).map(|p| p.label()).unwrap_or_default();
                    ctx.info(format!(
                        "  {p}  {}-{}  {st}  {}",
                        &x.record.number[..2],
                        &x.record.number[2..],
                        if x.record.seller.is_empty() { &x.record.subject } else { &x.record.seller }
                    ));
                }
            }
            ["scan"] => {
                self.shared.request_scan(false);
                ctx.info("invoice: 檢查新的信…");
            }
            ["scan", "full"] => {
                self.shared.request_scan(true);
                ctx.info("invoice: 全部重新掃描…");
            }
            _ => ctx.error("用法: 輸入 'help invoice' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        for (warn, l) in std::mem::take(&mut *self.shared.inner.logs.lock().unwrap()) {
            if warn {
                ctx.warn(l);
            } else {
                ctx.info(l);
            }
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("發票", "invoice", "green"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("invoice")
    }

    fn web_card(&self) -> Option<Card> {
        let v = self.shared.views();
        let p = self.shared.progress();
        let won: Vec<&View> = v.iter().filter(|x| x.status == "won" && !x.record.claimed).collect();
        let total: u64 = won.iter().map(|x| x.prize_amount.unwrap_or(0) as u64).sum();
        let summary = if p.running {
            format!("掃描中 {}/{}", p.done, p.total)
        } else if !won.is_empty() {
            format!("中獎 {} 張, 共 {} 元還沒領", won.len(), money(total))
        } else {
            format!("{} 張發票", v.len())
        };
        let mut card = Card::new()
            .status(if won.is_empty() { Status::Good } else { Status::Warning })
            .summary(summary);
        let pending = v.iter().filter(|x| matches!(x.status, "pending" | "waiting")).count();
        let rows: Vec<(String, String)> = won
            .iter()
            .take(5)
            .map(|x| (format!("{} 元", money(x.prize_amount.unwrap_or(0) as u64)), format!("{} 截止", x.claim_to)))
            .chain((pending > 0).then(|| ("未開獎".to_string(), format!("{pending} 張"))))
            .collect();
        if !rows.is_empty() {
            card = card.table(rows);
        }
        Some(card)
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(number: &str, period: &str) -> Record {
        Record {
            key: number.into(),
            account: "a".into(),
            msg: Some("1".into()),
            number: number.into(),
            period: period.into(),
            date: None,
            amount: None,
            seller: String::new(),
            subject: String::new(),
            received: 0,
            manual: false,
            claimed: false,
            notified: 0,
            todo: None,
            notice_prize: None,
            notice_amount: None,
            auto_deposit: false,
            mail_gone: false,
        }
    }

    #[test]
    fn statuses() {
        let mut w = BTreeMap::new();
        w.insert(
            "11509".to_string(),
            Winning {
                special: "97606522".into(),
                grand: "76160702".into(),
                first: vec!["02096005".into()],
            },
        );
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        // 開獎前
        assert_eq!(evaluate(&rec("AB11111005", "11509"), &w, d(2026, 11, 20)).0, "pending");
        // 開獎後: 中六獎、沒中
        assert_eq!(evaluate(&rec("AB11111005", "11509"), &w, d(2026, 11, 26)), ("won", Some(("六獎".to_string(), 200))));
        assert_eq!(evaluate(&rec("AB12345678", "11509"), &w, d(2026, 11, 26)).0, "lost");
        // 領獎期限過了
        assert_eq!(evaluate(&rec("AB11111005", "11509"), &w, d(2027, 3, 6)).0, "expired");
        // 沒有號碼的舊期別
        assert_eq!(evaluate(&rec("AB11111005", "11201"), &w, d(2026, 11, 26)).0, "expired");
        // 開獎了但號碼還沒抓到
        assert_eq!(evaluate(&rec("AB11111005", "11507"), &w, d(2026, 9, 26)).0, "waiting");
        // 財政部通知中雲端發票專屬獎(公開的號碼裡沒有)
        let mut r = rec("CU81234435", "11507");
        r.notice_prize = Some("雲端發票專屬五百元獎".into());
        r.notice_amount = Some(500);
        assert_eq!(evaluate(&r, &w, d(2026, 10, 1)), ("won", Some(("雲端發票專屬五百元獎".to_string(), 500))));
    }

    #[test]
    fn masked_numbers() {
        assert!(same_number("CU81234435", "CU8****435"));
        assert!(!same_number("CU81234436", "CU8****435"));
        assert!(!same_number("AB81234435", "CU8****435"));
    }

    #[test]
    fn money_format() {
        assert_eq!(money(200), "200");
        assert_eq!(money(10_000_000), "10,000,000");
    }
}
