//! Gmail IMAP 連線。
//!
//! 所有 IMAP 連線都在這個模組自己的 thread(tokio runtime)裡, 網頁透過 channel 送要求過來。
//! 每個帳號兩條連線:
//! - watch: 在收件匣 IDLE, 有新信時通知, 並更新未讀數與最新幾封
//! - command: 處理網頁的要求(列表、讀信、標記…), 大部分在「所有郵件」裡操作
//!
//! 信件用 Gmail 的 X-GM-MSGID 識別(每封信在所有標籤裡都一樣), 不用各資料夾不同的 UID。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_imap::Session;
use async_imap::types::{Fetch, Flag, NameAttribute};
use futures_util::TryStreamExt;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;

use super::{Account, Notice, Shared};

type Sess = Session<TlsStream<TcpStream>>;

const HOST: &str = "imap.gmail.com";
const PORT: u16 = 993;
/// 一頁幾封
pub const PAGE: usize = 50;
/// 卡片上顯示最新幾封
const RECENT: u32 = 5;
/// 一次通知最多列出幾封
const NOTIFY_MAX: usize = 5;
/// Gmail 會在 30 分鐘沒動靜時斷線, IDLE 要在這之前重新開始
const IDLE_RENEW: Duration = Duration::from_secs(25 * 60);
/// 一個要求最多等多久
const JOB_TIMEOUT: Duration = Duration::from_secs(90);
/// 記住最近讀過的幾封信(網頁讀信時會連續要內文、圖片、附件)
const CACHE: usize = 8;

/// 信件摘要(列表、卡片)
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// X-GM-MSGID(字串, JavaScript 的數字放不下 64 位元)
    pub id: String,
    pub from: String,
    pub from_addr: String,
    /// 寄件者公司的網域(個人信箱是 None), 網頁用來顯示圖示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    pub subject: String,
    /// 收到的時間(unix 秒)
    pub ts: i64,
    pub unread: bool,
    pub starred: bool,
    pub attach: bool,
    /// Gmail 標籤(不含 \Inbox 這類系統標籤)
    pub labels: Vec<String>,
    pub inbox: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Label {
    /// IMAP 的名稱(開資料夾用)
    pub name: String,
    /// 顯示用的名稱(解碼過的中文)
    pub display: String,
    /// 系統資料夾: all / trash / sent / drafts / junk / flagged / important; 一般標籤是 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub special: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct ListPage {
    pub total: usize,
    pub offset: usize,
    pub items: Vec<Summary>,
}

pub enum Job {
    Labels,
    /// 搜尋(Gmail 的搜尋語法); mailbox 是 None 時在「所有郵件」裡找
    List {
        mailbox: Option<String>,
        q: String,
        /// 從第幾封開始(新到舊)
        offset: usize,
    },
    /// 整封信的原始內容(讀信會標成已讀)
    Raw { id: u64 },
    Op { ids: Vec<u64>, op: Op },
    /// 在「所有郵件」裡搜尋(Gmail 語法), 回傳信件 id(新到舊)
    Search { q: String },
    /// 讀整封信但不標成已讀(例如掃描發票): (id, 收到的時間, 內容)
    Peek { ids: Vec<u64> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Read,
    Unread,
    Star,
    Unstar,
    /// 移出收件匣
    Archive,
    /// 移到收件匣(也可以從垃圾桶救回來)
    Inbox,
    Trash,
}

impl Op {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "read" => Self::Read,
            "unread" => Self::Unread,
            "star" => Self::Star,
            "unstar" => Self::Unstar,
            "archive" => Self::Archive,
            "inbox" => Self::Inbox,
            "trash" => Self::Trash,
            _ => return None,
        })
    }
}

pub enum Out {
    Labels(Vec<Label>),
    List(ListPage),
    Raw(Arc<Vec<u8>>),
    Ids(Vec<u64>),
    Raws(Vec<(u64, i64, Arc<Vec<u8>>)>),
    Done,
}

pub struct Request {
    pub account: String,
    pub job: Job,
    pub reply: oneshot::Sender<Result<Out, String>>,
}

