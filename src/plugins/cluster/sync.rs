//! 同步: 書的閱讀進度與書籤、音樂播放清單、天氣與世界時間的城市、桌布設定、待辦清單。
//!
//! 每一項是一個保留訊息(retain), 新加入的機器馬上拿到群組目前的資料。
//! 衝突時以最後修改的為準(時間較新的勝出)。
//!
//! - 本機定時檢查每一項的內容(雜湊值), 改變了就送出
//! - 收到比較新的就套用; 套用後內容和收到的一樣, 不會再被當成本機修改送回去
//! - 已知的狀態存在 cluster-sync.json, 重新啟動後不會把舊資料當成新的

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::http::Method;
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::bus::{Bus, Conn};
use crate::plugins::books;
use crate::plugins::books::library::Progress;
use crate::plugins::music;
use crate::plugins::music::library as mlib;
use crate::plugins::todo;
use crate::plugins::wallpaper;
use crate::remote;

/// 同步的設定
pub const SETTING_KEYS: &[&str] = &[
    "weather.locations",
    "worldclock.zones",
    "wallpaper.current",
    "wallpaper.rotate",
    "ai.node",
    "ai.model",
];
const STATE_FILE: &str = "cluster-sync.json";
/// 連線後先等保留訊息到齊, 才開始送本機的資料
const WARMUP: Duration = Duration::from_secs(8);

/// 同步需要用到的其他 plugin
#[derive(Clone)]
pub struct Deps {
    pub books: books::Shared,
    pub music: music::Shared,
    pub wallpaper: wallpaper::Shared,
    pub todo: todo::Shared,
    pub remote: remote::Sender,
    /// 目前的設定值與是否為預設值(由 plugin 從 SettingsChanged 更新)
    pub settings: Arc<RwLock<BTreeMap<String, (String, bool)>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Known {
    hash: String,
    ts: i64,
}

impl Known {
    /// 比較新; 時間一樣時用雜湊值決定, 每台的結論都一樣, 最後會收斂
    fn newer_than(&self, o: &Known) -> bool {
        (self.ts, &self.hash) > (o.ts, &o.hash)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMsg {
    key: String,
    ts: i64,
    origin: String,
    hash: String,
    /// null = 刪除(播放清單)
    value: Value,
}

const DELETED: &str = "deleted";

fn hash(v: &Value) -> String {
    if v.is_null() {
        return DELETED.into();
    }
    let s = serde_json::to_string(v).unwrap_or_default();
    digest(&SHA256, s.as_bytes())
        .as_ref()
        .iter()
        .take(10)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn mtime(p: &Path) -> i64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).map(|m| m.mtime()).unwrap_or(0)
}

/// 用引號包起來, 讓 CLI 把整個值當成一個參數
fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

pub struct Sync {
    deps: Deps,
    known: BTreeMap<String, Known>,
    /// 這次連線收到的群組資料
    remote: HashMap<String, Known>,
    /// 要重新送出的(收到比較舊的資料)
    resend: HashSet<String>,
    path: PathBuf,
    dirty: bool,
    ready_at: Option<Instant>,
}

impl Sync {
    pub fn new(deps: Deps) -> Self {
        let path = PathBuf::from(STATE_FILE);
        let known = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            deps,
            known,
            remote: HashMap::new(),
            resend: HashSet::new(),
            path,
            dirty: false,
            ready_at: None,
        }
    }

    /// 新的連線: 等保留訊息到齊再開始
    pub fn connected(&mut self) {
        self.remote.clear();
        self.ready_at = Some(Instant::now() + WARMUP);
        // 先記下本機每一項目前的狀態(用它本身的時間), 群組裡比較舊的資料才不會蓋掉本機的
        for (key, (value, natural)) in self.local_items() {
            self.known.entry(key).or_insert_with(|| Known {
                hash: hash(&value),
                ts: natural,
            });
        }
    }

