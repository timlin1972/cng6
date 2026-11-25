//! Mail plugin: 用 IMAP 收 Gmail(只收信)。
//!
//! - 帳號用 Google 的「應用程式密碼」登入, 存在 `mail-accounts.json`(權限 600, 不進 Git)
//! - 帳號只能從網頁加入, 密碼不會出現在指令歷史或 `settings`
//! - 新信即時通知(IMAP IDLE): message 與網頁, 可用 `mail.notify` 篩選
//! - 別台機器透過 cluster 看這台的信箱, 密碼不會離開這台

pub mod imap;

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::plugin::{Ctx, Plugin, complete_from_help};
use crate::webui::{Button, Card, Status, WebMeta};
use imap::{Ctl, Job, Out, Request, Summary};

const ACCOUNTS_FILE: &str = "mail-accounts.json";
/// 外部圖片一律直接載入的寄件者
const TRUSTED_FILE: &str = "mail-trusted.json";
/// 保留最近幾則新信通知(給網頁)
const NOTICES: usize = 50;
/// 手機列表滑動的動作
const SWIPE_ACTIONS: [&str; 5] = ["none", "archive", "trash", "read", "star"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// 顯示用的名稱(也用在網址與指令)
    pub name: String,
    pub email: String,
    pub password: String,
}

/// 一個帳號的狀態
#[derive(Debug, Clone, Default, Serialize)]
pub struct AccountStatus {
    pub connected: bool,
    pub error: Option<String>,
    pub unread: usize,
    /// 收件匣最新幾封(新到舊)
    pub recent: Vec<Summary>,
}

/// 新信通知
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub id: u64,
    pub account: String,
    pub from: String,
    pub subject: String,
    /// 信件的 id(X-GM-MSGID)
    pub msg: String,
    pub ts: i64,
}

/// 給網頁的總覽
#[derive(Serialize)]
pub struct Overview {
    pub accounts: Vec<AccountView>,
    /// 最新一則通知的 id
    pub notice: u64,
    pub notify: String,
    pub swipe: Swipe,
}

/// 手機列表往左、往右滑的動作: none / archive / trash / read / star
#[derive(Debug, Clone, Serialize)]
pub struct Swipe {
    pub left: String,
    pub right: String,
}

#[derive(Serialize)]
pub struct AccountView {
    pub name: String,
    pub email: String,
    #[serde(flatten)]
    pub status: AccountStatus,
}

struct Inner {
    file: PathBuf,
    trusted_file: PathBuf,
    /// 外部圖片一律直接載入的寄件者(小寫 email)
    trusted: RwLock<std::collections::BTreeSet<String>>,
    accounts: RwLock<Vec<Account>>,
    status: RwLock<BTreeMap<String, AccountStatus>>,
    notices: Mutex<VecDeque<Notice>>,
    seq: AtomicU64,
    /// 給 TUI 的訊息 (警告?, 文字)
    logs: Mutex<Vec<(bool, String)>>,
    notify: RwLock<String>,
    swipe: RwLock<Swipe>,
    tx: OnceLock<mpsc::UnboundedSender<Ctl>>,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<Inner>);

impl Shared {
    pub fn new() -> Self {
        Self::with_file(PathBuf::from(ACCOUNTS_FILE))
    }