pub enum Ctl {
    /// 帳號改變: 全部重新連線
    Reload,
    Req(Request),
    /// 測試帳號密碼能不能登入
    Test {
        email: String,
        password: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// 啟動 IMAP 的 thread
pub fn spawn(shared: Shared) -> mpsc::UnboundedSender<Ctl> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Ctl>();
    std::thread::Builder::new()
        .name("mail".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    shared.log(true, format!("無法啟動: {e}"));
                    return;
                }
            };
            rt.block_on(async move {
                let mut running: HashMap<String, Worker> = HashMap::new();
                start_all(&shared, &mut running);
                while let Some(c) = rx.recv().await {
                    match c {
                        Ctl::Reload => {
                            for (_, w) in running.drain() {
                                w.stop();
                            }
                            shared.clear_status();
                            start_all(&shared, &mut running);
                        }
                        Ctl::Req(r) => match running.get(&r.account) {
                            Some(w) => {
                                let _ = w.cmd.send(r);
                            }
                            None => {
                                let _ = r.reply.send(Err(format!("沒有帳號 '{}'", r.account)));
                            }
                        },
                        Ctl::Test { email, password, reply } => {
                            tokio::spawn(async move {
                                let r = match connect(&email, &password).await {
                                    Ok(mut s) => {
                                        let _ = s.logout().await;
                                        Ok(())
                                    }
                                    Err(e) => Err(e),
                                };
                                let _ = reply.send(r);
                            });
                        }
                    }
                }
            });
        })
        .ok();
    tx
}

struct Worker {
    cmd: mpsc::UnboundedSender<Request>,
    tasks: [tokio::task::JoinHandle<()>; 2],
}

impl Worker {
    fn stop(self) {
        for t in self.tasks {
            t.abort();
        }
    }
}

fn start_all(shared: &Shared, running: &mut HashMap<String, Worker>) {
    for acct in shared.accounts() {
        let (tx, rx) = mpsc::unbounded_channel();
        let watch = tokio::spawn(watch_loop(acct.clone(), shared.clone()));
        let cmd = tokio::spawn(command_loop(acct.clone(), rx));
        running.insert(acct.name.clone(), Worker { cmd: tx, tasks: [watch, cmd] });
    }
}

// ------------------------------------------------------------------ 連線

fn tls() -> TlsConnector {
    use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto::ring};
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let cfg = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("TLS 版本")
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(cfg))
}

async fn connect(email: &str, password: &str) -> Result<Sess, String> {
    let fut = async {
        let tcp = TcpStream::connect((HOST, PORT))
            .await
            .map_err(|e| format!("無法連線 {HOST}: {e}"))?;
        let name = ServerName::try_from(HOST).map_err(|e| e.to_string())?;
        let stream = tls()
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS 失敗: {e}"))?;
        let mut client = async_imap::Client::new(stream);
        // 伺服器的歡迎訊息
        client
            .read_response()
            .await
            .map_err(|e| format!("連線中斷: {e}"))?
            .ok_or("連線中斷")?;
        // 應用程式密碼顯示時有空白, 貼上時可能帶著
        let password: String = password.chars().filter(|c| !c.is_whitespace()).collect();
        client
            .login(email, &password)
            .await
            .map_err(|(e, _)| login_error(&e.to_string()))
    };
    timeout(Duration::from_secs(30), fut)
        .await
        .map_err(|_| "連線逾時".to_string())?
}

fn login_error(e: &str) -> String {
    if e.contains("AUTHENTICATIONFAILED") || e.contains("Invalid credentials") {
        "帳號或應用程式密碼錯誤(要用 Google 帳戶產生的「應用程式密碼」, 不是 Gmail 的登入密碼)".into()
    } else if e.contains("Web login required") || e.contains("WEBALERT") {
        "Google 要求先在瀏覽器登入確認, 或帳號沒有開啟兩步驟驗證".into()
    } else {
        format!("登入失敗: {e}")
    }
}

/// 把 IMAP 錯誤轉成文字
fn e2s(e: async_imap::error::Error) -> String {
    e.to_string()
}

