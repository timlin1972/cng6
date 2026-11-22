//! CLI plugin: 固定在畫面最下方, 只有一行輸入。

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::parse::in_quote;
use crate::plugin::{Action, Ctx, Event, Plugin, parse_bool, parse_usize};
use crate::webui::WebMeta;

/// 指令歷史檔(在目前目錄)
const HISTORY_FILE: &str = "cli_history.txt";

pub struct Cli {
    /// 提示字元, 會顯示目前所在層次, 例如 `panel> `
    prompt: String,
    /// 是否在最上層
    at_top: bool,
    input: Vec<char>,
    /// 游標位置(char index)
    cursor: usize,
    history: Vec<String>,
    /// 瀏覽歷史時的位置, None 表示正在編輯新的一行
    history_pos: Option<usize>,
    /// 開始瀏覽歷史前正在編輯的內容
    draft: Vec<char>,
    /// 水平捲動的起點(char index)
    scroll: usize,
    /// 設定 cli.history_max
    history_max: usize,
    /// 設定 cli.history_save
    history_save: bool,
    /// 是否已讀取過歷史檔(只讀一次)
    history_loaded: bool,
}

impl Cli {
    pub fn new() -> Self {
        Self {
            prompt: "> ".to_string(),
            at_top: true,
            input: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            draft: Vec::new(),
            scroll: 0,
            history_max: 500,
            history_save: false,
            history_loaded: false,
        }
    }

    fn set_input(&mut self, chars: Vec<char>) {
        self.input = chars;
        self.cursor = self.input.len();
    }

    fn submit(&mut self, ctx: &mut Ctx) {
        let line: String = self.input.iter().collect();
        self.input.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.history_pos = None;
        self.draft.clear();

        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        self.add_history(trimmed);
        ctx.execute(trimmed);
    }