    fn with_file(file: PathBuf) -> Self {
        let accounts = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let trusted_file = file.with_file_name(TRUSTED_FILE);
        let trusted = std::fs::read(&trusted_file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self(Arc::new(Inner {
            file,
            trusted_file,
            trusted: RwLock::new(trusted),
            accounts: RwLock::new(accounts),
            status: RwLock::new(BTreeMap::new()),
            notices: Mutex::new(VecDeque::new()),
            seq: AtomicU64::new(0),
            logs: Mutex::new(Vec::new()),
            notify: RwLock::new("all".into()),
            swipe: RwLock::new(Swipe {
                left: "archive".into(),
                right: "trash".into(),
            }),
            tx: OnceLock::new(),
        }))
    }

    /// 啟動背景連線(有帳號時)
    fn start(&self) {
        if self.0.tx.get().is_none() {
            let _ = self.0.tx.set(imap::spawn(self.clone()));
        }
    }

    /// 這個寄件者的外部圖片是否一律直接載入
    pub fn is_trusted(&self, addr: &str) -> bool {
        !addr.is_empty() && self.0.trusted.read().unwrap().contains(&addr.trim().to_lowercase())
    }

    pub fn set_trusted(&self, addr: &str, trust: bool) -> Result<(), String> {
        let addr = addr.trim().to_lowercase();
        if !addr.contains('@') {
            return Err("寄件者地址錯誤".into());
        }
        let mut t = self.0.trusted.write().unwrap();
        if trust {
            t.insert(addr);
        } else {
            t.remove(&addr);
        }
        let data = serde_json::to_vec_pretty(&*t).map_err(|e| e.to_string())?;
        let tmp = self.0.trusted_file.with_extension("json.tmp");
        std::fs::write(&tmp, data)
            .and_then(|_| std::fs::rename(&tmp, &self.0.trusted_file))
            .map_err(|e| format!("無法寫入 {}: {e}", self.0.trusted_file.display()))
    }

    pub fn accounts(&self) -> Vec<Account> {
        self.0.accounts.read().unwrap().clone()
    }

    pub(crate) fn log(&self, warn: bool, text: impl Into<String>) {
        self.0.logs.lock().unwrap().push((warn, format!("mail: {}", text.into())));
    }

    fn clear_status(&self) {
        self.0.status.write().unwrap().clear();
    }

    fn set_error(&self, name: &str, err: &str) {
        let mut st = self.0.status.write().unwrap();
        let s = st.entry(name.to_string()).or_default();
        let was = s.connected;
        s.connected = false;
        // 同樣的錯誤只記一次
        if was || s.error.as_deref() != Some(err) {
            drop(st);
            self.log(true, format!("[{name}] 連線中斷: {err}, 稍後重試"));
            st = self.0.status.write().unwrap();
        }
        st.entry(name.to_string()).or_default().error = Some(err.to_string());
    }

    fn set_inbox(&self, name: &str, unread: usize, recent: Vec<Summary>) {
        let mut st = self.0.status.write().unwrap();
        let s = st.entry(name.to_string()).or_default();
        if !s.connected {
            let first = s.error.is_none();
            s.connected = true;
            s.error = None;
            drop(st);
            if first {
                self.log(false, format!("[{name}] 已連線, 未讀 {unread} 封"));
            } else {
                self.log(false, format!("[{name}] 已恢復連線"));
            }
            st = self.0.status.write().unwrap();
        }
        let s = st.entry(name.to_string()).or_default();
        s.unread = unread;
        s.recent = recent;
    }

    fn notify_filter(&self) -> String {
        self.0.notify.read().unwrap().clone()
    }

    fn push_notice(&self, mut n: Notice) {
        n.id = self.0.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let from = if n.from.is_empty() { "(沒有寄件者)" } else { &n.from };
        let subject = if n.subject.is_empty() { "(沒有主旨)" } else { &n.subject };
        self.log(false, format!("[{}] 新郵件: {from} — {subject}", n.account));
        let mut q = self.0.notices.lock().unwrap();
        q.push_back(n);
        while q.len() > NOTICES {
            q.pop_front();
        }
    }

    pub fn notices_after(&self, after: u64) -> (Vec<Notice>, u64) {
        let q = self.0.notices.lock().unwrap();
        (
            q.iter().filter(|n| n.id > after).cloned().collect(),
            self.0.seq.load(Ordering::Relaxed),
        )
    }

    pub fn overview(&self) -> Overview {
        let st = self.0.status.read().unwrap();
        Overview {
            accounts: self
                .accounts()
                .into_iter()
                .map(|a| AccountView {
                    status: st.get(&a.name).cloned().unwrap_or_default(),
                    name: a.name,
                    email: a.email,
                })
                .collect(),
            notice: self.0.seq.load(Ordering::Relaxed),
            notify: self.notify_filter(),
            swipe: self.0.swipe.read().unwrap().clone(),
        }
    }

    fn send(&self, c: Ctl) -> Result<(), String> {
        self.start();
        self.0
            .tx
            .get()
            .ok_or("mail 沒有在執行")?
            .send(c)
            .map_err(|_| "mail 沒有在執行".to_string())
    }

    /// 送一個要求給 IMAP thread, 等結果
    pub async fn request(&self, account: &str, job: Job) -> Result<Out, String> {
        let (reply, rx) = oneshot::channel();
        self.send(Ctl::Req(Request {
            account: account.to_string(),
            job,
            reply,
        }))?;
        match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("要求被中斷".into()),
            Err(_) => Err("Gmail 沒有回應(逾時)".into()),
        }
    }

