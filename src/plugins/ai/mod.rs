//! AI plugin: 用 Google Gemini 的 AI 助理。
//!
//! - 網頁的「AI」App 聊天(可以貼圖片), 郵件看信時可以摘要、翻譯; TUI 用 `ai ask`
//! - AI 可以呼叫 cng6 的功能: 查郵件、待辦、發票、天氣、cluster 狀態(見 tools.rs)
//! - API key 只放在一台(ai-key, 權限 600); `ai.node` 設定(cluster 同步)記錄是哪台,
//!   其他機器的網頁與指令都經由 cluster 轉給那台
//! - 對話存在 ai-chats.json(權限 600), 手機和電腦看到同一份
//! - 回答在背景產生, 網頁用 long-poll 一邊收一邊顯示

pub mod gemini;
pub mod tools;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::plugin::{Ctx, Plugin};
use crate::webui::{Card, Status, WebMeta};

const CHATS_FILE: &str = "ai-chats.json";
const KEY_FILE: &str = "ai-key";
/// 自動用最新的 Flash 模型(便宜、快)
pub const DEFAULT_MODEL: &str = "gemini-flash-latest";
const MAX_CHATS: usize = 200;
/// 一次回答最多呼叫幾輪工具
const MAX_ROUNDS: usize = 8;
/// 一則訊息最多幾張圖片、每張多大(base64 前)
pub const MAX_IMAGES: usize = 4;
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub created: i64,
    pub updated: i64,
    /// Gemini 的 contents, 原樣保存(下一輪原樣送回去)
    pub contents: Vec<Value>,
    /// 顯示用的文字: 第幾個 content → 顯示的內容(例如郵件摘要只顯示「摘要這封信」, 不顯示整封信)
    #[serde(default)]
    pub display: BTreeMap<usize, String>,
}

/// 正在產生的回答
#[derive(Default)]
struct Run {
    running: bool,
    partial: String,
    status: String,
    error: Option<String>,
    stop: Arc<AtomicBool>,
}

/// 新訊息附的圖片
#[derive(Deserialize, Clone)]
pub struct Image {
    pub mime: String,
    /// base64
    pub data: String,
}

#[derive(Serialize)]
pub struct ChatMeta {
    pub id: String,
    pub title: String,
    pub updated: i64,
    pub running: bool,
}

#[derive(Serialize)]
pub struct MsgView {
    pub role: &'static str,
    pub text: String,
    /// 圖片: (content 的位置, part 的位置), 用 /api/ai/chats/{id}/img/{c}/{p} 取得
    pub images: Vec<(usize, usize)>,
    /// 用過的工具(中文說明)
    pub tools: Vec<String>,
}

#[derive(Serialize)]
pub struct ChatView {
    pub id: String,
    pub title: String,
    pub messages: Vec<MsgView>,
    pub running: bool,
    pub partial: String,
    pub status: String,
    pub error: Option<String>,
}

