//! Terminal plugin: 網頁上的終端機(真的 shell), 也可以透過 cluster 開別台的。
//!
//! - 每個終端機是一個 PTY + login shell, 以執行 cng6 的使用者身分執行
//! - 網頁用一般的 HTTP 要求溝通(長輪詢讀輸出、POST 送輸入), 所以 cluster 代理不用改就能遠端使用
//! - 關掉網頁不會結束終端機(像 tmux), 之後可以接回去; 沒人看超過 terminal.idle 分鐘才結束
//! - terminal.enabled = false 可以關閉這個功能(網頁就開不了 shell)

pub mod pty;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::Serialize;

use crate::plugin::{Ctx, Plugin, complete_from_help, parse_bool, parse_usize};
use crate::webui::WebMeta;

/// 每個終端機保留的輸出(重新接上時可以看到之前的畫面)
const MAX_BUF: usize = 512 * 1024;
/// 一次回應最多給多少輸出
const MAX_CHUNK: usize = 256 * 1024;
/// 最多同時幾個終端機
const MAX_SESSIONS: usize = 16;
/// 結束的終端機保留多久(讓網頁看到最後的輸出)
const ENDED_KEEP: Duration = Duration::from_secs(120);

struct Out {
    buf: VecDeque<u8>,
    /// buf[0] 的序號(從開始到現在的第幾個位元組)
    start: u64,
}

/// 輸入的順序: 網頁不等上一個送完就送下一個(遠端時快很多), 請求可能不照順序到,
/// 依序號寫進 shell
#[derive(Default)]
struct InputOrder {
    /// 送輸入的網頁(換人時重新開始算)
    client: String,
    next: u64,
    pending: BTreeMap<u64, Vec<u8>>,
}

/// 等太多還沒到的輸入: 放棄缺的那個, 繼續往下
const MAX_PENDING_INPUT: usize = 64;

pub struct Session {
    pub id: u32,
    input: Mutex<InputOrder>,
    pub created: i64,
    master: File,
    pid: u32,
    out: Mutex<Out>,
    notify: tokio::sync::Notify,
    ended: AtomicBool,
    last_seen: Mutex<Instant>,
    size: Mutex<(u16, u16)>,
}

/// 一次讀取的結果
pub struct Chunk {
    pub data: Vec<u8>,
    pub next: u64,
    pub ended: bool,
}

impl Session {
    fn touch(&self) {
        *self.last_seen.lock().unwrap() = Instant::now();
    }

    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// 從序號 from 開始的輸出(太舊的已經丟掉: 從還留著的開始)
    fn read_from(&self, from: u64) -> Chunk {
        let out = self.out.lock().unwrap();
        let end = out.start + out.buf.len() as u64;
        let from = from.clamp(out.start, end);
        let skip = (from - out.start) as usize;
        let data: Vec<u8> = out.buf.iter().skip(skip).take(MAX_CHUNK).copied().collect();
        Chunk {
            next: from + data.len() as u64,
            data,
            ended: self.ended(),
        }
    }

    /// 等到有新的輸出(或結束、逾時)再回傳
    pub async fn wait_output(&self, from: u64, wait: Duration) -> Chunk {
        self.touch();
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let c = self.read_from(from);
            if !c.data.is_empty() || c.ended {
                return c;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.read_from(from);
            }
            // 稍等一下, 把連續的輸出合在一起送
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
    }

    pub fn write_input(&self, data: &[u8]) -> std::io::Result<()> {
        self.touch();
        (&self.master).write_all(data)
    }

