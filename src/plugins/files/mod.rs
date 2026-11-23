//! Files plugin: 網頁上的檔案管理員。
//!
//! 可以管理的範圍由 `files.roots` 設定(多個根目錄), 網頁只能存取這些資料夾裡面。
//! 刪除會移到資源回收桶(freedesktop 規格, 和桌面環境共用)。

pub mod fsops;
pub mod trash;

use std::sync::{Arc, RwLock};

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::plugin::{Ctx, Plugin, complete_from_help};
use crate::webui::{Card, WebMeta};
use fsops::Root;

const DEFAULT_ROOTS: &str = "家目錄=~";

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<RwLock<Vec<Root>>>);

impl Shared {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(
            fsops::parse_roots(DEFAULT_ROOTS).unwrap_or_default(),
        )))
    }

    pub fn roots(&self) -> Vec<Root> {
        self.0.read().unwrap().clone()
    }

    pub fn root(&self, i: usize) -> Option<Root> {
        self.0.read().unwrap().get(i).cloned()
    }
}

pub struct Files {
    shared: Shared,
}

impl Files {
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }
}

fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// 根目錄所在磁碟的 (可用, 總共)
fn disk_space(disks: &sysinfo::Disks, path: &std::path::Path) -> Option<(u64, u64)> {
    let path = path.canonicalize().ok()?;
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| (d.available_space(), d.total_space()))
}

impl Plugin for Files {
    fn name(&self) -> &str {
        "files"
    }

    fn description(&self) -> &str {
        "檔案管理員(網頁)"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("roots", "列出可以管理的根目錄"),
            ("trash", "列出資源回收桶"),
            ("trash empty", "清空資源回收桶"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        complete_from_help(&self.help(), args)
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![(
            "roots",
            "可以管理的根目錄, 格式: 名稱=路徑; 名稱=路徑",
            DEFAULT_ROOTS.into(),
        )]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "roots" => {
                let roots = fsops::parse_roots(value)?;
                *self.shared.0.write().unwrap() = roots;
                Ok(())
            }
            _ => Err(format!("不支援的設定 '{key}'")),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let roots = self.shared.roots();
        match args {
            ["roots"] => {
                for r in &roots {
                    let ok = if r.path.is_dir() { "" } else { "  (不存在)" };
                    ctx.info(format!("  {}: {}{ok}", r.name, r.path.display()));
                }
            }
            ["trash"] => {
                let items = trash::list(&roots);
                for it in &items {
                    ctx.info(format!(
                        "  {}  {}{}",
                        it.deleted.replace('T', " "),
                        it.path,
                        if it.dir { "/" } else { "" }
                    ));
                }
                ctx.info(format!("  回收桶裡有 {} 個項目", items.len()));
            }
            ["trash", "empty"] => {
                let items = trash::list(&roots);
                let mut failed = 0;
                for it in &items {
                    if let Err(e) = trash::purge(&roots, it.trash, &it.id) {
                        failed += 1;
                        ctx.error(format!("{}: {e}", it.name));
                    }
                }
                ctx.info(format!(
                    "已清空回收桶({} 個項目)",
                    items.len() - failed
                ));
            }
            _ => ctx.error("用法: 輸入 'help files' 查看"),
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("檔案", "folder", "blue"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("files")
    }

    fn web_card(&self) -> Option<Card> {
        let roots = self.shared.roots();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let mut card = Card::new().summary(format!("{} 個根目錄", roots.len()));
        for r in &roots {
            match disk_space(&disks, &r.path) {
                Some((avail, total)) if total > 0 => {
                    let used = (total - avail) as f64 / total as f64;
                    card = card.meter(
                        r.name.clone(),
                        used,
                        format!("可用 {} / {}", human(avail), human(total)),
                        None,
                    );
                }
                _ => card = card.text(format!("{}: 無法讀取", r.name), None),
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
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GB");
    }
}