struct Inner {
    chats_file: PathBuf,
    key_file: PathBuf,
    chats: Mutex<Vec<Chat>>,
    runs: Mutex<HashMap<String, Run>>,
    key: RwLock<String>,
    model: RwLock<String>,
    node: RwLock<String>,
    ver: tokio::sync::watch::Sender<u64>,
    deps: tools::Deps,
    logs: Mutex<Vec<(bool, String)>>,
    /// 要執行的 CLI 指令(例如設定 key 後同步 ai.node)
    cmds: Mutex<Vec<String>>,
    /// TUI 目前的對話
    cli_chat: Mutex<Option<String>>,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<Inner>);

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_id() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 5];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    format!("{:x}{}", now(), b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// 單行、最多 n 個字
fn short(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n { one.chars().take(n).collect::<String>() + "…" } else { one }
}

fn write_private(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("無法寫入 {}: {e}", tmp.display()))?;
    f.write_all(data).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn is_tool_result(c: &Value) -> bool {
    c["parts"].as_array().is_some_and(|p| p.iter().any(|x| x.get("functionResponse").is_some()))
}

fn has_call(c: &Value) -> bool {
    c["parts"].as_array().is_some_and(|p| p.iter().any(|x| x.get("functionCall").is_some()))
}

/// 模型這一則的文字
fn model_text(c: &Value) -> String {
    c["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["thought"].as_bool() != Some(true))
        .filter_map(|p| p["text"].as_str())
        .collect()
}

/// 加新訊息前整理: 去掉沒做完的工具呼叫(例如中途停止), 對話要能接著送
fn repair(contents: &mut Vec<Value>) {
    loop {
        match contents.last() {
            Some(c) if c["role"] == "model" && has_call(c) => {
                contents.pop();
            }
            Some(c) if is_tool_result(c) => {
                contents.pop();
            }
            _ => break,
        }
    }
}

impl Shared {
    pub fn new(deps: tools::Deps) -> Self {
        Self::with_dir(PathBuf::from("."), deps)
    }

    fn with_dir(dir: PathBuf, deps: tools::Deps) -> Self {
        let chats_file = dir.join(CHATS_FILE);
        let key_file = dir.join(KEY_FILE);
        let chats = std::fs::read(&chats_file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let key = std::fs::read_to_string(&key_file).unwrap_or_default().trim().to_string();
        Self(Arc::new(Inner {
            chats_file,
            key_file,
            chats: Mutex::new(chats),
            runs: Mutex::new(HashMap::new()),
            key: RwLock::new(key),
            model: RwLock::new(DEFAULT_MODEL.into()),
            node: RwLock::new(String::new()),
            ver: tokio::sync::watch::channel(0).0,
            deps,
            logs: Mutex::new(Vec::new()),
            cmds: Mutex::new(Vec::new()),
            cli_chat: Mutex::new(None),
        }))
    }

    fn log(&self, warn: bool, text: impl Into<String>) {
        self.0.logs.lock().unwrap().push((warn, text.into()));
    }

    fn bump(&self) {
        self.0.ver.send_modify(|v| *v += 1);
    }

    pub fn version(&self) -> u64 {
        *self.0.ver.borrow()
    }

    /// 等到有任何變化(或逾時), 回傳新的版本
    pub async fn wait(&self, after: u64, secs: u64) -> u64 {
        let mut rx = self.0.ver.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(secs), rx.wait_for(|v| *v != after)).await;
        *rx.borrow()
    }

    pub fn configured(&self) -> bool {
        !self.0.key.read().unwrap().is_empty()
    }

    pub fn model(&self) -> String {
        self.0.model.read().unwrap().clone()
    }

    pub fn me(&self) -> String {
        self.0.deps.cluster.name()
    }

    pub fn cluster(&self) -> &crate::plugins::cluster::Shared {
        &self.0.deps.cluster
    }

    /// AI 在別台時回傳那台的名稱
    pub fn remote(&self) -> Option<String> {
        let n = self.0.node.read().unwrap().clone();
        (!n.is_empty() && n != self.me()).then_some(n)
    }

    /// 設定 API key(空字串移除); 先向 Gemini 確認能用。會花幾秒, 不要在 async 裡直接呼叫
    pub fn set_key(&self, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() {
            let _ = std::fs::remove_file(&self.0.key_file);
            self.0.key.write().unwrap().clear();
            self.0.cmds.lock().unwrap().push("/settings reset ai.node".into());
            self.bump();
            return Ok(());
        }
        // 只擋放不進 HTTP header 的字元; 能不能用交給 Gemini 判斷(key 有 AIza… 和 AQ.… 等格式)
        if !key.chars().all(|c| c.is_ascii_graphic()) || key.len() < 20 {
            return Err("API key 格式不對".into());
        }
        gemini::models(key)?;
        write_private(&self.0.key_file, key.as_bytes())?;
        *self.0.key.write().unwrap() = key.to_string();
        // 告訴 cluster 的其他機器: AI 在這台
        self.0.cmds.lock().unwrap().push(format!("/settings set ai.node {}", self.me()));
        self.bump();
        Ok(())
    }

    pub fn models(&self) -> Result<Vec<String>, String> {
        let key = self.0.key.read().unwrap().clone();
        if key.is_empty() {
            return Err("還沒有設定 API key".into());
        }
        gemini::models(&key)
    }

    pub fn list(&self) -> Vec<ChatMeta> {
        let runs = self.0.runs.lock().unwrap();
        let mut v: Vec<ChatMeta> = self
            .0
            .chats
            .lock()
            .unwrap()
            .iter()
            .map(|c| ChatMeta {
                id: c.id.clone(),
                title: if c.title.is_empty() { "新對話".into() } else { c.title.clone() },
                updated: c.updated,
                running: runs.get(&c.id).is_some_and(|r| r.running),
            })
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.updated));
        v
    }

    fn save(&self, chats: &[Chat]) {
        match serde_json::to_vec(chats) {
            Ok(b) => {
                if let Err(e) = write_private(&self.0.chats_file, &b) {
                    self.log(true, format!("ai: {e}"));
                }
            }
            Err(e) => self.log(true, format!("ai: {e}")),
        }
    }

    pub fn create(&self) -> String {
        let id = new_id();
        let mut chats = self.0.chats.lock().unwrap();
        chats.push(Chat { id: id.clone(), created: now(), updated: now(), ..Default::default() });
        // 太多時刪掉最舊的(沒有在回答中的)
        if chats.len() > MAX_CHATS {
            let runs = self.0.runs.lock().unwrap();
            chats.sort_by_key(|c| std::cmp::Reverse(c.updated));
            let mut keep = 0;
            chats.retain(|c| {
                keep += 1;
                keep <= MAX_CHATS || runs.get(&c.id).is_some_and(|r| r.running)
            });
        }
        self.save(&chats);
        drop(chats);
        self.bump();
        id
    }

    pub fn exists(&self, id: &str) -> bool {
        self.0.chats.lock().unwrap().iter().any(|c| c.id == id)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        if let Some(r) = self.0.runs.lock().unwrap().remove(id) {
            r.stop.store(true, Ordering::Relaxed);
        }
        let mut chats = self.0.chats.lock().unwrap();
        let n = chats.len();
        chats.retain(|c| c.id != id);
        if chats.len() == n {
            return Err("找不到這個對話".into());
        }
        self.save(&chats);
        drop(chats);
        self.bump();
        Ok(())
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<(), String> {
        let mut chats = self.0.chats.lock().unwrap();
        let c = chats.iter_mut().find(|c| c.id == id).ok_or("找不到這個對話")?;
        c.title = short(title, 60);
        self.save(&chats);
        drop(chats);
        self.bump();
        Ok(())
    }

    /// 圖片(content 位置, part 位置) → (mime, base64)
    pub fn image(&self, id: &str, c: usize, p: usize) -> Option<(String, String)> {
        let chats = self.0.chats.lock().unwrap();
        let part = &chats.iter().find(|x| x.id == id)?.contents.get(c)?["parts"][p]["inlineData"];
        Some((part["mimeType"].as_str()?.to_string(), part["data"].as_str()?.to_string()))
    }

    pub fn view(&self, id: &str) -> Option<ChatView> {
        let chats = self.0.chats.lock().unwrap();
        let c = chats.iter().find(|c| c.id == id)?;
        let mut msgs: Vec<MsgView> = Vec::new();
        for (i, content) in c.contents.iter().enumerate() {
            if is_tool_result(content) {
                continue;
            }
            let parts = content["parts"].as_array().cloned().unwrap_or_default();
            if content["role"] == "model" {
                let tools: Vec<String> = parts
                    .iter()
                    .filter_map(|p| p["functionCall"]["name"].as_str())
                    .map(|n| tools::label(n).to_string())
                    .collect();
                let text = model_text(content);
                // 工具呼叫和後面的回答併成同一則
                match msgs.last_mut() {
                    Some(m) if m.role == "model" => {
                        m.tools.extend(tools);
                        if !text.is_empty() {
                            if !m.text.is_empty() {
                                m.text.push_str("\n\n");
                            }
                            m.text.push_str(&text);
                        }
                    }
                    _ => msgs.push(MsgView { role: "model", text, images: Vec::new(), tools }),
                }
            } else {
                let text = c.display.get(&i).cloned().unwrap_or_else(|| {
                    parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")
                });
                let images = parts
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.get("inlineData").is_some())
                    .map(|(k, _)| (i, k))
                    .collect();
                msgs.push(MsgView { role: "user", text, images, tools: Vec::new() });
            }
        }
        let runs = self.0.runs.lock().unwrap();
        let r = runs.get(id);
        Some(ChatView {
            id: c.id.clone(),
            title: if c.title.is_empty() { "新對話".into() } else { c.title.clone() },
            messages: msgs,
            running: r.is_some_and(|r| r.running),
            partial: r.map(|r| r.partial.clone()).unwrap_or_default(),
            status: r.map(|r| r.status.clone()).unwrap_or_default(),
            error: r.and_then(|r| r.error.clone()),
        })
    }

    /// 送出新訊息並開始回答。`display` 是顯示用的文字(例如郵件摘要不顯示整封信)
    pub fn send(&self, id: &str, text: &str, images: Vec<Image>, display: Option<String>) -> Result<(), String> {
        if !self.configured() {
            return Err("還沒有設定 Gemini API key".into());
        }
        let text = text.trim();
        if text.is_empty() && images.is_empty() {
            return Err("內容是空的".into());
        }
        if images.len() > MAX_IMAGES {
            return Err(format!("一次最多 {MAX_IMAGES} 張圖片"));
        }
        if images.iter().any(|i| i.data.len() / 4 * 3 > MAX_IMAGE_BYTES || !i.mime.starts_with("image/")) {
            return Err("圖片太大或格式不對".into());
        }
        if self.0.runs.lock().unwrap().get(id).is_some_and(|r| r.running) {
            return Err("還在回答上一則".into());
        }
        let mut parts: Vec<Value> = images
            .iter()
            .map(|i| json!({ "inlineData": { "mimeType": i.mime, "data": i.data } }))
            .collect();
        if !text.is_empty() {
            parts.push(json!({ "text": text }));
        }
        {
            let mut chats = self.0.chats.lock().unwrap();
            let c = chats.iter_mut().find(|c| c.id == id).ok_or("找不到這個對話")?;
            repair(&mut c.contents);
            let n = c.contents.len();
            c.display.retain(|k, _| *k < n);
            // 上一則沒有得到回答(例如出錯): 接在同一則後面
            match c.contents.last_mut() {
                Some(last) if last["role"] == "user" => {
                    if let Some(p) = last["parts"].as_array_mut() {
                        p.extend(parts);
                    }
                    if let Some(d) = display.clone() {
                        c.display.insert(n - 1, d);
                    }
                }
                _ => {
                    c.contents.push(json!({ "role": "user", "parts": parts }));
                    if let Some(d) = display.clone() {
                        c.display.insert(n, d);
                    }
                }
            }
            if c.title.is_empty() {
                let t = display.as_deref().unwrap_or(text);
                c.title = short(if t.is_empty() { "圖片" } else { t }, 40);
            }
            c.updated = now();
            self.save(&chats);
        }
        self.start(id);
        Ok(())
    }

    /// 上一則出錯時再試一次
    pub fn retry(&self, id: &str) -> Result<(), String> {
        let last_user = {
            let mut chats = self.0.chats.lock().unwrap();
            let c = chats.iter_mut().find(|c| c.id == id).ok_or("找不到這個對話")?;
            repair(&mut c.contents);
            c.contents.last().is_some_and(|l| l["role"] == "user")
        };
        if !last_user {
            return Err("沒有要重試的訊息".into());
        }
        if self.0.runs.lock().unwrap().get(id).is_some_and(|r| r.running) {
            return Err("還在回答中".into());
        }
        self.start(id);
        Ok(())
    }

    pub fn stop(&self, id: &str) {
        if let Some(r) = self.0.runs.lock().unwrap().get(id) {
            r.stop.store(true, Ordering::Relaxed);
        }
    }

    fn start(&self, id: &str) {
        let stop = Arc::new(AtomicBool::new(false));
        self.0.runs.lock().unwrap().insert(
            id.to_string(),
            Run { running: true, status: "思考中…".into(), stop: stop.clone(), ..Default::default() },
        );
        self.bump();
        let me = self.clone();
        let id = id.to_string();
        std::thread::Builder::new()
            .name("ai".into())
            .spawn(move || {
                let err = me.run(&id, &stop).err();
                if let Some(r) = me.0.runs.lock().unwrap().get_mut(&id) {
                    r.running = false;
                    r.partial.clear();
                    r.status.clear();
                    r.error = err;
                }
                me.bump();
            })
            .ok();
    }

    fn with_run(&self, id: &str, f: impl FnOnce(&mut Run)) {
        if let Some(r) = self.0.runs.lock().unwrap().get_mut(id) {
            f(r);
        }
        self.bump();
    }

    /// 把一則 content 加到對話後面並存檔
    fn push(&self, id: &str, content: Value) {
        let mut chats = self.0.chats.lock().unwrap();
        if let Some(c) = chats.iter_mut().find(|c| c.id == id) {
            c.contents.push(content);
            c.updated = now();
            self.save(&chats);
        }
    }

    fn system_prompt(&self) -> String {
        let t = chrono::Local::now();
        let wd = ["一", "二", "三", "四", "五", "六", "日"][chrono::Datelike::weekday(&t).num_days_from_monday() as usize];
        let accounts: Vec<String> = self.0.deps.mail.accounts().into_iter().map(|a| a.name).collect();
        format!(
            "你是 cng6 裡的 AI 助理。cng6(Controller Next Generation Six)是使用者自己架設的系統, \
由幾台機器組成 cluster, 有郵件、待辦清單、電子發票對獎、天氣、音樂、電子書等功能。\n\
- 用繁體中文(台灣用語)回答, 除非使用者用別的語言\n\
- 可以用 Markdown(標題、清單、表格、程式碼區塊)\n\
- 需要使用者的資料時用工具查, 不要猜; 查不到就直說\n\
- 新增或修改待辦前, 如果使用者的意思不清楚先確認\n\
- 回答簡潔, 先講重點\n\n\
現在時間: {} (星期{wd})\n這台機器: {}\nGmail 帳號: {}",
            t.format("%Y-%m-%d %H:%M"),
            self.me(),
            if accounts.is_empty() { "(沒有)".to_string() } else { accounts.join(", ") },
        )
    }

    /// 產生回答(在背景 thread 執行): 串流接收, 需要時呼叫工具再繼續
    fn run(&self, id: &str, stop: &AtomicBool) -> Result<(), String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let key = self.0.key.read().unwrap().clone();
        let model = self.model();
        let system = self.system_prompt();
        let decls = tools::declarations();
        for _ in 0..MAX_ROUNDS {
            let contents = {
                let chats = self.0.chats.lock().unwrap();
                chats.iter().find(|c| c.id == id).ok_or("對話已刪除")?.contents.clone()
            };
            let res = gemini::stream(&key, &model, &system, &contents, &decls, stop, |t| {
                self.with_run(id, |r| {
                    r.partial.push_str(t);
                    r.status.clear();
                })
            });
            let reply = match res {
                Ok(r) => r,
                Err(e) => {
                    // 已經收到的部分留著
                    let partial = self.0.runs.lock().unwrap().get(id).map(|r| r.partial.clone()).unwrap_or_default();
                    if !partial.is_empty() {
                        self.push(id, json!({ "role": "model", "parts": [{ "text": partial }] }));
                    }
                    return Err(e);
                }
            };
            let stopped = stop.load(Ordering::Relaxed);
            let mut parts = reply.parts;
            if stopped {
                // 停止: 只留文字
                parts.retain(|p| p.get("functionCall").is_none());
            }
            if parts.is_empty() {
                if stopped {
                    return Ok(());
                }
                return Err(match reply.finish.as_str() {
                    "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" => "Gemini 因為安全規則沒有回答".to_string(),
                    "" | "STOP" => "Gemini 沒有回應".to_string(),
                    f => format!("Gemini 沒有回應({f})"),
                });
            }
            let calls: Vec<Value> = parts.iter().filter(|p| p.get("functionCall").is_some()).cloned().collect();
            self.push(id, json!({ "role": "model", "parts": parts }));
            self.with_run(id, |r| r.partial.clear());
            if calls.is_empty() || stopped {
                if reply.finish == "MAX_TOKENS" {
                    return Err("回答太長, 被截斷了".into());
                }
                return Ok(());
            }
            let mut results = Vec::new();
            for c in calls {
                let f = &c["functionCall"];
                let name = f["name"].as_str().unwrap_or("");
                self.with_run(id, |r| r.status = format!("{}…", tools::label(name)));
                let out = rt.block_on(tools::run(&self.0.deps, name, &f["args"]));
                let mut fr = json!({ "name": name, "response": { "result": out } });
                if let Some(cid) = f.get("id") {
                    fr["id"] = cid.clone();
                }
                results.push(json!({ "functionResponse": fr }));
            }
            self.push(id, json!({ "role": "user", "parts": results }));
            self.with_run(id, |r| r.status = "思考中…".into());
        }
        Err("工具呼叫太多次, 先停下來".into())
    }

    /// 問一個問題並等回答(TUI 與別台的 `ai ask` 用); chat 是 None 或不存在時開新的對話
    pub async fn ask(&self, chat: Option<String>, text: &str) -> Result<(String, String), String> {
        let id = match chat.filter(|c| self.exists(c)) {
            Some(c) => c,
            None => self.create(),
        };
        let before = self.0.chats.lock().unwrap().iter().find(|c| c.id == id).map_or(0, |c| c.contents.len());
        self.send(&id, text, Vec::new(), None)?;
        loop {
            let v = self.version();
            let running = self.0.runs.lock().unwrap().get(&id).is_some_and(|r| r.running);
            if !running {
                break;
            }
            self.wait(v, 30).await;
        }
        if let Some(e) = self.0.runs.lock().unwrap().get(&id).and_then(|r| r.error.clone()) {
            return Err(e);
        }
        let chats = self.0.chats.lock().unwrap();
        let c = chats.iter().find(|c| c.id == id).ok_or("對話已刪除")?;
        let answer: Vec<String> = c
            .contents
            .iter()
            .skip(before)
            .filter(|x| x["role"] == "model")
            .map(model_text)
            .filter(|t| !t.is_empty())
            .collect();
        Ok((id, answer.join("\n\n")))
    }
}