    /// 依序號寫入輸入(同一個網頁的序號從任意數字開始, 每次加一)
    pub fn write_ordered(&self, client: &str, seq: u64, data: Vec<u8>) -> std::io::Result<()> {
        self.touch();
        let mut o = self.input.lock().unwrap();
        if o.client != client {
            o.client = client.to_string();
            o.next = seq;
            o.pending.clear();
        }
        // 重送的(已經寫過了)
        if seq < o.next {
            return Ok(());
        }
        o.pending.insert(seq, data);
        if o.pending.len() > MAX_PENDING_INPUT
            && let Some(&first) = o.pending.keys().next()
        {
            o.next = first;
        }
        loop {
            let next = o.next;
            let Some(d) = o.pending.remove(&next) else {
                break;
            };
            o.next += 1;
            (&self.master).write_all(&d)?;
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        *self.size.lock().unwrap() = (cols, rows);
        pty::resize(&self.master, cols, rows)
    }

    pub fn kill(&self) {
        pty::hangup(self.pid);
    }
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: u32,
    pub created: i64,
    pub ended: bool,
    pub cols: u16,
    pub rows: u16,
    /// 最後一次有人看(秒前)
    pub idle: u64,
}

struct Inner {
    sessions: Mutex<HashMap<u32, Arc<Session>>>,
    next: AtomicU32,
    enabled: AtomicBool,
    shell: RwLock<String>,
    /// 沒人看多久就結束(分鐘, 0 表示不會)
    idle: AtomicU32,
    logs: Mutex<Vec<String>>,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<Inner>);

impl Shared {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            sessions: Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
            enabled: AtomicBool::new(true),
            shell: RwLock::new(String::new()),
            idle: AtomicU32::new(60),
            logs: Mutex::new(Vec::new()),
        }))
    }

    pub fn enabled(&self) -> bool {
        self.0.enabled.load(Ordering::Relaxed)
    }

    fn shell(&self) -> String {
        let s = self.0.shell.read().unwrap().clone();
        if !s.is_empty() {
            return s;
        }
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/bash".into())
    }

    fn log(&self, text: String) {
        self.0.logs.lock().unwrap().push(text);
    }

    /// 開一個新的終端機; who 是誰開的(紀錄用)
    pub fn open(&self, cols: u16, rows: u16, who: &str) -> Result<u32, String> {
        if !self.enabled() {
            return Err("終端機功能已關閉(terminal.enabled)".into());
        }
        self.cleanup();
        if self.0.sessions.lock().unwrap().len() >= MAX_SESSIONS {
            return Err(format!("最多同時 {MAX_SESSIONS} 個終端機"));
        }
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| "/".into());
        let shell = self.shell();
        let pty = pty::spawn(&shell, cols, rows, &home).map_err(|e| format!("無法開啟 {shell}: {e}"))?;
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let reader = pty.master.try_clone().map_err(|e| e.to_string())?;
        let pid = pty.child.id();
        let s = Arc::new(Session {
            id,
            input: Mutex::new(InputOrder::default()),
            created: chrono::Utc::now().timestamp(),
            master: pty.master,
            pid,
            out: Mutex::new(Out {
                buf: VecDeque::new(),
                start: 0,
            }),
            notify: tokio::sync::Notify::new(),
            ended: AtomicBool::new(false),
            last_seen: Mutex::new(Instant::now()),
            size: Mutex::new((cols, rows)),
        });
        self.0.sessions.lock().unwrap().insert(id, s.clone());
        // 讀 shell 輸出的 thread: shell 結束時讀到錯誤
        let shared = self.clone();
        let mut child = pty.child;
        std::thread::Builder::new()
            .name(format!("term-{id}"))
            .spawn(move || {
                read_loop(reader, &s);
                s.ended.store(true, Ordering::Relaxed);
                s.notify.notify_waiters();
                let code = child.wait().ok().and_then(|st| st.code());
                shared.log(format!(
                    "terminal: #{id} 已結束{}",
                    code.map_or(String::new(), |c| format!(" (結束碼 {c})"))
                ));
            })
            .map_err(|e| e.to_string())?;
        self.log(format!("terminal: #{id} 已開啟 ({shell}, {who})"));
        Ok(id)
    }

    pub fn get(&self, id: u32) -> Option<Arc<Session>> {
        self.0.sessions.lock().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.cleanup();
        let mut v: Vec<SessionInfo> = self
            .0
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| {
                let (cols, rows) = *s.size.lock().unwrap();
                SessionInfo {
                    id: s.id,
                    created: s.created,
                    ended: s.ended(),
                    cols,
                    rows,
                    idle: s.last_seen.lock().unwrap().elapsed().as_secs(),
                }
            })
            .collect();
        v.sort_by_key(|s| s.id);
        v
    }

    pub fn close(&self, id: u32) -> bool {
        match self.0.sessions.lock().unwrap().remove(&id) {
            Some(s) => {
                s.kill();
                true
            }
            None => false,
        }
    }

    /// 清掉結束很久的, 結束沒人看太久的
    fn cleanup(&self) {
        let idle = self.0.idle.load(Ordering::Relaxed) as u64 * 60;
        let mut gone = Vec::new();
        self.0.sessions.lock().unwrap().retain(|id, s| {
            let seen = s.last_seen.lock().unwrap().elapsed();
            if s.ended() && seen > ENDED_KEEP {
                return false;
            }
            if !s.ended() && idle > 0 && seen.as_secs() > idle {
                s.kill();
                gone.push(*id);
                return false;
            }
            true
        });
        for id in gone {
            self.log(format!("terminal: #{id} 沒人使用太久, 已結束"));
        }
    }

    fn close_all(&self) {
        for (_, s) in self.0.sessions.lock().unwrap().drain() {
            s.kill();
        }
    }
}

fn read_loop(mut reader: File, s: &Session) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                {
                    let mut out = s.out.lock().unwrap();
                    out.buf.extend(&buf[..n]);
                    let over = out.buf.len().saturating_sub(MAX_BUF);
                    if over > 0 {
                        out.buf.drain(..over);
                        out.start += over as u64;
                    }
                }
                s.notify.notify_waiters();
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // shell 結束時 PTY 會回 EIO
            Err(_) => break,
        }
    }
}

