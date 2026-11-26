//! Todo plugin: 待辦清單, 整個 cluster 共用一份。
//!
//! - 每一項有優先順序(高 / 中 / 低)、完成與否、備註、到期日(選填)
//! - 存在 todos.json; cluster 的同步(sync.rs)把每一項當成一筆資料同步, 衝突時最後修改的為準
//! - 排序: 未完成在前, 依優先順序 → 到期日 → 建立時間; 完成的依完成時間(新的在前)

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::plugin::{Ctx, Plugin, complete_from_help};
use crate::webui::{Card, Status, WebMeta};

const FILE: &str = "todos.json";
/// 標題、備註的長度上限(字元)
const MAX_TITLE: usize = 500;
const MAX_NOTES: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    /// 1 高、2 中、3 低
    pub priority: u8,
    #[serde(default)]
    pub done: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// 到期日 YYYY-MM-DD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// 建立、最後修改、完成的時間(unix 秒)
    pub created: i64,
    pub updated: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<i64>,
    /// 最後是哪一台改的
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
}

/// 修改的內容(沒給的欄位不變)
#[derive(Debug, Default, Deserialize)]
pub struct Patch {
    pub title: Option<String>,
    pub priority: Option<u8>,
    pub done: Option<bool>,
    pub notes: Option<String>,
    /// 空字串表示清掉到期日
    pub due: Option<String>,
}

pub fn priority_name(p: u8) -> &'static str {
    match p {
        1 => "高",
        3 => "低",
        _ => "中",
    }
}

/// 高 / 中 / 低、1~3、high / medium / low
pub fn parse_priority(s: &str) -> Option<u8> {
    match s.trim().to_lowercase().as_str() {
        "高" | "1" | "high" | "h" | "!!!" => Some(1),
        "中" | "2" | "medium" | "m" | "!!" => Some(2),
        "低" | "3" | "low" | "l" | "!" => Some(3),
        _ => None,
    }
}