/// 從別台送到 AI 那台的請求加上這個標頭, 避免設定錯誤時互相轉來轉去
pub const HOP_HEADER: &str = "x-cng6-ai-hop";

/// 在別台的 AI 上執行(經由 cluster), 回傳 JSON
pub async fn remote_call(
    cluster: &crate::plugins::cluster::Shared,
    node: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(HOP_HEADER, "1")
        .body(axum::body::Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .map_err(|e| e.to_string())?;
    let res = cluster.bus.http(node, req).await.map_err(|e| format!("連不到 {node}: {e}"))?;
    let ok = res.status().is_success();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024).await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "error": String::from_utf8_lossy(&bytes).to_string() }));
    if ok { Ok(v) } else { Err(v["error"].as_str().unwrap_or("失敗").to_string()) }
}

pub struct Ai {
    shared: Shared,
}

impl Ai {
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }

    /// 背景執行(要連網路), 結果寫到訊息區
    fn background(&self, f: impl FnOnce(&Shared, &tokio::runtime::Runtime) -> Result<Vec<String>, String> + Send + 'static) {
        let s = self.shared.clone();
        std::thread::spawn(move || {
            let res = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| f(&s, &rt));
            match res {
                Ok(lines) => {
                    for l in lines {
                        s.log(false, l);
                    }
                }
                Err(e) => s.log(true, format!("ai: {e}")),
            }
        });
    }
}