/// IMAP 的字串(加引號, 跳脫 \ 與 ")
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// UID SEARCH 加上 Gmail 搜尋條件(X-GM-RAW)。`before` 是放在前面的其他條件(可以是空的)。
///
/// Gmail 不接受引號裡的 UTF-8(不會報錯, 只是找不到), 中文要用 literal 傳送:
/// 先送長度 `{n}`, 等伺服器回「+」, 再送內容。
async fn gm_search(s: &mut Sess, before: &str, q: &str) -> Result<Vec<u32>, String> {
    use async_imap::imap_proto::{MailboxDatum, Response, Status};
    if q.is_ascii() {
        let crit = format!("{before}X-GM-RAW {}", quote(q));
        let r = s.uid_search(&crit).await.map_err(e2s)?;
        drain(s);
        return Ok(r.into_iter().collect());
    }
    let tag = s
        .run_command(format!("UID SEARCH CHARSET UTF-8 {before}X-GM-RAW {{{}}}", q.len()))
        .await
        .map_err(e2s)?;
    loop {
        let r = s.read_response().await.map_err(|e| e.to_string())?.ok_or("連線中斷")?;
        match r.parsed() {
            Response::Continue(_) => break,
            Response::Done { tag: t, .. } if *t == tag => return Err("Gmail 不接受這個搜尋條件".into()),
            _ => {}
        }
    }
    s.run_command_untagged(q).await.map_err(e2s)?;
    let mut out = Vec::new();
    loop {
        let r = s.read_response().await.map_err(|e| e.to_string())?.ok_or("連線中斷")?;
        match r.parsed() {
            Response::MailboxData(MailboxDatum::Search(v)) => out.extend(v),
            Response::Done { tag: t, status, .. } if *t == tag => {
                if *status == Status::Ok {
                    break;
                }
                return Err("Gmail 不接受這個搜尋條件".into());
            }
            _ => {}
        }
    }
    drain(s);
    Ok(out)
}

/// IMAP 資料夾名稱的編碼(modified UTF-7, RFC 3501 5.1.3)轉成一般文字, 例如 `&ZeVnLIqe-` → 日本語
pub fn decode_mutf7(name: &str) -> String {
    fn b64(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b',' => 63,
            _ => return None,
        } as u32)
    }
    let mut out = String::new();
    let mut rest = name;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let Some(j) = rest.find('-') else {
            out.push('&');
            break;
        };
        let enc = &rest[..j];
        rest = &rest[j + 1..];
        if enc.is_empty() {
            out.push('&');
            continue;
        }
        let (mut bits, mut n, mut units) = (0u32, 0, Vec::new());
        let mut bytes = Vec::new();
        for c in enc.bytes() {
            let Some(v) = b64(c) else {
                return name.to_string();
            };
            bits = (bits << 6) | v;
            n += 6;
            if n >= 8 {
                n -= 8;
                bytes.push((bits >> n) as u8);
                bits &= (1 << n) - 1;
            }
        }
        for pair in bytes.as_chunks::<2>().0 {
            units.push(u16::from_be_bytes(*pair));
        }
        out.push_str(&String::from_utf16_lossy(&units));
    }
    out.push_str(rest);
    out
}

/// 清掉沒人讀的伺服器通知(channel 滿了會擋住)
fn drain(s: &Sess) {
    while s.unsolicited_responses.try_recv().is_ok() {}
}

// ------------------------------------------------------------------ 摘要

const SUMMARY_QUERY: &str = "(UID FLAGS INTERNALDATE X-GM-MSGID X-GM-LABELS BODY.PEEK[HEADER])";

fn summary(f: &Fetch) -> Option<Summary> {
    let id = *f.gmail_msg_id()?;
    let header = f.header().unwrap_or_default();
    let msg = mail_parser::MessageParser::new().parse(header);
    let (from, from_addr) = msg
        .as_ref()
        .and_then(|m| m.from())
        .and_then(|l| l.first())
        .map(|a| {
            let addr = a.address().unwrap_or_default().to_string();
            (a.name().map(str::to_string).unwrap_or_else(|| addr.clone()), addr)
        })
        .unwrap_or_default();
    let subject = msg
        .as_ref()
        .and_then(|m| m.subject())
        .unwrap_or("")
        .trim()
        .to_string();
    let attach = msg
        .as_ref()
        .and_then(|m| m.content_type())
        .is_some_and(|c| c.ctype().eq_ignore_ascii_case("multipart") && c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("mixed")));
    let ts = f
        .internal_date()
        .map(|d| d.timestamp())
        .or_else(|| msg.as_ref().and_then(|m| m.date()).map(|d| d.to_timestamp()))
        .unwrap_or(0);
    let mut unread = true;
    let mut starred = false;
    for fl in f.flags() {
        match fl {
            Flag::Seen => unread = false,
            Flag::Flagged => starred = true,
            _ => {}
        }
    }
    let mut labels = Vec::new();
    let mut inbox = false;
    for l in f.gmail_labels().into_iter().flatten() {
        let l = l.as_ref();
        if l.eq_ignore_ascii_case("\\Inbox") {
            inbox = true;
        } else if !l.starts_with('\\') {
            labels.push(decode_mutf7(l));
        }
    }
    Some(Summary {
        id: id.to_string(),
        org: crate::plugins::web::org_domain(&from_addr),
        from,
        from_addr,
        subject,
        ts,
        unread,
        starred,
        attach,
        labels,
        inbox,
    })
}

