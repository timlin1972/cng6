//! Wallpaper plugin: 網頁桌面的桌布。
//!
//! - 圖片放在 `wallpaper.dir` 資料夾(預設 `wallpapers/`), 網頁可以上傳/刪除
//! - 目前的桌布存在設定 `wallpaper.current`(`default` 表示內建的漸層桌布)
//! - `wallpaper.rotate` 分鐘數 > 0 時自動輪播; 輪播只改變執行中顯示的桌布, 不寫入設定檔
//!
//! 網頁伺服器透過 [`Shared`] 讀取資料夾位置與目前顯示的桌布。

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::plugin::{Action, Ctx, Plugin, complete_from_help, parse_usize};
use crate::webui::{Button, Card, WebMeta};

pub const DEFAULT: &str = "default";
const DEFAULT_DIR: &str = "wallpapers";
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];

/// 是否為支援的圖片副檔名
pub fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// 檔名是否安全(不含路徑、不是隱藏檔、是支援的圖片)
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && is_image(name)
}

/// 和網頁伺服器共用的狀態
#[derive(Debug, Clone, Default)]
pub struct State {
    pub dir: PathBuf,
    /// 目前顯示的桌布(輪播時會和設定值不同)
    pub showing: String,
    /// 每次改變就加一, 讓網頁知道要更新
    pub version: u64,
}

#[derive(Clone, Default)]
pub struct Shared(Arc<RwLock<State>>);

impl Shared {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(State {
            dir: DEFAULT_DIR.into(),
            showing: DEFAULT.into(),
            version: 1,
        })))
    }

    pub fn get(&self) -> State {
        self.0.read().unwrap().clone()
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.0.write().unwrap();
        f(&mut s);
        s.version += 1;
    }

    /// 資料夾裡的圖片(依檔名排序)
    pub fn list(&self) -> Vec<String> {
        list_images(&self.get().dir)
    }
}

fn list_images(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_safe_name(n))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub struct Wallpaper {
    shared: Shared,
    /// 設定的桌布
    current: String,
    rotate: Duration,
    last_change: Instant,
}

impl Wallpaper {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            current: DEFAULT.into(),
            rotate: Duration::ZERO,
            last_change: Instant::now(),
        }
    }

    fn show(&mut self, name: &str) {
        let name = name.to_string();
        self.shared.update(|s| s.showing = name);
        self.last_change = Instant::now();
    }

    /// 相對目前顯示的桌布, 往前/往後第 step 張
    fn neighbor(&self, step: isize) -> Option<String> {
        let list = self.shared.list();
        if list.is_empty() {
            return None;
        }
        let showing = self.shared.get().showing;
        let next = match list.iter().position(|n| *n == showing) {
            Some(i) => (i as isize + step).rem_euclid(list.len() as isize) as usize,
            None if step >= 0 => 0,
            None => list.len() - 1,
        };
        Some(list[next].clone())
    }
}

impl Plugin for Wallpaper {
    fn name(&self) -> &str {
        "wallpaper"
    }

