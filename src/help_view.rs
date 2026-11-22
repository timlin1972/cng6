//! panel 說明視窗(modal): 依內容大小置中顯示, 開啟時無法切換到其他 panel。
//!
//! 在有 focus 的 panel 按 `?`, 或在 CLI 下 `panel help <name>` 開啟。

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

pub struct HelpView {
    title: String,
    lines: Vec<Line<'static>>,
    scroll: usize,
    /// 上次繪製時可視的行數
    page: usize,
}

impl HelpView {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            scroll: 0,
            page: 10,
        }
    }

    pub fn text(&mut self, text: impl Into<String>) {
        self.lines.push(Line::raw(text.into()));
    }

    pub fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    /// 區段標題
    pub fn heading(&mut self, text: impl Into<String>) {
        self.lines.push(Line::styled(
            text.into(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }

    /// 兩欄的表格, 左欄(按鍵/指令)會對齊
    pub fn table(&mut self, rows: &[(String, String)]) {
        if rows.is_empty() {
            self.lines
                .push(Line::styled("  (無)", Style::default().fg(Color::DarkGray)));
            return;
        }
        let width = rows.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
        for (k, d) in rows {
            let pad = " ".repeat(width - k.width());
            self.lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(k.clone(), Style::default().fg(Color::Yellow)),
                Span::raw(format!("{pad}   {d}")),
            ]));
        }
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.page)
    }

    /// 處理按鍵, 回傳 true 表示要關閉說明
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => return true,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.page),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll += self.page,
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = usize::MAX,
            _ => {}
        }
        self.scroll = self.scroll.min(self.max_scroll());
        false
    }

    /// 依內容大小置中顯示, 超出 `area` 時縮小並可捲動。
    /// `area` 通常是整個畫面。說明開啟時所有按鍵都由它處理, 所以無法切換到其他 panel。
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let title = format!(" 說明: {} ", self.title);
        // 內容寬高 + 邊框(2) + 左右留白(各 1)
        let content_w = self.lines.iter().map(Line::width).max().unwrap_or(0);
        let want_w = (content_w + 4).max(title.width() + 4) as u16;
        let want_h = self.lines.len() as u16 + 2;

        let scrollable = want_h > area.height;
        let hint = if scrollable {
            " Esc / q / ? 關閉   ↑↓ PgUp PgDn 捲動 "
        } else {
            " Esc / q / ? 關閉 "
        };
        let w = want_w.max(hint.width() as u16 + 4).min(area.width);
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
            .border_style(Style::default().fg(Color::Yellow))
            .padding(Padding::horizontal(1))
            .title(title)
            .title_bottom(hint);
        let inner = block.inner(rect);
        self.page = inner.height as usize;
        self.scroll = self.scroll.min(self.max_scroll());

        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(self.lines.clone())
                .block(block)
                .scroll((self.scroll as u16, 0)),
            rect,
        );
    }
}
