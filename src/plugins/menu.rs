//! Menu plugin: 開啟選單視窗, 以選單方式輸入指令。
//!
//! 選單視窗本身由 `App` 繪製(它需要知道整個指令樹), 這個 plugin 提供指令與說明。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::plugin::{Action, Ctx, Plugin};

pub struct Menu;

impl Plugin for Menu {
    fn name(&self) -> &str {
        "menu"
    }

    fn description(&self) -> &str {
        "以選單方式輸入指令"
    }

    /// `menu` 直接開啟選單, 不是可以進入的一層
    fn is_group(&self) -> bool {
        false
    }

    /// 選單是彈出視窗, 不是一般 panel
    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![("open", "開啟選單 (直接輸入 menu 或按 F10 也可以)")]
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        vec![
            ("↑ / ↓ / k / j", "選擇項目"),
            ("Enter / → / l", "進入下一層或執行"),
            ("1 ~ 9", "直接選擇第 n 個項目"),
            ("← / h / Backspace", "回上一層"),
            ("Home / End", "第一個/最後一個項目"),
            ("Esc / q / F10", "關閉選單"),
            ("?", "顯示這個說明"),
        ]
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            [] | ["open"] => ctx.push(Action::OpenMenu),
            _ => ctx.error(format!("menu: 不認識的指令 '{}'", args.join(" "))),
        }
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}