    /// 本機目前的每一項: key → (內容, 本身的時間)
    fn local_items(&self) -> BTreeMap<String, (Value, i64)> {
        let mut out = BTreeMap::new();
        let bdir = self.deps.books.dir();
        for (file, p) in self.deps.books.lib().load_progress(&bdir) {
            if p.updated > 0 {
                let ts = p.updated as i64;
                out.insert(format!("book:{file}"), (serde_json::to_value(p).unwrap_or(Value::Null), ts));
            }
        }
        let mdir = self.deps.music.dir();
        for (name, files) in mlib::playlists(&mdir) {
            let ts = mtime(&mlib::playlist_path(&mdir, &name));
            out.insert(format!("playlist:{name}"), (Value::from(files), ts));
        }
        // 待辦: 每一項一筆, 時間用它最後修改的時間
        for (id, v, ts) in self.deps.todo.sync_items() {
            out.insert(format!("todo:{id}"), (v, ts));
        }
        let settings = self.deps.settings.read().unwrap();
        for k in SETTING_KEYS {
            if let Some((v, is_default)) = settings.get(*k) {
                // 設定沒有時間: 預設值用 0, 有設定過的用 1,
                // 第一次加入群組時採用群組的設定, 但不會被別台的預設值蓋掉
                out.insert(format!("setting:{k}"), (Value::from(v.clone()), if *is_default { 0 } else { 1 }));
            }
        }
        out
    }