impl Plugin for Ai {
    fn name(&self) -> &str {
        "ai"
    }

    fn description(&self) -> &str {
        "AI 助理(Google Gemini)"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("ask <問題>", "問 AI(接著這個畫面上一次的對話)"),
            ("new", "開始新的對話"),
            ("status", "目前的狀態"),
            ("models", "列出可以用的模型"),
            ("key <API key>", "設定 Gemini API key(存在這台, 權限 600; 其他機器會改用這台)"),
            ("key clear", "移除這台的 API key"),
        ]
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            ("model", "Gemini 模型(gemini-flash-latest 自動用最新的 Flash)", DEFAULT_MODEL.into()),
            ("node", "AI(API key)放在哪台, 空白表示這台; cluster 同步, 設定 key 時自動設定", String::new()),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "model" => {
                let v = value.trim().trim_start_matches("models/");
                if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
                    return Err("模型名稱格式不對, 例如 gemini-flash-latest".into());
                }
                *self.shared.0.model.write().unwrap() = v.to_string();
                Ok(())
            }
            "node" => {
                *self.shared.0.node.write().unwrap() = value.trim().to_string();
                self.shared.bump();
                Ok(())
            }
            _ => Err(format!("不支援的設定 '{key}'")),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["ask", rest @ ..] if !rest.is_empty() => {
                let q = rest.join(" ");
                ctx.info(format!("ai> {q}"));
                ctx.info("  (思考中…)");
                self.background(move |s, rt| {
                    let chat = s.0.cli_chat.lock().unwrap().clone();
                    let (id, answer) = match s.remote() {
                        Some(node) => {
                            let v = rt.block_on(remote_call(
                                s.cluster(),
                                &node,
                                "POST",
                                "/api/ai/ask",
                                Some(json!({ "chat": chat, "text": q })),
                            ))?;
                            (
                                v["chat"].as_str().unwrap_or("").to_string(),
                                v["text"].as_str().unwrap_or("").to_string(),
                            )
                        }
                        None => rt.block_on(s.ask(chat, &q))?,
                    };
                    *s.0.cli_chat.lock().unwrap() = Some(id);
                    Ok(answer.lines().map(|l| format!("  {l}")).collect())
                });
            }
            ["new"] => {
                *self.shared.0.cli_chat.lock().unwrap() = None;
                ctx.info("ai: 下一個問題會開始新的對話");
            }
            [] | ["status"] => {
                let s = &self.shared;
                match s.remote() {
                    Some(n) => ctx.info(format!("ai: 使用 {n} 的 AI(經由 cluster)")),
                    None if s.configured() => ctx.info(format!("ai: 已設定 API key, 模型 {}", s.model())),
                    None => ctx.info("ai: 還沒有設定, 用 'ai key <API key>' 設定(到 aistudio.google.com 申請)"),
                }
                ctx.info(format!("ai: {} 個對話", s.list().len()));
            }
            ["models"] => self.background(|s, rt| {
                let list: Vec<String> = match s.remote() {
                    Some(node) => {
                        let v = rt.block_on(remote_call(s.cluster(), &node, "GET", "/api/ai/models", None))?;
                        v["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string)).collect()
                    }
                    None => s.models()?,
                };
                Ok(list.into_iter().map(|m| format!("  {m}")).collect())
            }),
            ["key", "clear"] => match self.shared.set_key("") {
                Ok(()) => ctx.info("ai: 已移除 API key"),
                Err(e) => ctx.error(e),
            },
            ["key", k] => {
                let k = k.to_string();
                ctx.info("ai: 確認 API key 中…");
                self.background(move |s, _| s.set_key(&k).map(|_| vec!["ai: API key 可以用, 已儲存(cluster 的其他機器會改用這台)".into()]));
            }
            _ => ctx.error("用法: 輸入 'help ai' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        for (warn, text) in std::mem::take(&mut *self.shared.0.logs.lock().unwrap()) {
            if warn {
                ctx.warn(text);
            } else {
                ctx.info(text);
            }
        }
        for c in std::mem::take(&mut *self.shared.0.cmds.lock().unwrap()) {
            ctx.execute(c);
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("AI", "ai", "indigo"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("ai")
    }

    fn web_card(&self) -> Option<Card> {
        let s = &self.shared;
        let (status, summary) = match s.remote() {
            Some(n) => (Status::Good, format!("使用 {n} 的 Gemini")),
            None if s.configured() => (Status::Good, format!("Gemini · {}", s.model())),
            None => (Status::Off, "還沒有設定 API key".to_string()),
        };
        let mut card = Card::new().status(status).summary(summary);
        if s.remote().is_none() {
            let rows: Vec<(String, String)> = s
                .list()
                .into_iter()
                .take(4)
                .map(|c| (chrono::DateTime::from_timestamp(c.updated, 0)
                    .map(|t| t.with_timezone(&chrono::Local).format("%m/%d").to_string())
                    .unwrap_or_default(), c.title))
                .collect();
            if !rows.is_empty() {
                card = card.table(rows);
            }
        }
        Some(card)
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_drops_unfinished_tool_calls() {
        let mut c = vec![
            json!({"role": "user", "parts": [{"text": "hi"}]}),
            json!({"role": "model", "parts": [{"text": "hello"}]}),
            json!({"role": "user", "parts": [{"text": "q"}]}),
            json!({"role": "model", "parts": [{"functionCall": {"name": "weather", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"name": "weather", "response": {}}}]}),
        ];
        repair(&mut c);
        assert_eq!(c.len(), 3);
        assert_eq!(c[2]["parts"][0]["text"], "q");
        assert_eq!(model_text(&c[1]), "hello");
        assert_eq!(short("  a\n b  c ", 3), "a b…");
    }
}
