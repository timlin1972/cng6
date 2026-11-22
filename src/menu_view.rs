//! 選單視窗(modal): 以選單方式組出 CLI 指令。
//!
//! 選單只負責顯示與按鍵, 每一層有哪些項目由 `App` 依指令樹提供。

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// 執行目前組出的指令
    Exec,
    /// 選擇這個字, 進入下一層
    Child,
    /// 需要輸入的參數, 例如 `<x1>`
    Param,
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    pub desc: String,
    pub kind: ItemKind,
    /// 選了之後還有下一層
    pub more: bool,
}

/// 選單按鍵處理的結果, 交給 `App` 處理
pub enum MenuEvent {
    None,
    Close,
    Back,
    /// 選了第 n 個項目(Exec 或 Child)
    Select(usize),
    /// 輸入完一個參數
    Arg(String),
    /// 開啟選單的說明
    Help,
}

pub struct MenuView {
    /// 目前已選的字, 例如 ["panel", "show"]
    pub path: Vec<String>,
    items: Vec<MenuItem>,
    selected: usize,
    /// 參數輸入中: (參數名稱, 已輸入的文字)
    input: Option<(String, String)>,
    /// 清單捲動位置與可視行數
    offset: usize,
    page: usize,
}

impl MenuView {
    pub fn new() -> Self {
        Self {
            path: Vec::new(),
            items: Vec::new(),
            selected: 0,
            input: None,
            offset: 0,
            page: 10,
        }
    }

    pub fn set_items(&mut self, items: Vec<MenuItem>) {
        self.items = items;
        self.selected = 0;
        self.offset = 0;
        // 只剩一個參數可以選時, 直接進入輸入
        self.input = match self.items.as_slice() {
            [only] if only.kind == ItemKind::Param => Some((only.label.clone(), String::new())),
            _ => None,
        };
    }

    pub fn item(&self, i: usize) -> Option<&MenuItem> {
        self.items.get(i)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> MenuEvent {
        // 參數輸入模式
        if let Some((_, text)) = self.input.as_mut() {
            match key.code {
                KeyCode::Char(c) => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Enter if !text.trim().is_empty() => {
                    let text = text.trim().to_string();
                    self.input = None;
                    return MenuEvent::Arg(text);
                }
                KeyCode::Esc => {
                    self.input = None;
                    // 自動進入的輸入(只有一個參數), 取消就直接回上一層
                    if matches!(self.items.as_slice(), [only] if only.kind == ItemKind::Param) {
                        return MenuEvent::Back;
                    }
                }
                _ => {}
            }
            return MenuEvent::None;
        }

        let n = self.items.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return MenuEvent::Close,
            KeyCode::Char('?') => return MenuEvent::Help,
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.selected = (self.selected + n - 1) % n
            }
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.selected = (self.selected + 1) % n,
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = n.saturating_sub(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => return MenuEvent::Back,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                return self.choose(self.selected);
            }
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < n {
                    self.selected = i;
                    return self.choose(i);
                }
            }
            _ => {}
        }
        MenuEvent::None
    }

    fn choose(&mut self, i: usize) -> MenuEvent {
        match self.items.get(i) {
            Some(item) if item.kind == ItemKind::Param => {
                self.input = Some((item.label.clone(), String::new()));
                MenuEvent::None
            }
            Some(_) => MenuEvent::Select(i),
            None => MenuEvent::None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.path.is_empty() {
            " Menu ".to_string()
        } else {
            format!(" Menu: {} ", self.path.join(" › "))
        };
        let hint = if self.input.is_some() {
            " Enter 確認  Esc 取消 "
        } else {
            " ↑↓ 選擇  Enter 進入/執行  ← 上一層  Esc/F10 關閉  ? 說明 "
        };

        // 每一行: "n. label ›   desc"
        let label_w = self
            .items
            .iter()
            .map(|it| it.label.width() + if it.more { 2 } else { 0 })
            .max()
            .unwrap_or(0);
        let rows: Vec<(String, String, &MenuItem)> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let num = if i < 9 {
                    format!("{}.", i + 1)
                } else {
                    "  ".into()
                };
                let label = if it.more {
                    format!("{} ›", it.label)
                } else {
                    it.label.clone()
                };
                let pad = " ".repeat(label_w - label.width());
                (format!("{num} {label}{pad}"), it.desc.clone(), it)
            })
            .collect();
        let content_w = rows
            .iter()
            .map(|(l, d, _)| l.width() + 3 + d.width())
            .max()
            .unwrap_or(0);
        let input_rows = if self.input.is_some() { 2 } else { 0 };

        // 依內容大小置中
        let want_w = (content_w.max(title.width()).max(hint.width()) + 4) as u16;
        let want_h = (rows.len().max(1) + input_rows + 2) as u16;
        let w = want_w.min(area.width);
        let h = want_h.min(area.height);
        let rect = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(Color::Magenta))
            .padding(Padding::horizontal(1))
            .title(title)
            .title_bottom(hint);
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);

        // 清單(保持選取項目在可視範圍)
        self.page = (inner.height as usize).saturating_sub(input_rows).max(1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.page {
            self.offset = self.selected + 1 - self.page;
        }
        let mut lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(self.page)
            .map(|(i, (label, desc, it))| {
                let label_style = match it.kind {
                    ItemKind::Exec => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    ItemKind::Param => Style::default().fg(Color::Yellow),
                    ItemKind::Child => Style::default(),
                };
                let mut line = Line::from(vec![
                    Span::styled(label.clone(), label_style),
                    Span::raw("   "),
                    Span::styled(desc.clone(), Style::default().fg(Color::DarkGray)),
                ]);
                if i == self.selected {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                line
            })
            .collect();
        if rows.is_empty() {
            lines.push(Line::styled(
                "(沒有項目)",
                Style::default().fg(Color::DarkGray),
            ));
        }

        // 參數輸入列
        if let Some((name, text)) = &self.input {
            lines.push(Line::default());
            let prompt = format!("{name}: ");
            let cx = inner.x + (prompt.width() + text.width()) as u16;
            let cy = inner.y + lines.len() as u16;
            lines.push(Line::from(vec![
                Span::styled(prompt, Style::default().fg(Color::Yellow)),
                Span::raw(text.clone()),
            ]));
            if cy < inner.bottom() {
                frame.set_cursor_position((cx.min(inner.right().saturating_sub(1)), cy));
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
}
