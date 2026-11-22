//! Settings plugin: 查看與修改系統設定(存在設定檔中)。
//!
//! 設定的定義與套用由 `App` 負責(它知道所有 plugin), 這個 plugin 透過
//! [`Event::SettingsChanged`] 取得最新的設定表, 並以 Action / 指令要求修改。

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::plugin::{
    Action, Ctx, Event, PanelRect, Plugin, SettingRow, complete_from_help, panel_block,
};

pub struct SettingsPanel {
    rows: Vec<SettingRow>,
    selected: usize,
    offset: usize,
    /// 編輯中的新值
    editing: Option<String>,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            offset: 0,
            editing: None,
        }
    }

    fn find(&self, key: &str) -> Option<&SettingRow> {
        self.rows.iter().find(|r| r.key == key)
    }
}

impl Plugin for SettingsPanel {
    fn name(&self) -> &str {
        "settings"
    }

    fn description(&self) -> &str {
        "查看/修改系統設定"
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("list", "列出所有設定"),
            ("get <key>", "查看設定"),
            ("set <key> <value>", "修改設定並寫入設定檔"),
            ("reset <key>", "設定恢復預設並寫入設定檔"),
            ("load", "重新讀取設定檔"),
            ("save", "寫入設定檔"),
        ]
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        vec![
            ("↑ / ↓ / k / j", "選擇設定"),
            ("Enter / e", "編輯選擇的設定, 再按 Enter 套用"),
            ("Backspace", "編輯時刪除一個字"),
            ("r", "選擇的設定恢復預設"),
        ]
    }

    fn default_rect(&self) -> PanelRect {
        PanelRect {
            x1: 50,
            y1: 0,
            x2: 100,
            y2: 50,
        }
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["get" | "set" | "reset"] => self
                .rows
                .iter()
                .map(|r| (r.key.clone(), r.desc.clone()))
                .collect(),
            ["set", key, ..] => match self.find(key) {
                Some(r) => vec![(
                    "<value>".into(),
                    format!("目前: {}  預設: {}", r.value, r.default),
                )],
                None => Vec::new(),
            },
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["list"] => {
                let w = self.rows.iter().map(|r| r.key.width()).max().unwrap_or(0);
                for r in &self.rows {
                    let mark = if r.value != r.default { " *" } else { "" };
                    let pad = " ".repeat(w - r.key.width());
                    ctx.info(format!("  {}{pad}  {}{mark}", r.key, r.value));
                }
                ctx.info("  (* 表示和預設值不同)");
            }
            ["get", key] => match self.find(key) {
                Some(r) => {
                    ctx.info(format!("{} = {}", r.key, r.value));
                    ctx.info(format!("  {}  (預設: {})", r.desc, r.default));
                }
                None => ctx.error(format!("沒有設定 '{key}'")),
            },
            ["set", key, value @ ..] if !value.is_empty() => {
                ctx.push(Action::SetSetting(key.to_string(), value.join(" ")));
            }
            ["reset", key] => ctx.push(Action::ResetSetting(key.to_string())),
            ["load"] => ctx.push(Action::LoadSettings),
            ["save"] => ctx.push(Action::SaveSettings),
            _ => ctx.error("用法: 輸入 'help settings' 查看"),
        }
    }

    fn on_event(&mut self, event: &Event) {
        if let Event::SettingsChanged(rows) = event {
            self.rows = rows.clone();
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> bool {
        // 編輯模式
        if let Some(text) = self.editing.as_mut() {
            match key.code {
                KeyCode::Char(c) => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Enter => {
                    let value = text.trim().to_string();
                    self.editing = None;
                    if let Some(r) = self.rows.get(self.selected)
                        && !value.is_empty()
                        && value != r.value
                    {
                        ctx.execute(format!("/settings set {} {value}", r.key));
                    }
                }
                _ => return false,
            }
            return true;
        }

        let n = self.rows.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.selected = (self.selected + n - 1) % n;
            }
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.selected = (self.selected + 1) % n;
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                if let Some(r) = self.rows.get(self.selected) {
                    self.editing = Some(r.value.clone());
                }
            }
            KeyCode::Char('r') => {
                if let Some(r) = self.rows.get(self.selected)
                    && r.value != r.default
                {
                    ctx.execute(format!("/settings reset {}", r.key));
                }
            }
            _ => return false,
        }
        true
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        // 失去 focus 時取消編輯(Esc 是系統保留鍵, 會直接把 focus 交回 CLI)
        if !focused {
            self.editing = None;
        }
        let block = panel_block("Settings", focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // 底部保留 2 行: 說明 / 編輯列
        let list_h = (inner.height as usize).saturating_sub(2).max(1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + list_h {
            self.offset = self.selected + 1 - list_h;
        }

        let key_w = self.rows.iter().map(|r| r.key.width()).max().unwrap_or(0);
        let mut lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(list_h)
            .map(|(i, r)| {
                let changed = r.value != r.default;
                let pad = " ".repeat(key_w - r.key.width());
                let value_style = if changed {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                let mut line = Line::from(vec![
                    Span::styled(format!("{}{pad}", r.key), Style::default().fg(Color::Cyan)),
                    Span::raw("  "),
                    Span::styled(r.value.clone(), value_style),
                    Span::raw(if changed { " *" } else { "" }),
                ]);
                if focused && i == self.selected {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                line
            })
            .collect();
        if self.rows.is_empty() {
            lines.push(Line::styled(
                "(沒有設定)",
                Style::default().fg(Color::DarkGray),
            ));
        }
        while lines.len() < list_h {
            lines.push(Line::default());
        }

        // 說明 / 編輯列
        let selected = self.rows.get(self.selected);
        if let (Some(text), Some(r)) = (&self.editing, selected) {
            lines.push(Line::styled(
                format!("編輯 {} (預設: {})", r.key, r.default),
                Style::default().fg(Color::DarkGray),
            ));
            let prompt = "新值: ";
            lines.push(Line::from(vec![
                Span::styled(prompt, Style::default().fg(Color::Yellow)),
                Span::raw(text.clone()),
            ]));
            let cx = inner.x + (prompt.width() + text.width()) as u16;
            let cy = inner.y + lines.len() as u16 - 1;
            if cy < inner.bottom() {
                frame.set_cursor_position((cx.min(inner.right().saturating_sub(1)), cy));
            }
        } else if let Some(r) = selected.filter(|_| focused) {
            lines.push(Line::styled(
                r.desc.clone(),
                Style::default().fg(Color::DarkGray),
            ));
            lines.push(Line::styled(
                format!("預設: {}   Enter 編輯  r 恢復預設  ? 說明", r.default),
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}