async fn fetch_summaries(s: &mut Sess, set: &str, uid: bool) -> Result<Vec<Summary>, String> {
    let list: Vec<Fetch> = if uid {
        s.uid_fetch(set, SUMMARY_QUERY).await.map_err(e2s)?.try_collect().await.map_err(e2s)?
    } else {
        s.fetch(set, SUMMARY_QUERY).await.map_err(e2s)?.try_collect().await.map_err(e2s)?
    };
    drain(s);
    let mut out: Vec<Summary> = list.iter().filter_map(summary).collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.ts));
    Ok(out)
}

// ------------------------------------------------------------------ 收件匣監看

async fn watch_loop(acct: Account, shared: Shared) {
    let mut backoff = 5u64;
    loop {
        let started = std::time::Instant::now();
        let err = watch(&acct, &shared).await.err().unwrap_or_default();
        shared.set_error(&acct.name, &err);
        // 連線維持了一段時間才斷: 從頭計算等待時間
        if started.elapsed() > Duration::from_secs(300) {
            backoff = 5;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(300);
    }
}

async fn watch(acct: &Account, shared: &Shared) -> Result<(), String> {
    let mut s = connect(&acct.email, &acct.password).await?;
    let mb = s.examine("INBOX").await.map_err(e2s)?;
    let mut next = mb.uid_next.unwrap_or(1);
    refresh(&mut s, acct, shared, mb.exists).await?;
    loop {
        let mut idle = s.idle();
        idle.init().await.map_err(e2s)?;
        let r = {
            let (fut, _stop) = idle.wait_with_timeout(IDLE_RENEW);
            fut.await
        };
        s = idle.done().await.map_err(e2s)?;
        r.map_err(e2s)?;
        drain(&s);
        // 重新 EXAMINE 取得最新的數量與下一個 UID
        let mb = s.examine("INBOX").await.map_err(e2s)?;
        let uid_next = mb.uid_next.unwrap_or(next);
        if uid_next > next {
            notify(&mut s, acct, shared, next, uid_next).await?;
            next = uid_next;
        }
        refresh(&mut s, acct, shared, mb.exists).await?;
    }
}

/// 未讀數與最新幾封
async fn refresh(s: &mut Sess, acct: &Account, shared: &Shared, exists: u32) -> Result<(), String> {
    let unread = s.uid_search("UNSEEN").await.map_err(e2s)?.len();
    drain(s);
    let recent = if exists > 0 {
        let from = exists.saturating_sub(RECENT - 1).max(1);
        fetch_summaries(s, &format!("{from}:{exists}"), false).await?
    } else {
        Vec::new()
    };
    shared.set_inbox(&acct.name, unread, recent);
    Ok(())
}

/// 新信通知(依 mail.notify 篩選)
async fn notify(s: &mut Sess, acct: &Account, shared: &Shared, from: u32, to: u32) -> Result<(), String> {
    let filter = shared.notify_filter();
    if filter == "none" {
        return Ok(());
    }
    let range = format!("{from}:{}", to.saturating_sub(1).max(from));
    let found: Vec<u32> = if filter == "all" || filter.is_empty() {
        let r = s.uid_search(format!("UID {range} UNSEEN")).await.map_err(e2s)?;
        drain(s);
        r.into_iter().collect()
    } else {
        gm_search(s, &format!("UID {range} UNSEEN "), &filter).await?
    };
    // n:* 至少會回傳最後一封, 要再過濾
    let mut uids: Vec<u32> = found.into_iter().filter(|u| *u >= from).collect();
    if uids.is_empty() {
        return Ok(());
    }
    uids.sort_unstable();
    let more = uids.len().saturating_sub(NOTIFY_MAX);
    let set: Vec<String> = uids.iter().rev().take(NOTIFY_MAX).map(u32::to_string).collect();
    let list = fetch_summaries(s, &set.join(","), true).await?;
    for m in list {
        shared.push_notice(Notice {
            id: 0,
            account: acct.name.clone(),
            from: m.from.clone(),
            subject: m.subject.clone(),
            msg: m.id.clone(),
            ts: m.ts,
        });
    }
    if more > 0 {
        shared.log(false, format!("[{}] 另外還有 {more} 封新郵件", acct.name));
    }
    Ok(())
}

// ------------------------------------------------------------------ 網頁的要求

/// Gmail 系統資料夾的實際名稱(依語言不同, 例如「[Gmail]/所有郵件」)
#[derive(Default)]
struct Names {
    all: String,
    trash: String,
    junk: String,
    labels: Vec<Label>,
}

struct Command {
    acct: Account,
    sess: Option<Sess>,
    names: Names,
    /// 目前 SELECT 的資料夾
    selected: Option<String>,
    cache: VecDeque<(u64, Arc<Vec<u8>>)>,
    /// 「所有郵件」裡 信件 id → UID(搜尋時記下, 讀取時不用再找)
    uids: HashMap<u64, u32>,
}

async fn command_loop(acct: Account, mut rx: mpsc::UnboundedReceiver<Request>) {
    let mut c = Command {
        acct,
        sess: None,
        names: Names::default(),
        selected: None,
        cache: VecDeque::new(),
        uids: HashMap::new(),
    };
    while let Some(r) = rx.recv().await {
        let out = match timeout(JOB_TIMEOUT, c.handle(&r.job)).await {
            Ok(o) => o,
            Err(_) => {
                c.sess = None;
                Err("Gmail 沒有回應(逾時)".into())
            }
        };
        let _ = r.reply.send(out);
    }
}

impl Command {
    async fn handle(&mut self, job: &Job) -> Result<Out, String> {
        // 連線可能已經被伺服器關掉: 失敗時重新連線再試一次
        let mut last = String::new();
        for attempt in 0..2 {
            if self.sess.is_none() {
                self.open().await?;
            }
            match self.run(job).await {
                Ok(o) => return Ok(o),
                Err(e) => {
                    last = e;
                    if attempt == 0 {
                        self.sess = None;
                        self.selected = None;
                    }
                }
            }
        }
        Err(last)
    }

    async fn open(&mut self) -> Result<(), String> {
        let mut s = connect(&self.acct.email, &self.acct.password).await?;
        let names: Vec<_> = s
            .list(Some(""), Some("*"))
            .await
            .map_err(e2s)?
            .try_collect()
            .await
            .map_err(e2s)?;
        drain(&s);
        let mut n = Names::default();
        for name in &names {
            let attrs = name.attributes();
            if attrs.iter().any(|a| matches!(a, NameAttribute::NoSelect)) {
                continue;
            }
            let special = attrs.iter().find_map(|a| match a {
                NameAttribute::All => Some("all"),
                NameAttribute::Trash => Some("trash"),
                NameAttribute::Sent => Some("sent"),
                NameAttribute::Drafts => Some("drafts"),
                NameAttribute::Junk => Some("junk"),
                NameAttribute::Flagged => Some("flagged"),
                NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\Important") => Some("important"),
                _ => None,
            });
            match special {
                Some("all") => n.all = name.name().to_string(),
                Some("trash") => n.trash = name.name().to_string(),
                Some("junk") => n.junk = name.name().to_string(),
                _ => {}
            }
            n.labels.push(Label {
                name: name.name().to_string(),
                display: decode_mutf7(name.name()),
                special,
            });
        }
        n.labels.sort_by_key(|l| l.display.to_lowercase());
        if n.all.is_empty() {
            return Err("找不到「所有郵件」資料夾(請在 Gmail 設定裡開啟 IMAP 並顯示「所有郵件」)".into());
        }
        self.names = n;
        self.sess = Some(s);
        self.selected = None;
        self.uids.clear();
        Ok(())
    }

    fn sess(&mut self) -> Result<&mut Sess, String> {
        self.sess.as_mut().ok_or_else(|| "沒有連線".to_string())
    }

    async fn select(&mut self, mailbox: &str) -> Result<(), String> {
        if self.selected.as_deref() == Some(mailbox) {
            return Ok(());
        }
        let s = self.sess()?;
        s.select(mailbox).await.map_err(e2s)?;
        drain(s);
        self.selected = Some(mailbox.to_string());
        Ok(())
    }

    async fn run(&mut self, job: &Job) -> Result<Out, String> {
        match job {
            Job::Labels => Ok(Out::Labels(self.names.labels.clone())),
            Job::Search { q } => {
                let all = self.names.all.clone();
                self.select(&all).await?;
                let s = self.sess()?;
                let uids = gm_search(s, "", q.trim()).await?;
                let mut pairs: Vec<(u32, u64)> = Vec::with_capacity(uids.len());
                for chunk in uids.chunks(500) {
                    let set: Vec<String> = chunk.iter().map(u32::to_string).collect();
                    let list: Vec<Fetch> = s
                        .uid_fetch(set.join(","), "(UID X-GM-MSGID)")
                        .await
                        .map_err(e2s)?
                        .try_collect()
                        .await
                        .map_err(e2s)?;
                    drain(s);
                    for f in &list {
                        if let (Some(uid), Some(id)) = (f.uid, f.gmail_msg_id()) {
                            pairs.push((uid, *id));
                        }
                    }
                }
                // 新到舊(UID 越大越新)
                pairs.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
                for (uid, id) in &pairs {
                    self.uids.insert(*id, *uid);
                }
                let ids: Vec<u64> = pairs.into_iter().map(|(_, id)| id).collect();
                Ok(Out::Ids(ids))
            }
            Job::Peek { ids } => {
                let all = self.names.all.clone();
                let known: Vec<u32> = ids.iter().filter_map(|i| self.uids.get(i).copied()).collect();
                let mut groups: Vec<(String, Vec<u32>)> = Vec::new();
                if known.len() == ids.len() {
                    groups.push((all, known));
                } else {
                    groups = self.locate(ids).await?;
                }
                let mut out = Vec::new();
                for (mailbox, uids) in groups {
                    self.select(&mailbox).await?;
                    let s = self.sess()?;
                    let set: Vec<String> = uids.iter().map(u32::to_string).collect();
                    // BODY.PEEK: 不會標成已讀
                    let list: Vec<Fetch> = s
                        .uid_fetch(set.join(","), "(UID X-GM-MSGID INTERNALDATE BODY.PEEK[])")
                        .await
                        .map_err(e2s)?
                        .try_collect()
                        .await
                        .map_err(e2s)?;
                    drain(s);
                    for f in &list {
                        if let (Some(id), Some(body)) = (f.gmail_msg_id(), f.body()) {
                            let ts = f.internal_date().map(|d| d.timestamp()).unwrap_or(0);
                            out.push((*id, ts, Arc::new(body.to_vec())));
                        }
                    }
                }
                Ok(Out::Raws(out))
            }
            Job::List { mailbox, q, offset } => {
                let mailbox = match mailbox {
                    Some(m) if !m.is_empty() => m.clone(),
                    _ => self.names.all.clone(),
                };
                self.select(&mailbox).await?;
                let s = self.sess()?;
                let mut uids: Vec<u32> = if q.trim().is_empty() {
                    let r = s.uid_search("ALL").await.map_err(e2s)?;
                    drain(s);
                    r.into_iter().collect()
                } else {
                    gm_search(s, "", q.trim()).await?
                };
                // UID 越大越新
                uids.sort_unstable_by(|a, b| b.cmp(a));
                let total = uids.len();
                let set: Vec<String> = uids
                    .iter()
                    .skip(*offset)
                    .take(PAGE)
                    .map(u32::to_string)
                    .collect();
                let items = if set.is_empty() {
                    Vec::new()
                } else {
                    fetch_summaries(s, &set.join(","), true).await?
                };
                Ok(Out::List(ListPage {
                    total,
                    offset: *offset,
                    items,
                }))
            }
            Job::Raw { id } => {
                if let Some((_, raw)) = self.cache.iter().find(|(i, _)| i == id) {
                    return Ok(Out::Raw(raw.clone()));
                }
                let (mailbox, uids) = self
                    .locate(&[*id])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or("找不到這封信(可能已經刪除)")?;
                self.select(&mailbox).await?;
                let s = self.sess()?;
                // BODY[](不是 PEEK): 讀信同時標成已讀
                let list: Vec<Fetch> = s
                    .uid_fetch(uids[0].to_string(), "(UID BODY[])")
                    .await
                    .map_err(e2s)?
                    .try_collect()
                    .await
                    .map_err(e2s)?;
                drain(s);
                let raw = list
                    .iter()
                    .find_map(|f| f.body().map(<[u8]>::to_vec))
                    .ok_or("讀取失敗")?;
                let raw = Arc::new(raw);
                self.cache.push_front((*id, raw.clone()));
                self.cache.truncate(CACHE);
                Ok(Out::Raw(raw))
            }
            Job::Op { ids, op } => {
                for (mailbox, uids) in self.locate(ids).await? {
                    self.select(&mailbox).await?;
                    let in_all = mailbox == self.names.all;
                    let trash = self.names.trash.clone();
                    let set: Vec<String> = uids.iter().map(u32::to_string).collect();
                    let set = set.join(",");
                    let s = self.sess()?;
                    match op {
                        Op::Read => run_store(s, &set, "+FLAGS.SILENT (\\Seen)").await?,
                        Op::Unread => run_store(s, &set, "-FLAGS.SILENT (\\Seen)").await?,
                        Op::Star => run_store(s, &set, "+FLAGS.SILENT (\\Flagged)").await?,
                        Op::Unstar => run_store(s, &set, "-FLAGS.SILENT (\\Flagged)").await?,
                        Op::Archive if in_all => {
                            run_store(s, &set, "-X-GM-LABELS.SILENT (\\Inbox)").await?
                        }
                        Op::Archive => {}
                        Op::Inbox if in_all => {
                            run_store(s, &set, "+X-GM-LABELS.SILENT (\\Inbox)").await?
                        }
                        // 從垃圾桶或垃圾郵件救回
                        Op::Inbox => s.uid_mv(&set, "INBOX").await.map_err(e2s)?,
                        Op::Trash if mailbox == trash => {}
                        Op::Trash => s.uid_mv(&set, &trash).await.map_err(e2s)?,
                    }
                    drain(s);
                }
                // 狀態改了, 快取的內容不影響(內文一樣)
                Ok(Out::Done)
            }
        }
    }

    /// 找出信件在哪個資料夾、UID 多少: 先找「所有郵件」, 找不到再找垃圾桶與垃圾郵件
    async fn locate(&mut self, ids: &[u64]) -> Result<Vec<(String, Vec<u32>)>, String> {
        let mut out = Vec::new();
        let mut missing: Vec<u64> = ids.to_vec();
        let boxes = [
            self.names.all.clone(),
            self.names.trash.clone(),
            self.names.junk.clone(),
        ];
        for mailbox in boxes {
            if missing.is_empty() || mailbox.is_empty() {
                continue;
            }
            self.select(&mailbox).await?;
            let s = self.sess()?;
            // 每封一個條件, 用 OR 串起來
            let mut crit = format!("X-GM-MSGID {}", missing[0]);
            for id in &missing[1..] {
                crit = format!("OR X-GM-MSGID {id} {crit}");
            }
            let uids: Vec<u32> = s.uid_search(&crit).await.map_err(e2s)?.into_iter().collect();
            drain(s);
            if uids.is_empty() {
                continue;
            }
            // 哪些找到了: 再拿一次 MSGID
            let set: Vec<String> = uids.iter().map(u32::to_string).collect();
            let found: Vec<Fetch> = s
                .uid_fetch(set.join(","), "(UID X-GM-MSGID)")
                .await
                .map_err(e2s)?
                .try_collect()
                .await
                .map_err(e2s)?;
            drain(s);
            let found: Vec<u64> = found.iter().filter_map(|f| f.gmail_msg_id().copied()).collect();
            missing.retain(|id| !found.contains(id));
            out.push((mailbox, uids));
        }
        if out.is_empty() {
            return Err("找不到這封信(可能已經刪除)".into());
        }
        Ok(out)
    }
}

async fn run_store(s: &mut Sess, set: &str, q: &str) -> Result<(), String> {
    let _: Vec<Fetch> = s
        .uid_store(set, q)
        .await
        .map_err(e2s)?
        .try_collect()
        .await
        .map_err(e2s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("from:a b"), "\"from:a b\"");
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn mutf7() {
        // RFC 3501 的例子
        assert_eq!(decode_mutf7("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(decode_mutf7("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode_mutf7("INBOX"), "INBOX");
    }

    #[test]
    fn ops() {
        assert_eq!(Op::parse("trash"), Some(Op::Trash));
        assert_eq!(Op::parse("x"), None);
    }
}