    /// 新增(或更新同名的)帳號: 先試登入, 成功才存檔
    pub async fn add_account(&self, name: &str, email: &str, password: &str) -> Result<(), String> {
        let name = name.trim();
        let email = email.trim();
        if name.is_empty() || name.contains(['/', '\\', '?', '#', '%']) {
            return Err("名稱不能是空的, 也不能有 / \\ ? # %".into());
        }
        if !email.contains('@') {
            return Err("請輸入完整的 Gmail 地址".into());
        }
        let password: String = password.chars().filter(|c| !c.is_whitespace()).collect();
        if password.is_empty() {
            return Err("請輸入應用程式密碼".into());
        }
        let (reply, rx) = oneshot::channel();
        self.send(Ctl::Test {
            email: email.to_string(),
            password: password.clone(),
            reply,
        })?;
        rx.await.map_err(|_| "要求被中斷".to_string())??;
        {
            let mut list = self.0.accounts.write().unwrap();
            let acct = Account {
                name: name.to_string(),
                email: email.to_string(),
                password,
            };
            match list.iter_mut().find(|a| a.name == name) {
                Some(a) => *a = acct,
                None => list.push(acct),
            }
            self.save(&list)?;
        }
        self.log(false, format!("已加入帳號 {name} ({email})"));
        self.send(Ctl::Reload)
    }

    pub fn remove_account(&self, name: &str) -> Result<(), String> {
        {
            let mut list = self.0.accounts.write().unwrap();
            let before = list.len();
            list.retain(|a| a.name != name);
            if list.len() == before {
                return Err(format!("沒有帳號 '{name}'"));
            }
            self.save(&list)?;
        }
        self.log(false, format!("已移除帳號 {name}"));
        self.send(Ctl::Reload)
    }

    /// 存檔: 只有自己能讀(權限 600)
    fn save(&self, list: &[Account]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = self.0.file.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("無法寫入 {}: {e}", tmp.display()))?;
        f.write_all(&data).map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, &self.0.file).map_err(|e| e.to_string())
    }
}

pub struct Mail {
    shared: Shared,
}

impl Mail {
    pub fn new(shared: Shared) -> Self {
        // 舊檔案的權限可能太寬: 修正成 600
        if shared.0.file.exists() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&shared.0.file, std::fs::Permissions::from_mode(0o600));
        }
        if !shared.accounts().is_empty() {
            shared.start();
        }
        Self { shared }
    }
}

fn ago(ts: i64) -> String {
    let d = chrono::Utc::now().timestamp() - ts;
    if d < 60 {
        "剛剛".into()
    } else {
        format!("{} 前", crate::plugins::cluster::human_duration(d))
    }
}

fn line(m: &Summary) -> String {
    format!(
        "{}{} — {}",
        if m.unread { "● " } else { "" },
        if m.from.is_empty() { "(沒有寄件者)" } else { &m.from },
        if m.subject.is_empty() { "(沒有主旨)" } else { &m.subject }
    )
}

impl Plugin for Mail {
    fn name(&self) -> &str {
        "mail"
    }