    fn description(&self) -> &str {
        "網頁桌面的桌布"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("list", "列出資料夾裡的桌布"),
            ("set <name>", "換桌布並寫入設定檔 (default = 內建漸層)"),
            ("next", "換下一張"),
            ("prev", "換上一張"),
            ("status", "顯示目前桌布、資料夾與輪播"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["set"] => {
                let mut c = vec![(DEFAULT.to_string(), "內建漸層桌布".to_string())];
                c.extend(self.shared.list().into_iter().map(|n| (n, String::new())));
                c
            }
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            ("dir", "存放桌布圖片的資料夾", DEFAULT_DIR.into()),
            (
                "current",
                "目前的桌布檔名, default 為內建漸層",
                DEFAULT.into(),
            ),
            (
                "rotate",
                "自動輪播間隔(分鐘), 0 為不輪播 (0~1440)",
                "0".into(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dir" => {
                let dir = PathBuf::from(value);
                self.shared.update(|s| s.dir = dir);
            }
            "current" => {
                if value != DEFAULT {
                    if !is_safe_name(value) {
                        return Err(format!("'{value}' 不是支援的圖片檔名"));
                    }
                    if !self.shared.get().dir.join(value).is_file() {
                        return Err(format!("資料夾裡沒有 '{value}'"));
                    }
                }
                self.current = value.to_string();
                self.show(value);
            }
            "rotate" => {
                let min = parse_usize(value, 0, 1440)?;
                self.rotate = Duration::from_secs(min as u64 * 60);
                self.last_change = Instant::now();
            }
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let set = |ctx: &mut Ctx, name: String| {
            ctx.push(Action::SetSetting("wallpaper.current".into(), name));
        };
        match args {
            ["list"] => {
                let list = self.shared.list();
                let showing = self.shared.get().showing;
                let mark = |n: &str| if n == showing { " *" } else { "" };
                ctx.info(format!("  {DEFAULT}{}", mark(DEFAULT)));
                for n in &list {
                    ctx.info(format!("  {n}{}", mark(n)));
                }
                if list.is_empty() {
                    let dir = self.shared.get().dir;
                    ctx.info(format!(
                        "  (資料夾 {} 裡還沒有圖片, 可以從網頁 Settings › 桌布 上傳)",
                        dir.display()
                    ));
                }
            }
            ["set", name] => set(ctx, name.to_string()),
            ["set", ..] => ctx.error("用法: wallpaper set <name>, 檔名有空白請加引號"),
            ["next"] | ["prev"] => {
                let step = if args[0] == "next" { 1 } else { -1 };
                match self.neighbor(step) {
                    Some(n) => set(ctx, n),
                    None => ctx.warn("資料夾裡沒有圖片"),
                }
            }
            ["status"] => {
                let st = self.shared.get();
                ctx.info(format!("目前桌布: {}", st.showing));
                if st.showing != self.current {
                    ctx.info(format!("設定桌布: {} (輪播中)", self.current));
                }
                ctx.info(format!(
                    "資料夾:   {} ({} 張)",
                    st.dir.display(),
                    self.shared.list().len()
                ));
                if self.rotate.is_zero() {
                    ctx.info("輪播:     關閉");
                } else {
                    ctx.info(format!("輪播:     每 {} 分鐘", self.rotate.as_secs() / 60));
                }
            }
            _ => ctx.error("用法: 輸入 'help wallpaper' 查看"),
        }
    }

    fn on_tick(&mut self, _ctx: &mut Ctx) {
        if !self.rotate.is_zero()
            && self.last_change.elapsed() >= self.rotate
            && let Some(next) = self.neighbor(1)
        {
            // 輪播不寫入設定檔
            self.show(&next);
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("桌布", "wallpaper", "teal"))
    }

    fn web_card(&self) -> Option<Card> {
        let st = self.shared.get();
        let count = self.shared.list().len();
        let rotate = if self.rotate.is_zero() {
            "關閉".to_string()
        } else {
            format!("每 {} 分鐘", self.rotate.as_secs() / 60)
        };
        let showing = if st.showing == DEFAULT {
            "預設漸層".to_string()
        } else {
            st.showing.clone()
        };
        Some(
            Card::new()
                .summary(format!("{showing} · {count} 張"))
                .table([
                    ("目前", showing),
                    ("圖片", format!("{count} 張")),
                    ("輪播", rotate),
                ])
                .buttons(vec![
                    Button::new("上一張", "/wallpaper prev"),
                    Button::new("下一張", "/wallpaper next"),
                    Button::new("預設", "/wallpaper set default"),
                ]),
        )
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert!(is_safe_name("sunset.jpg"));
        assert!(is_safe_name("山 景.PNG"));
        assert!(!is_safe_name("../etc/passwd.jpg"));
        assert!(!is_safe_name("a/b.jpg"));
        assert!(!is_safe_name(".hidden.jpg"));
        assert!(!is_safe_name("notes.txt"));
        assert!(!is_safe_name(""));
    }
}
