//! Message plugin: 收集所有指令輸出。即使 panel 沒顯示, 訊息仍會被保存。

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::plugin::{Ctx, Event, Level, PanelRect, Plugin, panel_block, parse_bool, parse_usize};
use crate::webui::WebMeta;

pub struct Message {
    /// (等級, 時間 HH:MM:SS, 內容); 時間一律記錄, 是否顯示由 message.timestamp 決定
    lines: Vec<(Level, String, String)>,
    /// 設定 message.max_lines
    max_lines: usize,
    /// 設定 message.timestamp
    timestamp: bool,
    /// 從底部往上捲的行數, 0 表示跟著最新訊息
    scroll_back: usize,
    /// 上次繪製時可視的行數(給 PageUp/PageDown 用)
    page: usize,
}

impl Message {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            max_lines: 5000,
            timestamp: false,
            scroll_back: 0,
            page: 10,
        }
    }

    fn push(&mut self, level: Level, text: &str) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        for l in text.split('\n') {
            self.lines.push((level, time.clone(), l.to_string()));
            // 使用者往上捲時, 新訊息進來不要讓畫面跳動
            if self.scroll_back > 0 {
                self.scroll_back += 1;
            }
        }
        self.trim();
    }

    /// 超過上限時移除最舊的
    fn trim(&mut self) {
        if self.lines.len() > self.max_lines {
            let n = self.lines.len() - self.max_lines;
            self.lines.drain(..n);
        }
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.page)
    }
}

impl Plugin for Message {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "顯示所有指令的輸出"
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![("clear", "清除所有訊息")]
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        vec![
            ("↑ / k", "往上捲一行"),
            ("↓ / j", "往下捲一行"),
            ("PgUp / PgDn", "往上/往下捲一頁 (focus 在 CLI 時也可用)"),
            ("Home / g", "捲到最上面"),
            ("End / G", "捲到最下面 (跟著最新訊息)"),
        ]
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            (
                "max_lines",
                "Message 最多保留幾行, 100~1000000",
                "5000".into(),
            ),
            (
                "timestamp",
                "每行訊息前顯示時間 (true/false)",
                "false".into(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "max_lines" => {
                self.max_lines = parse_usize(value, 100, 1_000_000)?;
                self.trim();
            }
            "timestamp" => self.timestamp = parse_bool(value)?,
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn default_rect(&self) -> PanelRect {
        PanelRect {
            x1: 0,
            y1: 50,
            x2: 100,
            y2: 100,
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["clear"] => {
                self.lines.clear();
                self.scroll_back = 0;
            }
            _ => ctx.error(format!("message: 不認識的指令 '{}'", args.join(" "))),
        }
    }

    fn on_event(&mut self, event: &Event) {
        if let Event::Output(level, text) = event {
            self.push(*level, text);
        }
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> bool {
        match key.code {
            KeyCode::PageUp => {
                self.scroll_back = (self.scroll_back + self.page.max(1)).min(self.max_scroll());
            }
            KeyCode::PageDown => {
                self.scroll_back = self.scroll_back.saturating_sub(self.page.max(1));
            }
            // 以下按鍵只有 Message 有 focus 時才會收到(focus 在 CLI 時被 CLI 用掉)
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_back = (self.scroll_back + 1).min(self.max_scroll());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_back = self.scroll_back.saturating_sub(1);
            }
            KeyCode::Home | KeyCode::Char('g') => self.scroll_back = self.max_scroll(),
            KeyCode::End | KeyCode::Char('G') => self.scroll_back = 0,
            _ => return false,
        }
        true
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("Message", "message", "blue"))
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let title = if self.scroll_back > 0 {
            format!("Message (↑{})", self.scroll_back)
        } else {
            "Message".to_string()
        };
        let block = panel_block(title, focused);
        let inner = block.inner(area);
        self.page = inner.height as usize;
        self.scroll_back = self.scroll_back.min(self.max_scroll());

        let end = self.lines.len() - self.scroll_back;
        let start = end.saturating_sub(self.page);
        let lines: Vec<Line> = self.lines[start..end]
            .iter()
            .map(|(level, time, text)| {
                let style = match level {
                    Level::Info => Style::default(),
                    Level::Warn => Style::default().fg(Color::Yellow),
                    Level::Error => Style::default().fg(Color::Red),
                    Level::Command => Style::default().fg(Color::Green),
                };
                let mut spans = Vec::new();
                if self.timestamp {
                    spans.push(Span::styled(
                        format!("{time} "),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                spans.push(Span::styled(text.clone(), style));
                Line::from(spans)
            })
            .collect();

        frame.render_widget(Paragraph::new(lines).block(block), area);
    }
}