fn valid_due(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_id() -> String {
    let mut b = [0u8; 8];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 排序用: 未完成在前(優先順序、到期日、建立時間), 完成的在後(最近完成的在前)
fn order(a: &Item, b: &Item) -> std::cmp::Ordering {
    a.done.cmp(&b.done).then_with(|| {
        if a.done {
            b.done_at.unwrap_or(b.updated).cmp(&a.done_at.unwrap_or(a.updated))
        } else {
            a.priority
                .cmp(&b.priority)
                .then_with(|| match (&a.due, &b.due) {
                    (Some(x), Some(y)) => x.cmp(y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then(a.created.cmp(&b.created))
        }
    })
}

struct Inner {
    path: PathBuf,
    items: Mutex<Vec<Item>>,
    /// 這台的名稱(記錄誰改的)
    me: Mutex<String>,
}

/// 和網頁伺服器、cluster 同步共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<Inner>);

impl Shared {
    pub fn new() -> Self {
        Self::with_path(PathBuf::from(FILE))
    }

    fn with_path(path: PathBuf) -> Self {
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let me = sysinfo::System::host_name().unwrap_or_default();
        Self(Arc::new(Inner {
            path,
            items: Mutex::new(items),
            me: Mutex::new(me),
        }))
    }

    /// 排序好的清單
    pub fn list(&self) -> Vec<Item> {
        let mut v = self.0.items.lock().unwrap().clone();
        v.sort_by(order);
        v
    }

    fn save(&self, items: &[Item]) -> Result<(), String> {
        let data = serde_json::to_vec_pretty(items).map_err(|e| e.to_string())?;
        let tmp = self.0.path.with_extension("json.tmp");
        std::fs::write(&tmp, data)
            .and_then(|_| std::fs::rename(&tmp, &self.0.path))
            .map_err(|e| format!("無法寫入 {}: {e}", self.0.path.display()))
    }

    pub fn add(&self, title: &str, priority: u8, due: Option<String>, notes: &str) -> Result<Item, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("內容不能是空的".into());
        }
        if title.chars().count() > MAX_TITLE || notes.chars().count() > MAX_NOTES {
            return Err("內容太長了".into());
        }
        let due = due.filter(|d| !d.is_empty());
        if let Some(d) = &due
            && !valid_due(d)
        {
            return Err("到期日格式是 YYYY-MM-DD".into());
        }
        let t = now();
        let item = Item {
            id: new_id(),
            title: title.to_string(),
            priority: priority.clamp(1, 3),
            done: false,
            notes: notes.trim().to_string(),
            due,
            created: t,
            updated: t,
            done_at: None,
            by: self.0.me.lock().unwrap().clone(),
        };
        let mut items = self.0.items.lock().unwrap();
        items.push(item.clone());
        self.save(&items)?;
        Ok(item)
    }

    pub fn update(&self, id: &str, p: Patch) -> Result<Item, String> {
        let me = self.0.me.lock().unwrap().clone();
        let mut items = self.0.items.lock().unwrap();
        let it = items.iter_mut().find(|i| i.id == id).ok_or("找不到這一項(可能已經刪除)")?;
        if let Some(t) = p.title {
            let t = t.trim();
            if t.is_empty() {
                return Err("內容不能是空的".into());
            }
            if t.chars().count() > MAX_TITLE {
                return Err("內容太長了".into());
            }
            it.title = t.to_string();
        }
        if let Some(pr) = p.priority {
            it.priority = pr.clamp(1, 3);
        }
        if let Some(n) = p.notes {
            if n.chars().count() > MAX_NOTES {
                return Err("備註太長了".into());
            }
            it.notes = n.trim().to_string();
        }
        if let Some(d) = p.due {
            let d = d.trim();
            if d.is_empty() {
                it.due = None;
            } else if valid_due(d) {
                it.due = Some(d.to_string());
            } else {
                return Err("到期日格式是 YYYY-MM-DD".into());
            }
        }
        let t = now();
        if let Some(done) = p.done
            && done != it.done
        {
            it.done = done;
            it.done_at = done.then_some(t);
        }
        // 同一秒改兩次: 時間也要往前, 同步才知道是新的
        it.updated = t.max(it.updated + 1);
        it.by = me;
        let out = it.clone();
        self.save(&items)?;
        Ok(out)
    }

    pub fn remove(&self, id: &str) -> Result<Item, String> {
        let mut items = self.0.items.lock().unwrap();
        let i = items.iter().position(|i| i.id == id).ok_or("找不到這一項(可能已經刪除)")?;
        let it = items.remove(i);
        self.save(&items)?;
        Ok(it)
    }

    /// 清掉全部已完成的
    pub fn clear_done(&self) -> Result<usize, String> {
        let mut items = self.0.items.lock().unwrap();
        let before = items.len();
        items.retain(|i| !i.done);
        let n = before - items.len();
        self.save(&items)?;
        Ok(n)
    }

    // ---- cluster 同步用 ----

    /// 每一項: id → (內容, 最後修改時間)
    pub fn sync_items(&self) -> Vec<(String, serde_json::Value, i64)> {
        self.0
            .items
            .lock()
            .unwrap()
            .iter()
            .map(|i| (i.id.clone(), serde_json::to_value(i).unwrap_or_default(), i.updated))
            .collect()
    }

    /// 套用群組來的一項(None 表示刪除); 回傳記錄用的說明(cluster 會記錄)。
    /// 不合法的資料回傳 None(不套用)
    pub fn apply_sync(&self, id: &str, value: Option<serde_json::Value>, origin: &str) -> Option<String> {
        let mut items = self.0.items.lock().unwrap();
        let note = match value {
            None => match items.iter().position(|i| i.id == id) {
                Some(i) => format!("同步刪除待辦: {}(來自 {origin})", items.remove(i).title),
                None => format!("同步刪除待辦(來自 {origin})"),
            },
            Some(v) => {
                let mut it: Item = serde_json::from_value(v).ok()?;
                if it.id != id || it.title.trim().is_empty() {
                    return None;
                }
                it.priority = it.priority.clamp(1, 3);
                let note = format!("同步待辦: {}(來自 {origin})", it.title);
                match items.iter_mut().find(|i| i.id == id) {
                    Some(old) => *old = it,
                    None => items.push(it),
                }
                note
            }
        };
        let _ = self.save(&items);
        Some(note)
    }
}

pub struct Todo {
    shared: Shared,
}

impl Todo {
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }

    /// 指令裡的編號(todo list 顯示的 1、2、3…)或 id
    fn find(&self, key: &str) -> Option<Item> {
        let list = self.shared.list();
        match key.trim_start_matches('#').parse::<usize>() {
            Ok(n) if n >= 1 && n <= list.len() => Some(list[n - 1].clone()),
            _ => list.into_iter().find(|i| i.id.starts_with(key) && key.len() >= 4),
        }
    }
}

fn line(n: usize, i: &Item) -> String {
    let mut s = format!(
        "  {n:>2}. [{}] {} {}",
        if i.done { "x" } else { " " },
        priority_name(i.priority),
        i.title
    );
    if let Some(d) = &i.due {
        s.push_str(&format!("  (到期 {d})"));
    }
    s
}

impl Plugin for Todo {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "待辦清單(整個 cluster 共用一份)"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("list", "列出待辦(未完成在前, 依優先順序)"),
            ("add <內容>", "新增, 可以在前面加 高/中/低, 例如: todo add 高 繳電費"),
            ("done <編號>", "標成完成"),
            ("undo <編號>", "標成未完成"),
            ("pri <編號> <高|中|低>", "改優先順序"),
            ("del <編號>", "刪除"),
            ("clear", "清掉全部已完成的"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["done" | "undo" | "pri" | "del"] => self
                .shared
                .list()
                .iter()
                .enumerate()
                .map(|(n, i)| ((n + 1).to_string(), i.title.clone()))
                .collect(),
            ["pri", _] | ["add"] => ["高", "中", "低"].iter().map(|p| (p.to_string(), String::new())).collect(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let res: Result<String, String> = match args {
            [] | ["list"] => {
                let list = self.shared.list();
                if list.is_empty() {
                    ctx.info("  沒有待辦事項");
                }
                for (n, i) in list.iter().enumerate() {
                    ctx.info(line(n + 1, i));
                }
                return;
            }
            ["add", rest @ ..] if !rest.is_empty() => {
                let (pri, words) = match parse_priority(rest[0]) {
                    Some(p) if rest.len() > 1 => (p, &rest[1..]),
                    _ => (2, rest),
                };
                self.shared
                    .add(&words.join(" "), pri, None, "")
                    .map(|i| format!("已新增({}): {}", priority_name(i.priority), i.title))
            }
            ["done" | "undo", key] => match self.find(key) {
                Some(i) => {
                    let done = args[0] == "done";
                    self.shared
                        .update(&i.id, Patch { done: Some(done), ..Default::default() })
                        .map(|i| format!("{}: {}", if done { "完成" } else { "改回未完成" }, i.title))
                }
                None => Err(format!("沒有第 {key} 項")),
            },
            ["pri", key, p] => match (self.find(key), parse_priority(p)) {
                (Some(i), Some(p)) => self
                    .shared
                    .update(&i.id, Patch { priority: Some(p), ..Default::default() })
                    .map(|i| format!("{} → {}", i.title, priority_name(i.priority))),
                (None, _) => Err(format!("沒有第 {key} 項")),
                (_, None) => Err("優先順序請用 高 / 中 / 低".into()),
            },
            ["del", key] => match self.find(key) {
                Some(i) => self.shared.remove(&i.id).map(|i| format!("已刪除: {}", i.title)),
                None => Err(format!("沒有第 {key} 項")),
            },
            ["clear"] => self.shared.clear_done().map(|n| format!("已清掉 {n} 項已完成的")),
            _ => Err("用法: 輸入 'help todo' 查看".into()),
        };
        match res {
            Ok(m) => ctx.info(format!("todo: {m}")),
            Err(e) => ctx.error(e),
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("待辦", "todo", "orange"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("todo")
    }

    fn web_card(&self) -> Option<Card> {
        let list = self.shared.list();
        let open: Vec<&Item> = list.iter().filter(|i| !i.done).collect();
        let high = open.iter().filter(|i| i.priority == 1).count();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let overdue = open.iter().filter(|i| i.due.as_deref().is_some_and(|d| d < today.as_str())).count();
        let summary = if open.is_empty() {
            "沒有待辦事項".to_string()
        } else {
            let mut s = format!("{} 項未完成", open.len());
            if high > 0 {
                s.push_str(&format!(" · 高 {high}"));
            }
            if overdue > 0 {
                s.push_str(&format!(" · 過期 {overdue}"));
            }
            s
        };
        let status = if overdue > 0 { Status::Warning } else { Status::Good };
        let mut card = Card::new().status(status).summary(summary);
        let rows: Vec<(String, String)> = open
            .iter()
            .take(6)
            .map(|i| (priority_name(i.priority).to_string(), i.title.clone()))
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

    #[test]
    fn add_update_sort_sync() {
        let dir = std::env::temp_dir().join(format!("cng6-todo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = Shared::with_path(dir.join("t.json"));
        let low = t.add("低的", 3, None, "").unwrap();
        let high = t.add("高的", 1, Some("2026-12-01".into()), "").unwrap();
        let mid = t.add("中的", 2, None, "").unwrap();
        assert!(t.add("  ", 2, None, "").is_err());
        assert!(t.add("x", 2, Some("12/1".into()), "").is_err());
        let names = |t: &Shared| t.list().iter().map(|i| i.title.clone()).collect::<Vec<_>>();
        assert_eq!(names(&t), ["高的", "中的", "低的"]);
        // 完成的排到最後
        t.update(&high.id, Patch { done: Some(true), ..Default::default() }).unwrap();
        assert_eq!(names(&t), ["中的", "低的", "高的"]);
        // 改優先順序
        t.update(&low.id, Patch { priority: Some(1), ..Default::default() }).unwrap();
        assert_eq!(names(&t), ["低的", "中的", "高的"]);
        // 存檔後讀回
        let t2 = Shared::with_path(dir.join("t.json"));
        assert_eq!(names(&t2), names(&t));
        // 同步: 新增、修改、刪除
        let mut v = serde_json::to_value(&mid).unwrap();
        v["title"] = "別台改的".into();
        t2.apply_sync(&mid.id, Some(v), "pi4");
        assert!(names(&t2).contains(&"別台改的".to_string()));
        t2.apply_sync(&mid.id, None, "pi4");
        assert!(!names(&t2).contains(&"別台改的".to_string()));
        // id 不符的不套用
        let v = serde_json::to_value(&low).unwrap();
        t2.apply_sync("other", Some(v), "pi4");
        assert_eq!(t2.list().len(), 2);
        assert_eq!(t2.clear_done().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn priorities() {
        assert_eq!(parse_priority("高"), Some(1));
        assert_eq!(parse_priority("low"), Some(3));
        assert_eq!(parse_priority("x"), None);
        assert_eq!(priority_name(2), "中");
    }
}
