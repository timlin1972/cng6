//! Update plugin: 發現 Git 上有新的程式碼時, 自動下載、編譯, 倒數後重新啟動。
//!
//! - 背景定時 `git fetch`, 比較執行中的版本和 upstream(例如 origin/main)
//! - 有新版時先在背景 pull + 編譯(舊版照常執行), 編譯成功才倒數 5 秒重新啟動
//! - 工作目錄有沒 commit 的修改、`update.auto = notify`、或按過取消: 只提示, 不自動更新
//! - cluster 有傳輸進行中時延後(最多 10 分鐘)
//!
//! 重新啟動沿用 `restart`: 由 utils/cng6.sh 管理時它會再 pull + 編譯(已經是最新, 很快)。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::Serialize;

use crate::plugin::{Action, Ctx, Plugin, complete_from_help};
use crate::plugins::cluster;
use crate::webui::{Button, Card, Status, WebMeta};

const DEFAULT_INTERVAL: u64 = 300;
/// 開始執行後第一次檢查的時間(讓啟動時的其他工作先做完)
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// 編譯好之後倒數多久重新啟動
const COUNTDOWN: Duration = Duration::from_secs(5);
/// 有傳輸進行中時最多延後多久
const MAX_DEFER: Duration = Duration::from_secs(600);
/// git 指令最多等多久(秒)
const GIT_TIMEOUT: &str = "120";

/// 自動更新的方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Auto {
    /// 自動下載、編譯並重新啟動
    On,
    /// 只提示有新版
    Notify,
    /// 不檢查
    Off,
}

impl Auto {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "on" => Ok(Self::On),
            "notify" => Ok(Self::Notify),
            "off" => Ok(Self::Off),
            _ => Err("請輸入 on / notify / off".into()),
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::On => "自動更新",
            Self::Notify => "只提示",
            Self::Off => "關閉",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub title: String,
}

impl Commit {
    fn label(&self) -> String {
        format!("{} {}", self.short, self.title)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    /// 已是最新(或還沒檢查)
    Idle,
    Checking,
    /// 有新版, 等使用者決定(只提示 / 有修改 / 取消過)
    Available,
    Building,
    /// 編譯好了, 由主執行緒決定倒數或延後
    Ready,
    /// 有傳輸進行中, 從這個時間開始延後
    Deferred(Instant),
    /// 倒數到這個時間重新啟動
    Countdown(Instant),
    Failed(String),
    Restarting,
}

impl Phase {
    fn key(&self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::Available => "available",
            Phase::Building => "building",
            Phase::Ready => "ready",
            Phase::Deferred(_) => "deferred",
            Phase::Countdown(_) => "countdown",
            Phase::Failed(_) => "failed",
            Phase::Restarting => "restarting",
        }
    }

    /// 背景可以開始新的檢查
    fn can_check(&self) -> bool {
        matches!(self, Phase::Idle | Phase::Available | Phase::Failed(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Req {
    /// 檢查; manual 時連「已是最新」也回報
    Check { manual: bool },
    /// 不管設定, 立刻更新
    Now,
}

struct State {
    auto: Auto,
    interval: u64,
    /// 執行中的版本(啟動時的 HEAD)
    running: Option<Commit>,
    /// 啟動時工作目錄有沒 commit 的修改
    running_dirty: bool,
    /// upstream 最新的版本
    target: Option<Commit>,
    /// 還沒用到的 commit(新到舊)
    pending: Vec<Commit>,
    dirty: bool,
    phase: Phase,
    last_check: Option<i64>,
    error: Option<String>,
    /// 按過取消的版本: 不再自動更新, 等下一個新 commit
    cancelled: Option<String>,
    /// 已經提示過的版本(同一版只提示一次)
    announced: Option<String>,
    /// 編譯好就立刻重新啟動(update now)
    now: bool,
    request: Option<Req>,
    /// 給主執行緒輸出的訊息 (警告?, 文字)
    logs: Vec<(bool, String)>,
}

impl State {
    fn log(&mut self, warn: bool, text: impl Into<String>) {
        self.logs.push((warn, format!("update: {}", text.into())));
    }
}

/// 給網頁的資料
#[derive(Serialize)]
pub struct Snapshot {
    pub auto: Auto,
    pub phase: &'static str,
    /// 倒數剩下的毫秒(倒數中才有)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_ms: Option<u64>,
    pub running: Option<Commit>,
    pub running_dirty: bool,
    pub target: Option<Commit>,
    pub pending: Vec<Commit>,
    pub dirty: bool,
    pub last_check: Option<i64>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<Mutex<State>>);

impl Shared {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            auto: Auto::On,
            interval: DEFAULT_INTERVAL,
            running: None,
            running_dirty: false,
            target: None,
            pending: Vec::new(),
            dirty: false,
            phase: Phase::Idle,
            last_check: None,
            error: None,
            cancelled: None,
            announced: None,
            now: false,
            request: None,
            logs: Vec::new(),
        })))
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.0.lock().unwrap();
        Snapshot {
            auto: st.auto,
            phase: st.phase.key(),
            remaining_ms: match st.phase {
                Phase::Countdown(t) => {
                    Some(t.saturating_duration_since(Instant::now()).as_millis() as u64)
                }
                _ => None,
            },
            running: st.running.clone(),
            running_dirty: st.running_dirty,
            target: st.target.clone(),
            pending: st.pending.clone(),
            dirty: st.dirty,
            last_check: st.last_check,
            error: st.error.clone(),
            failed: match &st.phase {
                Phase::Failed(m) => Some(m.clone()),
                _ => None,
            },
        }
    }
}