    /// 加入歷史, 與上一筆相同時不重複加入
    fn add_history(&mut self, line: &str) {
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
            self.trim_history();
            self.save_history();
        }
        // 瀏覽歷史中途有新指令加入時, 重新從最新一筆開始
        self.history_pos = None;
    }

    /// 超過上限時移除最舊的
    fn trim_history(&mut self) {
        if self.history.len() > self.history_max {
            let n = self.history.len() - self.history_max;
            self.history.drain(..n);
        }
        self.history_pos = None;
    }

    /// 讀取歷史檔, 放在這次執行的歷史之前(只讀一次)
    fn load_history(&mut self) {
        if self.history_loaded {
            return;
        }
        self.history_loaded = true;
        if let Ok(content) = std::fs::read_to_string(HISTORY_FILE) {
            let mut old: Vec<String> = content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(String::from)
                .collect();
            old.append(&mut self.history);
            self.history = old;
            self.trim_history();
        }
    }

    /// 寫入歷史檔(有開啟 cli.history_save 時)。寫入失敗時忽略, 不影響操作。
    fn save_history(&self) {
        if self.history_save {
            let mut content = self.history.join("\n");
            content.push('\n');
            let _ = std::fs::write(HISTORY_FILE, content);
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => {
                self.draft = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.set_input(self.history[pos].chars().collect());
    }

    fn history_next(&mut self) {
        match self.history_pos {
            None => {}
            Some(p) if p + 1 < self.history.len() => {
                self.history_pos = Some(p + 1);
                self.set_input(self.history[p + 1].chars().collect());
            }
            Some(_) => {
                self.history_pos = None;
                let draft = std::mem::take(&mut self.draft);
                self.set_input(draft);
            }
        }
    }

    /// 刪除游標前的一個字(Ctrl-W)
    fn delete_word_back(&mut self) {
        let mut i = self.cursor;
        while i > 0 && self.input[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.input[i - 1].is_whitespace() {
            i -= 1;
        }
        self.input.drain(i..self.cursor);
        self.cursor = i;
    }

    fn width_of(chars: &[char]) -> usize {
        chars.iter().map(|c| c.width().unwrap_or(0)).sum()
    }
}

impl Plugin for Cli {
    fn name(&self) -> &str {
        "cli"
    }

    fn description(&self) -> &str {
        "指令輸入列(固定在最下方)"
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("history", "列出指令歷史"),
            ("history clear", "清除指令歷史"),
        ]
    }

    /// CLI 固定在最下方, 不受 `panel` 指令控制
    fn is_panel(&self) -> bool {
        false
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        vec![
            ("Enter", "執行指令"),
            ("?", "列出游標位置可用的指令 (在引號內則輸入 ?)"),
            ("Tab", "補完"),
            ("↑ / ↓", "指令歷史"),
            ("← / → / Home / End", "移動游標"),
            ("Ctrl-A / Ctrl-E", "移到行首/行尾"),
            ("Ctrl-U / Ctrl-K", "刪除游標前/後的內容"),
            ("Ctrl-W", "刪除游標前的一個字"),
            ("Esc", "清除輸入; 沒有輸入時回上一層"),
        ]
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            (
                "history_save",
                "指令歷史存到 cli_history.txt, 重新啟動後仍可用 ↑ 叫出 (true/false)",
                "true".into(),
            ),
            (
                "history_max",
                "指令歷史最多保留幾筆, 10~100000",
                "500".into(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "history_save" => {
                self.history_save = parse_bool(value)?;
                if self.history_save {
                    self.load_history();
                    self.save_history();
                }
            }
            "history_max" => {
                self.history_max = parse_usize(value, 10, 100_000)?;
                self.trim_history();
                self.save_history();
            }
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["history"] => {
                if self.history.is_empty() {
                    ctx.info("(沒有歷史紀錄)");
                }
                for (i, h) in self.history.iter().enumerate() {
                    ctx.info(format!("{:>4}  {}", i + 1, h));
                }
            }
            ["history", "clear"] => {
                self.history.clear();
                self.history_pos = None;
                self.save_history();
                ctx.info("指令歷史已清除");
            }
            _ => ctx.error(format!("cli: 不認識的指令 '{}'", args.join(" "))),
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.len(),
            KeyCode::Char('u') if ctrl => {
                self.input.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => self.input.truncate(self.cursor),
            KeyCode::Char('w') if ctrl => self.delete_word_back(),
            KeyCode::Char(_) if ctrl => return false,
            // 像 Cisco 一樣, `?` 立即列出可用指令, 不會插入輸入列;
            // 但在引號內(例如 `echo "what?`)就當作一般字元
            KeyCode::Char('?')
                if !in_quote(&self.input[..self.cursor].iter().collect::<String>()) =>
            {
                let before: String = self.input[..self.cursor].iter().collect();
                ctx.push(Action::Describe(before));
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Enter => self.submit(ctx),
            KeyCode::Tab => {
                let before: String = self.input[..self.cursor].iter().collect();
                ctx.push(Action::Complete(before));
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            KeyCode::Esc => {
                // 有輸入時清除輸入, 沒輸入時回上一層
                if self.input.is_empty() && !self.at_top {
                    ctx.execute("..");
                }
                self.input.clear();
                self.cursor = 0;
                self.history_pos = None;
            }
            _ => return false,
        }
        true
    }

    fn on_event(&mut self, event: &Event) {
        if let Event::AddHistory(line) = event {
            self.add_history(line);
        }
        if let Event::PathChanged(path) = event {
            self.at_top = path.is_empty();
            self.prompt = format!("{}> ", path.join(" "));
        }
        if let Event::Completed {
            before,
            replacement,
        } = event
        {
            // 確認輸入在補完期間沒有變動
            let current: String = self.input[..self.cursor].iter().collect();
            if current == *before {
                let mut chars: Vec<char> = replacement.chars().collect();
                let n = chars.len();
                chars.extend_from_slice(&self.input[self.cursor..]);
                self.input = chars;
                self.cursor = n;
            }
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("CLI", "cli", "dark"))
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let prompt_w = self.prompt.width() as u16;
        let avail = area.width.saturating_sub(prompt_w).max(1) as usize;

        // 調整水平捲動, 讓游標保持在可視範圍內
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        while Self::width_of(&self.input[self.scroll..self.cursor]) >= avail {
            self.scroll += 1;
        }

        let prompt_style = if focused {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let prompt = Span::styled(self.prompt.as_str(), prompt_style);
        let line = if !focused && self.input.is_empty() {
            Line::from(vec![
                prompt,
                Span::styled(
                    "Esc 回到 CLI, Ctrl-N/Ctrl-P 切換 focus",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else if self.input.is_empty() {
            Line::from(vec![
                prompt,
                Span::styled(
                    "輸入指令, 'help' 查看說明",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else {
            let visible: String = self.input[self.scroll..].iter().collect();
            Line::from(vec![prompt, Span::raw(visible)])
        };
        frame.render_widget(Paragraph::new(line), area);
        if !focused {
            return;
        }

        let cx = area.x + prompt_w + Self::width_of(&self.input[self.scroll..self.cursor]) as u16;
        frame.set_cursor_position((cx.min(area.right().saturating_sub(1)), area.y));
    }
}