pub struct Terminal {
    shared: Shared,
    last_cleanup: Instant,
}

impl Terminal {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            last_cleanup: Instant::now(),
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // cng6 結束(或 restart)時一起結束
        self.shared.close_all();
    }
}

impl Plugin for Terminal {
    fn name(&self) -> &str {
        "terminal"
    }

    fn description(&self) -> &str {
        "終端機: 在網頁上開 shell(也可以透過 cluster 開別台的)"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("list", "列出開著的終端機"),
            ("kill <id>", "結束一個終端機"),
            ("kill all", "結束全部"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["kill"] => {
                let mut v: Vec<(String, String)> = self
                    .shared
                    .list()
                    .into_iter()
                    .map(|s| (s.id.to_string(), String::new()))
                    .collect();
                v.push(("all".into(), "結束全部".into()));
                v
            }
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            ("enabled", "允許從網頁開終端機(同一個 cluster 的機器也能開)", "true".into()),
            ("shell", "使用的 shell, 空的表示用 $SHELL(沒有時 /bin/bash)", String::new()),
            ("idle", "終端機沒人看多久就結束(分鐘, 0 表示不會)", "60".into()),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "enabled" => {
                let on = parse_bool(value)?;
                self.shared.0.enabled.store(on, Ordering::Relaxed);
                if !on {
                    self.shared.close_all();
                }
            }
            "shell" => *self.shared.0.shell.write().unwrap() = value.trim().to_string(),
            "idle" => self.shared.0.idle.store(parse_usize(value, 0, 7 * 24 * 60)? as u32, Ordering::Relaxed),
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            [] | ["list"] => {
                let list = self.shared.list();
                if list.is_empty() {
                    ctx.info("  沒有開著的終端機");
                }
                for s in list {
                    ctx.info(format!(
                        "  #{}  {}x{}  {}  {} 秒前有人用",
                        s.id,
                        s.cols,
                        s.rows,
                        if s.ended { "已結束" } else { "執行中" },
                        s.idle
                    ));
                }
            }
            ["kill", "all"] => {
                self.shared.close_all();
                ctx.info("terminal: 已全部結束");
            }
            ["kill", id] => match id.trim_start_matches('#').parse::<u32>() {
                Ok(n) if self.shared.close(n) => ctx.info(format!("terminal: #{n} 已結束")),
                _ => ctx.error(format!("沒有終端機 '{id}'")),
            },
            _ => ctx.error("用法: 輸入 'help terminal' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        for line in std::mem::take(&mut *self.shared.0.logs.lock().unwrap()) {
            ctx.info(line);
        }
        if self.last_cleanup.elapsed() > Duration::from_secs(30) {
            self.last_cleanup = Instant::now();
            self.shared.cleanup();
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("終端機", "terminal", "dark"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("terminal")
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 開一個 shell, 執行指令, 讀到輸出, 結束
    #[tokio::test]
    async fn shell_roundtrip() {
        let t = Shared::new();
        *t.0.shell.write().unwrap() = "/bin/sh".into();
        let id = t.open(80, 24, "test").unwrap();
        let s = t.get(id).unwrap();
        s.write_input(b"echo cng6-$((40+2))\n").unwrap();
        let mut from = 0;
        let mut seen = String::new();
        for _ in 0..50 {
            let c = s.wait_output(from, Duration::from_millis(200)).await;
            from = c.next;
            seen.push_str(&String::from_utf8_lossy(&c.data));
            if seen.contains("cng6-42") {
                break;
            }
        }
        assert!(seen.contains("cng6-42"), "輸出: {seen:?}");
        // 依序號: 晚到的先放著, 前面的到了一起寫入
        s.write_ordered("t", 10, b"echo ord-$((1+".to_vec()).unwrap();
        s.write_ordered("t", 12, b"\n".to_vec()).unwrap();
        s.write_ordered("t", 11, b"2))".to_vec()).unwrap();
        s.write_ordered("t", 11, b"XX".to_vec()).unwrap(); // 重送: 忽略
        let mut seen2 = String::new();
        for _ in 0..50 {
            let c = s.wait_output(from, Duration::from_millis(200)).await;
            from = c.next;
            seen2.push_str(&String::from_utf8_lossy(&c.data));
            if seen2.contains("ord-3") {
                break;
            }
        }
        assert!(seen2.contains("ord-3") && !seen2.contains("XX"), "輸出: {seen2:?}");
        s.resize(100, 30).unwrap();
        s.write_input(b"exit\n").unwrap();
        for _ in 0..50 {
            let c = s.wait_output(from, Duration::from_millis(200)).await;
            from = c.next;
            if c.ended {
                break;
            }
        }
        assert!(s.ended());
        // 重新從頭讀: 之前的輸出還在
        assert!(String::from_utf8_lossy(&s.read_from(0).data).contains("cng6-42"));
        assert!(t.close(id));
    }
}