/// 程式碼所在的 Git 工作目錄與編譯方式
#[derive(Clone)]
struct Repo {
    root: PathBuf,
    /// 編譯指令(測試時換掉)
    build: Vec<String>,
}

impl Repo {
    fn default_repo() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let build = vec![
            root.join("utils/build.sh").to_string_lossy().into_owned(),
            "build".into(),
        ];
        Self { root, build }
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("timeout")
            .arg(GIT_TIMEOUT)
            .arg("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            // 在背景執行, 不能停下來問帳號密碼; 網路不通時也不要卡住
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=15")
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("無法執行 git: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("失敗");
            Err(format!("git {}: {line}", args.first().unwrap_or(&"")))
        }
    }

    fn commit(&self, rev: &str) -> Result<Commit, String> {
        let s = self.git(&["log", "-1", "--format=%H%x1f%h%x1f%s", rev])?;
        parse_commits(&s)
            .into_iter()
            .next()
            .ok_or_else(|| format!("找不到 {rev}"))
    }

    /// 工作目錄有沒有已追蹤檔案的修改(未追蹤的檔案不算)
    fn dirty(&self) -> bool {
        self.git(&["status", "--porcelain", "--untracked-files=no"])
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    fn build(&self) -> Result<(), String> {
        let (cmd, args) = self.build.split_first().ok_or("沒有編譯指令")?;
        let out = Command::new(cmd)
            .args(args)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("無法執行 {cmd}: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        // 取第一個 error 那一行當摘要
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let line = text
            .lines()
            .find(|l| l.starts_with("error"))
            .or_else(|| text.lines().rfind(|l| !l.trim().is_empty()))
            .unwrap_or("編譯失敗");
        Err(format!("編譯失敗: {}", line.trim()))
    }
}

fn parse_commits(s: &str) -> Vec<Commit> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split('\x1f');
            Some(Commit {
                hash: it.next()?.to_string(),
                short: it.next()?.to_string(),
                title: it.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 背景執行緒: 定時檢查, 收到要求時立刻檢查
fn worker(repo: Repo, shared: Shared) {
    let mut next = Instant::now() + FIRST_CHECK;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let req = {
            let mut st = shared.0.lock().unwrap();
            if !st.phase.can_check() {
                None
            } else if let Some(r) = st.request.take() {
                Some(r)
            } else if st.auto != Auto::Off && Instant::now() >= next {
                Some(Req::Check { manual: false })
            } else {
                None
            }
        };
        if let Some(req) = req {
            run(&repo, &shared, req);
            let interval = shared.0.lock().unwrap().interval;
            next = Instant::now() + Duration::from_secs(interval);
        }
    }
}

/// 一次檢查; 需要時 pull + 編譯
fn run(repo: &Repo, shared: &Shared, req: Req) {
    let manual = !matches!(req, Req::Check { manual: false });
    let prev = {
        let mut st = shared.0.lock().unwrap();
        std::mem::replace(&mut st.phase, Phase::Checking)
    };
    let set = |f: &dyn Fn(&mut State)| f(&mut shared.0.lock().unwrap());

    let checked = (|| -> Result<(Commit, Commit, Vec<Commit>, bool, bool), String> {
        repo.git(&["fetch", "--quiet"])?;
        let head = repo.commit("HEAD")?;
        let up = repo
            .commit("@{upstream}")
            .map_err(|_| "目前的分支沒有設定 upstream".to_string())?;
        let can_ff = repo
            .git(&["merge-base", "--is-ancestor", "HEAD", "@{upstream}"])
            .is_ok();
        let running = shared.0.lock().unwrap().running.clone();
        let base = running.map_or(head.hash.clone(), |c| c.hash);
        let pending = parse_commits(
            &repo
                .git(&["log", "--format=%H%x1f%h%x1f%s", &format!("{base}..@{{upstream}}")])
                .unwrap_or_default(),
        );
        Ok((head, up, pending, can_ff, repo.dirty()))
    })();

    let (head, up, pending, can_ff, dirty) = match checked {
        Ok(v) => v,
        Err(e) => {
            set(&|st| {
                // 同樣的錯誤只記一次
                if manual || st.error.as_deref() != Some(&e) {
                    st.log(true, format!("檢查失敗: {e}"));
                }
                st.error = Some(e.clone());
                st.last_check = Some(now_ts());
                st.phase = if matches!(prev, Phase::Failed(_)) { Phase::Idle } else { prev.clone() };
            });
            return;
        }
    };

    let mut st = shared.0.lock().unwrap();
    st.last_check = Some(now_ts());
    st.error = None;
    st.target = Some(up.clone());
    st.pending = pending;
    st.dirty = dirty;
    let running = st.running.clone().unwrap_or_else(|| head.clone());
    if up.hash == running.hash {
        st.phase = Phase::Idle;
        st.cancelled = None;
        if manual {
            st.log(false, format!("已是最新版本 ({})", up.label()));
        }
        return;
    }
    if !can_ff {
        let msg = "本機有 upstream 沒有的 commit(或分支已分岔), 無法自動更新".to_string();
        if manual || st.announced.as_deref() != Some(&up.hash) {
            st.log(true, &msg);
            st.announced = Some(up.hash.clone());
        }
        st.phase = Phase::Failed(msg);
        return;
    }
    let reason = if req == Req::Now {
        None
    } else if st.auto != Auto::On {
        Some("自動更新已關閉(update.auto)")
    } else if dirty {
        Some("工作目錄有沒 commit 的修改, 不自動更新")
    } else if st.cancelled.as_deref() == Some(&up.hash) {
        Some("這一版已取消自動更新")
    } else {
        None
    };
    if let Some(reason) = reason {
        if manual || st.announced.as_deref() != Some(&up.hash) {
            let n = st.pending.len();
            st.log(
                true,
                format!("有新版 {} (新的 commit {n} 個); {reason}, 輸入 'update now' 更新", up.label()),
            );
            st.announced = Some(up.hash.clone());
        }
        st.phase = Phase::Available;
        return;
    }

    // 下載 + 編譯(舊版照常執行)
    st.phase = Phase::Building;
    st.announced = Some(up.hash.clone());
    if req == Req::Now {
        st.now = true;
    }
    st.log(false, format!("發現新版 {}, 下載並編譯中…", up.label()));
    drop(st);
    let result = if head.hash != up.hash {
        repo.git(&["merge", "--ff-only", "--quiet", "@{upstream}"])
    } else {
        Ok(String::new())
    }
    .and_then(|_| repo.build());
    let mut st = shared.0.lock().unwrap();
    match result {
        Ok(()) if st.cancelled.as_deref() == Some(&up.hash) => {
            st.log(false, format!("新版 {} 已編譯好, 但已取消; 輸入 'update now' 重新啟動", up.short));
            st.phase = Phase::Available;
        }
        Ok(()) => st.phase = Phase::Ready,
        Err(e) => {
            st.log(true, format!("{} {e}", up.short));
            st.now = false;
            st.phase = Phase::Failed(e);
        }
    }
}

pub struct Update {
    shared: Shared,
    cluster: cluster::Shared,
}

impl Update {
    pub fn new(shared: Shared, cluster: cluster::Shared) -> Self {
        Self::with_repo(shared, cluster, Repo::default_repo())
    }

    fn with_repo(shared: Shared, cluster: cluster::Shared, repo: Repo) -> Self {
        {
            let mut st = shared.0.lock().unwrap();
            st.running = repo.commit("HEAD").ok();
            st.running_dirty = repo.dirty();
        }
        if repo.root.join(".git").exists() || is_repo(&repo.root) {
            let s = shared.clone();
            std::thread::Builder::new()
                .name("update".into())
                .spawn(move || worker(repo, s))
                .ok();
        }
        Self { shared, cluster }
    }
}

fn is_repo(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-dir"])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn ago(ts: i64) -> String {
    format!("{} 前", cluster::human_duration(now_ts() - ts))
}

impl Plugin for Update {
    fn name(&self) -> &str {
        "update"
    }

    fn description(&self) -> &str {
        "自動更新: Git 上有新版時下載、編譯並重新啟動"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("status", "目前版本、upstream 版本與更新狀態"),
            ("check", "立刻檢查有沒有新版"),
            ("now", "立刻更新並重新啟動(不管 update.auto)"),
            ("cancel", "取消這次自動重新啟動(這一版不再自動更新)"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        complete_from_help(&self.help(), args)
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            (
                "auto",
                "on: 自動更新並重新啟動 / notify: 只提示 / off: 不檢查",
                "on".into(),
            ),
            (
                "interval",
                "檢查間隔(秒, 60~86400)",
                DEFAULT_INTERVAL.to_string(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        let mut st = self.shared.0.lock().unwrap();
        match key {
            "auto" => st.auto = Auto::parse(value)?,
            "interval" => {
                st.interval = crate::plugin::parse_usize(value, 60, 86400)? as u64;
            }
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            [] | ["status"] => {
                let s = self.shared.snapshot();
                let st = self.shared.0.lock().unwrap();
                let dirty = |d: bool| if d { " (有沒 commit 的修改)" } else { "" };
                ctx.info(format!(
                    "  執行中: {}{}",
                    s.running.as_ref().map_or("不明".into(), Commit::label),
                    dirty(s.running_dirty)
                ));
                ctx.info(format!(
                    "  upstream: {}",
                    s.target.as_ref().map_or("還沒檢查".into(), Commit::label)
                ));
                for c in s.pending.iter().take(10) {
                    ctx.info(format!("    + {}", c.label()));
                }
                ctx.info(format!(
                    "  狀態: {}  ·  {} · 每 {} 秒檢查{}",
                    phase_text(&st.phase),
                    st.auto.text(),
                    st.interval,
                    s.last_check.map_or(String::new(), |t| format!(", 上次 {}", ago(t)))
                ));
                if let Some(e) = &s.error {
                    ctx.warn(format!("  上次檢查失敗: {e}"));
                }
            }
            ["check"] => {
                let mut st = self.shared.0.lock().unwrap();
                if st.phase.can_check() {
                    st.request = Some(Req::Check { manual: true });
                    ctx.info("update: 檢查中…");
                } else {
                    ctx.info(format!("update: {}", phase_text(&st.phase)));
                }
            }
            ["now"] => {
                let mut st = self.shared.0.lock().unwrap();
                match st.phase {
                    Phase::Ready | Phase::Deferred(_) | Phase::Countdown(_) => {
                        st.phase = Phase::Countdown(Instant::now());
                    }
                    Phase::Checking | Phase::Building => {
                        st.now = true;
                        ctx.info("update: 處理中, 編譯好就重新啟動");
                    }
                    Phase::Restarting => {}
                    _ => {
                        st.request = Some(Req::Now);
                        st.cancelled = None;
                        ctx.info("update: 檢查並更新中…");
                    }
                }
            }
            ["cancel"] => {
                let mut st = self.shared.0.lock().unwrap();
                let target = st.target.as_ref().map(|c| c.hash.clone());
                match st.phase {
                    Phase::Ready | Phase::Deferred(_) | Phase::Countdown(_) => {
                        st.phase = Phase::Available;
                        st.cancelled = target;
                        st.now = false;
                        ctx.info("update: 已取消, 這一版不再自動更新(輸入 'update now' 更新)");
                    }
                    Phase::Building => {
                        st.cancelled = target;
                        st.now = false;
                        ctx.info("update: 已取消, 編譯完不會重新啟動");
                    }
                    _ => ctx.info("update: 沒有要取消的更新"),
                }
            }
            _ => ctx.error("用法: 輸入 'help update' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        let mut st = self.shared.0.lock().unwrap();
        for (warn, text) in std::mem::take(&mut st.logs) {
            if warn {
                ctx.warn(text);
            } else {
                ctx.info(text);
            }
        }
        let label = st.target.as_ref().map_or(String::new(), Commit::label);
        match st.phase {
            Phase::Ready => {
                if st.now {
                    st.phase = Phase::Countdown(Instant::now());
                } else if self.cluster.busy() {
                    st.phase = Phase::Deferred(Instant::now());
                    ctx.info(format!("update: 新版 {label} 已編譯好, 等 cluster 的傳輸結束再重新啟動"));
                } else {
                    st.phase = Phase::Countdown(Instant::now() + COUNTDOWN);
                    ctx.warn(format!(
                        "update: 新版 {label}, {} 秒後重新啟動(輸入 'update cancel' 取消)",
                        COUNTDOWN.as_secs()
                    ));
                }
            }
            Phase::Deferred(since) => {
                if !self.cluster.busy() || since.elapsed() > MAX_DEFER {
                    st.phase = Phase::Countdown(Instant::now() + COUNTDOWN);
                    ctx.warn(format!(
                        "update: 新版 {label}, {} 秒後重新啟動(輸入 'update cancel' 取消)",
                        COUNTDOWN.as_secs()
                    ));
                }
            }
            Phase::Countdown(t) if Instant::now() >= t => {
                st.phase = Phase::Restarting;
                ctx.warn(format!("update: 更新到 {label}, 重新啟動中…"));
                ctx.push(Action::Restart);
            }
            _ => {}
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("更新", "update", "green"))
    }

    fn web_card(&self) -> Option<Card> {
        let s = self.shared.snapshot();
        let st = self.shared.0.lock().unwrap();
        let (status, summary) = match &st.phase {
            Phase::Idle if s.error.is_some() => (Status::Warning, "無法檢查".to_string()),
            Phase::Idle if s.target.is_none() => (Status::Good, "還沒檢查".to_string()),
            Phase::Idle => (Status::Good, "已是最新版本".to_string()),
            Phase::Failed(_) => (Status::Critical, phase_text(&st.phase)),
            Phase::Available
            | Phase::Building
            | Phase::Ready
            | Phase::Deferred(_)
            | Phase::Countdown(_)
            | Phase::Restarting => (Status::Warning, phase_text(&st.phase)),
            Phase::Checking => (Status::Good, phase_text(&st.phase)),
        };
        let status = if st.auto == Auto::Off && status == Status::Good {
            Status::Off
        } else {
            status
        };
        let mut rows = vec![
            (
                "執行中".to_string(),
                s.running.as_ref().map_or("不明".into(), Commit::label)
                    + if s.running_dirty { "(有修改)" } else { "" },
            ),
            (
                "Git 最新".to_string(),
                s.target.as_ref().map_or("還沒檢查".into(), Commit::label),
            ),
            (
                "上次檢查".to_string(),
                s.last_check.map_or("—".into(), ago),
            ),
            (
                "自動更新".to_string(),
                format!("{} · 每 {}", st.auto.text(), cluster::human_duration(st.interval as i64)),
            ),
        ];
        if s.dirty {
            rows.push(("工作目錄".into(), "有沒 commit 的修改".into()));
        }
        let mut card = Card::new().status(status).summary(summary).table(rows);
        if let Some(e) = s.error.as_ref().or(s.failed.as_ref()) {
            card = card.text(e.clone(), Some(Status::Warning));
        }
        if !s.pending.is_empty() {
            let mut text = format!("新的 commit ({} 個):", s.pending.len());
            for c in s.pending.iter().take(8) {
                text.push_str(&format!("\n• {}", c.label()));
            }
            if s.pending.len() > 8 {
                text.push_str("\n…");
            }
            card = card.text(text, None);
        }
        let mut buttons = Vec::new();
        match st.phase {
            Phase::Countdown(_) | Phase::Deferred(_) | Phase::Ready => {
                buttons.push(Button::new("現在重新啟動", "/update now"));
                buttons.push(Button::new("取消", "/update cancel"));
            }
            Phase::Building => buttons.push(Button::new("取消", "/update cancel")),
            Phase::Checking | Phase::Restarting => {}
            _ => {
                buttons.push(Button::new("立即檢查", "/update check"));
                if !s.pending.is_empty() || st.phase == Phase::Available {
                    buttons.push(Button::new("立即更新", "/update now").confirm());
                }
            }
        }
        if !buttons.is_empty() {
            card = card.buttons(buttons);
        }
        Some(card)
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

fn phase_text(p: &Phase) -> String {
    match p {
        Phase::Idle => "已是最新版本".into(),
        Phase::Checking => "檢查中…".into(),
        Phase::Available => "有新版".into(),
        Phase::Building => "下載並編譯新版中…".into(),
        Phase::Ready => "新版已編譯好".into(),
        Phase::Deferred(_) => "新版已編譯好, 等傳輸結束".into(),
        Phase::Countdown(t) => format!(
            "{} 秒後重新啟動",
            t.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64
        ),
        Phase::Failed(m) => format!("更新失敗: {m}"),
        Phase::Restarting => "重新啟動中…".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, cmd: &str) {
        let ok = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(ok.status.success(), "{cmd}: {}", String::from_utf8_lossy(&ok.stderr));
    }

    /// 本機的 bare repo 當 upstream: 有新 commit 時 pull + 編譯, 倒數後要求重新啟動
    #[test]
    fn updates_from_upstream() {
        let tmp = std::env::temp_dir().join(format!("cng6-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        sh(&tmp, "git init -q --bare -b main origin.git && git clone -q origin.git a 2>/dev/null && cd a && git checkout -q -b main && echo 1 > f && git add f && git commit -qm one && git push -q -u origin main");
        sh(&tmp, "git clone -q origin.git b && cd b && echo 2 > f && git commit -qam two && git push -q");
        let repo = Repo {
            root: tmp.join("a"),
            build: vec!["sh".into(), "-c".into(), "touch built".into()],
        };
        let shared = Shared::new();
        let running = repo.commit("HEAD").unwrap();
        shared.0.lock().unwrap().running = Some(running.clone());

        // 只提示
        shared.0.lock().unwrap().auto = Auto::Notify;
        run(&repo, &shared, Req::Check { manual: false });
        {
            let st = shared.0.lock().unwrap();
            assert_eq!(st.phase, Phase::Available);
            assert_eq!(st.pending.len(), 1);
            assert_eq!(st.pending[0].title, "two");
            assert!(!repo.root.join("built").exists());
        }

        // 有沒 commit 的修改: 也只提示
        shared.0.lock().unwrap().auto = Auto::On;
        sh(&repo.root, "echo x > f");
        run(&repo, &shared, Req::Check { manual: false });
        assert_eq!(shared.0.lock().unwrap().phase, Phase::Available);
        sh(&repo.root, "git checkout -q f");

        // 自動: pull + 編譯
        run(&repo, &shared, Req::Check { manual: false });
        assert_eq!(shared.0.lock().unwrap().phase, Phase::Ready);
        assert!(repo.root.join("built").exists());
        assert_eq!(repo.commit("HEAD").unwrap().title, "two");

        // 主執行緒: 倒數, 時間到要求重新啟動
        let mut p = Update {
            shared: shared.clone(),
            cluster: cluster::Shared::new(),
        };
        let mut ctx = Ctx::new();
        p.on_tick(&mut ctx);
        assert!(matches!(shared.0.lock().unwrap().phase, Phase::Countdown(_)));
        assert_eq!(shared.snapshot().phase, "countdown");
        // 取消後這一版不再自動更新
        p.handle_command(&["cancel"], &mut ctx);
        assert_eq!(shared.0.lock().unwrap().phase, Phase::Available);
        run(&repo, &shared, Req::Check { manual: false });
        assert_eq!(shared.0.lock().unwrap().phase, Phase::Available);
        // update now: 立刻重新啟動
        p.handle_command(&["now"], &mut ctx);
        let req = shared.0.lock().unwrap().request.take().unwrap();
        run(&repo, &shared, req);
        p.on_tick(&mut ctx);
        p.on_tick(&mut ctx);
        assert_eq!(shared.0.lock().unwrap().phase, Phase::Restarting);
        assert!(ctx.take().iter().any(|a| matches!(a, Action::Restart)));

        // 已是最新
        shared.0.lock().unwrap().running = Some(repo.commit("HEAD").unwrap());
        shared.0.lock().unwrap().phase = Phase::Idle;
        run(&repo, &shared, Req::Check { manual: true });
        assert_eq!(shared.0.lock().unwrap().phase, Phase::Idle);
        assert!(shared.0.lock().unwrap().pending.is_empty());

        // 本機有 upstream 沒有的 commit: 不更新
        sh(&repo.root, "echo 3 > f && git commit -qam local");
        sh(&tmp.join("b"), "echo 4 > g && git add g && git commit -qm four && git push -q");
        run(&repo, &shared, Req::Check { manual: false });
        assert!(matches!(shared.0.lock().unwrap().phase, Phase::Failed(_)));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parse() {
        assert_eq!(Auto::parse("notify"), Ok(Auto::Notify));
        assert!(Auto::parse("x").is_err());
        let c = parse_commits("abc\x1fa\x1fhello world\ndef\x1fd\x1f");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].title, "hello world");
        assert_eq!(c[1].title, "");
    }
}