    /// 定時檢查: 送出本機改變的項目
    pub fn poll(&mut self, conn: &Conn, now: i64) {
        if self.ready_at.is_none_or(|t| Instant::now() < t) {
            return;
        }
        let items = self.local_items();
        let mut send: Vec<(String, Value, Known)> = Vec::new();
        for (key, (value, natural)) in &items {
            let h = hash(value);
            let known = self.known.get(key).cloned();
            let k = match &known {
                // 第一次看到: 用它本身的時間, 不要蓋掉群組比較新的資料
                None => Known { hash: h.clone(), ts: *natural },
                Some(k) if k.hash != h => Known {
                    hash: h.clone(),
                    // 閱讀進度、待辦有自己的修改時間, 比較準
                    ts: if (key.starts_with("book:") || key.starts_with("todo:")) && *natural > k.ts {
                        *natural
                    } else {
                        now.max(k.ts + 1)
                    },
                },
                Some(k) => k.clone(),
            };
            let changed = known.as_ref() != Some(&k);
            // 群組裡沒有、或群組的比較舊: 送出
            let group_behind = self.remote.get(key).is_none_or(|r| k.newer_than(r));
            if changed || self.resend.remove(key) || group_behind {
                send.push((key.clone(), value.clone(), k.clone()));
            }
            if changed {
                self.known.insert(key.clone(), k);
                self.dirty = true;
            }
        }
        // 刪除的播放清單、待辦
        let deleted: Vec<String> = self
            .known
            .iter()
            .filter(|(k, v)| {
                (k.starts_with("playlist:") || k.starts_with("todo:")) && v.hash != DELETED && !items.contains_key(*k)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in deleted {
            let k = Known {
                hash: DELETED.into(),
                ts: now,
            };
            self.known.insert(key.clone(), k.clone());
            self.dirty = true;
            send.push((key, Value::Null, k));
        }
        for (key, value, k) in send {
            // 群組現在有這一份了, 下次檢查不用再送
            self.remote.insert(key.clone(), k.clone());
            let msg = SyncMsg {
                key: key.clone(),
                ts: k.ts,
                origin: conn.me.clone(),
                hash: k.hash,
                value,
            };
            let topic = conn.cipher.sync_topic(&conn.group, &key);
            let payload = conn.cipher.seal(&topic, &serde_json::to_vec(&msg).unwrap_or_default());
            let _ = conn
                .client
                .try_publish(topic, rumqttc::QoS::AtLeastOnce, true, payload);
        }
        self.save();
    }

    fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Ok(json) = serde_json::to_string_pretty(&self.known) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// 收到群組的資料; 回傳要記錄的訊息
    pub fn on_message(&mut self, conn: &Conn, bus: &Arc<Bus>, topic: &str, m: SyncMsg) -> Option<String> {
        // topic 必須是這個 key 的 topic
        if conn.cipher.sync_topic(&conn.group, &m.key) != topic || hash(&m.value) != m.hash {
            return None;
        }
        let incoming = Known {
            hash: m.hash.clone(),
            ts: m.ts,
        };
        self.remote.insert(m.key.clone(), incoming.clone());
        let known = self.known.get(&m.key).cloned();
        match &known {
            Some(k) if k.hash == m.hash => {
                // 內容一樣: 只更新時間
                if m.ts > k.ts {
                    self.known.insert(m.key.clone(), incoming);
                    self.dirty = true;
                }
                return None;
            }
            Some(k) if !incoming.newer_than(k) => {
                // 本機的比較新: 送出本機的版本
                if m.origin != conn.me {
                    self.resend.insert(m.key.clone());
                }
                return None;
            }
            _ => {}
        }
        if m.origin == conn.me {
            return None;
        }
        let note = self.apply(bus, &m)?;
        self.known.insert(m.key.clone(), incoming);
        self.dirty = true;
        self.save();
        Some(note)
    }

    /// 套用一項; 回傳記錄用的說明
    fn apply(&self, bus: &Arc<Bus>, m: &SyncMsg) -> Option<String> {
        let (kind, name) = m.key.split_once(':')?;
        match kind {
            "book" => {
                let p: Progress = serde_json::from_value(m.value.clone()).ok()?;
                if !books::library::is_safe_name(name) {
                    return None;
                }
                let dir = self.deps.books.dir();
                if !dir.is_dir() {
                    return None;
                }
                self.deps.books.lib().save_progress(&dir, name, p).ok()?;
                Some(format!("同步閱讀進度: {name}(來自 {})", m.origin))
            }
            "playlist" => {
                if !mlib::is_playlist_name(name) {
                    return None;
                }
                let dir = self.deps.music.dir();
                if !dir.is_dir() {
                    return None;
                }
                if m.value.is_null() {
                    let _ = std::fs::remove_file(mlib::playlist_path(&dir, name));
                    Some(format!("同步刪除播放清單: {name}(來自 {})", m.origin))
                } else {
                    let files: Vec<String> = serde_json::from_value(m.value.clone()).ok()?;
                    mlib::write_playlist(&dir, name, &files).ok()?;
                    Some(format!("同步播放清單: {name}(來自 {})", m.origin))
                }
            }
            "todo" => self
                .deps
                .todo
                .apply_sync(name, (!m.value.is_null()).then(|| m.value.clone()), &m.origin),
            "setting" => {
                if !SETTING_KEYS.contains(&name) {
                    return None;
                }
                let value = m.value.as_str()?.to_string();
                let key = name.to_string();
                let remote = self.deps.remote.clone();
                let origin = m.origin.clone();
                // 桌布: 本機沒有這張圖就先從對方抓過來
                let fetch = (key == "wallpaper.current"
                    && value != wallpaper::DEFAULT
                    && wallpaper::is_safe_name(&value))
                .then(|| self.deps.wallpaper.get().dir.join(&value))
                .filter(|p| !p.exists());
                let bus = bus.clone();
                tokio::spawn(async move {
                    if let Some(path) = fetch {
                        let uri = format!("/wallpapers/{}", urlencode(&value));
                        match bus.call(&origin, Method::GET, &uri, None, Vec::new()).await {
                            Ok((200, data)) => {
                                if let Some(parent) = path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                if std::fs::write(&path, data).is_err() {
                                    return;
                                }
                            }
                            _ => return,
                        }
                    }
                    let (tx, _rx) = tokio::sync::oneshot::channel();
                    let _ = remote.send(remote::Request {
                        session: "cluster-sync".into(),
                        peer: format!("cluster:{origin}"),
                        op: remote::Op::Exec,
                        line: format!("settings set {key} {}", quote(&value)),
                        reply: tx,
                    });
                });
                None // 執行設定時 Message 會記錄
            }
            _ => None,
        }
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(quote(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(
            crate::parse::split_args(&format!("settings set k {}", quote("台北@25,121; X@1,2"))),
            ["settings", "set", "k", "台北@25,121; X@1,2"]
        );
        assert_eq!(hash(&Value::Null), DELETED);
        assert_eq!(hash(&Value::from("a")), hash(&Value::from("a")));
        assert_ne!(hash(&Value::from("a")), hash(&Value::from("b")));
        assert_eq!(urlencode("桌 1.jpg"), "%E6%A1%8C%201.jpg");
    }
}