    fn description(&self) -> &str {
        "Gmail: 收信、新信通知(帳號從網頁加入)"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("status", "各帳號的連線狀態與未讀數"),
            ("list [帳號]", "收件匣最新幾封"),
            ("reconnect", "全部重新連線"),
            ("account list", "列出帳號"),
            ("account remove <名稱>", "移除帳號(加入帳號請用網頁的「郵件」)"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["list"] | ["account", "remove"] => self
                .shared
                .accounts()
                .into_iter()
                .map(|a| (a.name, a.email))
                .collect(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            (
                "notify",
                "新信通知: all 全部 / none 不通知 / Gmail 搜尋條件(例如 is:important、from:boss@example.com)",
                "all".into(),
            ),
            (
                "swipe_left",
                "手機列表往左滑: none 不動作 / archive 封存 / trash 刪除 / read 已讀未讀 / star 星號",
                "archive".into(),
            ),
            (
                "swipe_right",
                "手機列表往右滑: none 不動作 / archive 封存 / trash 刪除 / read 已讀未讀 / star 星號",
                "trash".into(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "notify" => {
                let v = value.trim();
                *self.shared.0.notify.write().unwrap() = if v.is_empty() { "all".into() } else { v.to_string() };
                Ok(())
            }
            "swipe_left" | "swipe_right" => {
                if !SWIPE_ACTIONS.contains(&value) {
                    return Err(format!("請輸入 {}", SWIPE_ACTIONS.join(" / ")));
                }
                let mut sw = self.shared.0.swipe.write().unwrap();
                if key == "swipe_left" {
                    sw.left = value.to_string();
                } else {
                    sw.right = value.to_string();
                }
                Ok(())
            }
            _ => Err(format!("不支援的設定 '{key}'")),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let ov = self.shared.overview();
        match args {
            [] | ["status"] => {
                if ov.accounts.is_empty() {
                    ctx.info("  還沒有帳號: 在網頁開啟「郵件」加入 Gmail 帳號");
                }
                for a in &ov.accounts {
                    let state = if a.status.connected {
                        format!("已連線, 未讀 {}", a.status.unread)
                    } else if let Some(e) = &a.status.error {
                        format!("沒有連線: {e}")
                    } else {
                        "連線中…".into()
                    };
                    ctx.info(format!("  {} ({})  {state}", a.name, a.email));
                }
                ctx.info(format!("  新信通知: {}", ov.notify));
            }
            ["list"] | ["list", _] => {
                let want = args.get(1).copied();
                let mut found = false;
                for a in ov.accounts.iter().filter(|a| want.is_none_or(|w| w == a.name)) {
                    found = true;
                    ctx.info(format!("[{}] 未讀 {}", a.name, a.status.unread));
                    for m in &a.status.recent {
                        ctx.info(format!("  {}  {}", ago(m.ts), line(m)));
                    }
                }
                if !found {
                    ctx.error(match want {
                        Some(w) => format!("沒有帳號 '{w}'"),
                        None => "還沒有帳號: 在網頁開啟「郵件」加入".into(),
                    });
                }
            }
            ["reconnect"] => match self.shared.send(Ctl::Reload) {
                Ok(()) => ctx.info("mail: 重新連線中…"),
                Err(e) => ctx.error(e),
            },
            ["account", "list"] | ["account"] => {
                for a in self.shared.accounts() {
                    ctx.info(format!("  {}  {}", a.name, a.email));
                }
            }
            ["account", "remove", name] => match self.shared.remove_account(name) {
                Ok(()) => {}
                Err(e) => ctx.error(e),
            },
            ["account", "add", ..] => {
                ctx.error("為了不讓密碼留在指令歷史裡, 請在網頁開啟「郵件」加入帳號")
            }
            _ => ctx.error("用法: 輸入 'help mail' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        let logs = std::mem::take(&mut *self.shared.0.logs.lock().unwrap());
        for (warn, text) in logs {
            if warn {
                ctx.warn(text);
            } else {
                ctx.info(text);
            }
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("郵件", "mail", "red"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("mail")
    }

    fn web_card(&self) -> Option<Card> {
        let ov = self.shared.overview();
        if ov.accounts.is_empty() {
            return Some(
                Card::new()
                    .status(Status::Off)
                    .summary("還沒有帳號")
                    .text("開啟「郵件」加入 Gmail 帳號(需要 Google 的應用程式密碼)", None),
            );
        }
        let unread: usize = ov.accounts.iter().map(|a| a.status.unread).sum();
        let bad = ov.accounts.iter().filter(|a| a.status.error.is_some()).count();
        let mut card = Card::new()
            .status(if bad > 0 { Status::Warning } else { Status::Good })
            .summary(if unread > 0 { format!("未讀 {unread} 封") } else { "沒有未讀".into() });
        for a in &ov.accounts {
            card = card.stat(
                a.name.clone(),
                a.status.unread.to_string(),
                Some("封未讀"),
                Some(if a.status.error.is_some() { Status::Warning } else { Status::Good }),
            );
            if let Some(e) = &a.status.error {
                card = card.hint(e.clone());
            } else {
                card = card.hint(a.email.clone());
            }
        }
        let rows: Vec<(String, String)> = ov
            .accounts
            .iter()
            .flat_map(|a| a.status.recent.iter().take(3))
            .map(|m| (ago(m.ts), line(m)))
            .collect();
        if !rows.is_empty() {
            card = card.table(rows);
        }
        Some(card.buttons(vec![Button::new("重新連線", "/mail reconnect")]))
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_file() {
        let dir = std::env::temp_dir().join(format!("cng6-mail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.json");
        let s = Shared::with_file(file.clone());
        let list = vec![Account {
            name: "個人".into(),
            email: "a@gmail.com".into(),
            password: "abcd".into(),
        }];
        s.save(&list).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let s2 = Shared::with_file(file);
        assert_eq!(s2.accounts()[0].email, "a@gmail.com");
        // 網頁的總覽不能有密碼
        let json = serde_json::to_string(&s2.overview()).unwrap();
        assert!(!json.contains("abcd"));
        // 外部圖片一律載入的寄件者: 不分大小寫, 會存檔
        s2.set_trusted("News@Example.com", true).unwrap();
        assert!(Shared::with_file(dir.join("a.json")).is_trusted("news@example.com"));
        s2.set_trusted("news@example.com", false).unwrap();
        assert!(!Shared::with_file(dir.join("a.json")).is_trusted("news@example.com"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notices() {
        let s = Shared::with_file(std::env::temp_dir().join("cng6-mail-none.json"));
        for i in 0..3 {
            s.push_notice(Notice {
                id: 0,
                account: "a".into(),
                from: "x".into(),
                subject: format!("s{i}"),
                msg: "1".into(),
                ts: 0,
            });
        }
        let (list, last) = s.notices_after(1);
        assert_eq!(last, 3);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].subject, "s1");
    }
}
