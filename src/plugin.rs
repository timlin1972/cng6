//! Plugin 介面定義。
//!
//! 每個 panel 都是一個 plugin。plugin 透過 [`Ctx`] 對系統提出 [`Action`]
//! (輸出訊息、執行指令、離開…), 系統再把 [`Event`] 廣播給各 plugin。

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::webui::{Card, WebMeta};

/// 訊息等級
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
    /// 使用者輸入的指令回顯
    Command,
}

/// panel 位置, 以 panel 區域(CLI 以上)的百分比表示:
/// 左上角 (x1, y1) 到右下角 (x2, y2), 範圍 0~100。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelRect {
    pub x1: u16,
    pub y1: u16,
    pub x2: u16,
    pub y2: u16,
}

impl PanelRect {
    pub const FULL: Self = Self {
        x1: 0,
        y1: 0,
        x2: 100,
        y2: 100,
    };

    pub fn new(x1: u16, y1: u16, x2: u16, y2: u16) -> Result<Self, String> {
        if x2 > 100 || y2 > 100 {
            return Err("座標範圍是 0~100".into());
        }
        if x1 >= x2 || y1 >= y2 {
            return Err("左上角必須在右下角的左上方 (x1 < x2, y1 < y2)".into());
        }
        Ok(Self { x1, y1, x2, y2 })
    }

    /// 從 `["x1", "y1", "x2", "y2"]` 解析
    pub fn parse(args: &[&str]) -> Result<Self, String> {
        let [x1, y1, x2, y2] = args else {
            return Err("需要 4 個數字: x1 y1 x2 y2".into());
        };
        let n = |s: &str| {
            s.parse::<u16>()
                .map_err(|_| format!("'{s}' 不是 0~100 的整數"))
        };
        Self::new(n(x1)?, n(y1)?, n(x2)?, n(y2)?)
    }

    /// 換算成實際的螢幕區域
    pub fn to_area(self, area: Rect) -> Rect {
        let x = |p: u16| area.x + (area.width as u32 * p as u32 / 100) as u16;
        let y = |p: u16| area.y + (area.height as u32 * p as u32 / 100) as u16;
        let (left, right, top, bottom) = (x(self.x1), x(self.x2), y(self.y1), y(self.y2));
        Rect::new(left, top, right - left, bottom - top)
    }
}

impl std::fmt::Display for PanelRect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} {} {}", self.x1, self.y1, self.x2, self.y2)
    }
}

/// 一筆設定(給 Settings plugin 與網頁顯示)
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettingRow {
    pub key: String,
    pub value: String,
    pub default: String,
    pub desc: String,
}

/// plugin 對系統提出的要求
#[derive(Debug, Clone)]
pub enum Action {
    /// 輸出一行(或多行)訊息到 Message
    Output(Level, String),
    /// 執行一行 CLI 指令
    Execute(String),
    /// 要求補完(參數為游標前的文字)
    Complete(String),
    /// 開啟選單視窗
    OpenMenu,
    /// 把一行指令加入 CLI 歷史(例如由選單執行的指令)
    AddHistory(String),
    /// 修改設定並寫入設定檔 (key, value)
    SetSetting(String, String),
    /// 設定恢復預設並寫入設定檔
    ResetSetting(String),
    /// 重新讀取設定檔
    LoadSettings,
    /// 寫入設定檔
    SaveSettings,
    /// 要求列出游標位置可用的指令(按下 `?`, 參數為游標前的文字)
    Describe(String),
    /// 結束程式
    Quit,
    /// 重新啟動程式(載入新編譯的版本)
    Restart,
}

/// 系統廣播給 plugin 的事件
#[derive(Debug, Clone)]
pub enum Event {
    /// 有訊息輸出(Message plugin 會收集它)
    Output(Level, String),
    /// CLI 所在層次改變, 空的表示最上層
    PathChanged(Vec<String>),
    /// 把一行指令加入 CLI 歷史
    AddHistory(String),
    /// 設定有變動, 附上所有設定的最新內容
    SettingsChanged(Vec<SettingRow>),
    /// 補完結果: 把游標前的文字 `before` 換成 `replacement`
    Completed { before: String, replacement: String },
}

/// plugin 執行時的 context, 用來收集 plugin 產生的 Action
#[derive(Default)]
pub struct Ctx {
    actions: Vec<Action>,
}

impl Ctx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.actions.push(Action::Output(Level::Info, msg.into()));
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.actions.push(Action::Output(Level::Warn, msg.into()));
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.actions.push(Action::Output(Level::Error, msg.into()));
    }

    pub fn execute(&mut self, cmd: impl Into<String>) {
        self.actions.push(Action::Execute(cmd.into()));
    }

    pub fn quit(&mut self) {
        self.actions.push(Action::Quit);
    }

    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    pub fn take(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
}

pub trait Plugin {
    /// plugin 名稱, 也是 CLI 指令的前綴, 例如 `message clear`
    fn name(&self) -> &str;

    /// 一行描述, 顯示在 `plugin list`
    fn description(&self) -> &str;

    /// 單獨輸入 plugin 名稱時是否進入該層。
    /// 回傳 false 時, 單獨輸入名稱會以空參數呼叫 [`Plugin::handle_command`]。
    fn is_group(&self) -> bool {
        true
    }

    /// 是否為一般 panel(可以 `panel show`、`focus`)。CLI 與彈出式的 plugin 回傳 false。
    fn is_panel(&self) -> bool {
        true
    }

    /// 這個 plugin 的子指令說明, 顯示在 `help <plugin>`
    fn help(&self) -> Vec<(&str, &str)> {
        Vec::new()
    }

    /// panel 有 focus 時可用的按鍵 (按鍵, 說明), 會顯示在 panel 說明畫面
    fn keys(&self) -> Vec<(&str, &str)> {
        Vec::new()
    }

    /// Tab 補完與 `?` 說明: `args` 是已輸入完成的子指令, 回傳下一個字的候選與描述。
    /// `<xxx>` 形式的候選表示參數, 只在 `?` 顯示, Tab 不會補它。
    /// 預設從 [`Plugin::help`] 推導, 需要動態候選(例如裝置名稱)時可覆寫。
    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        complete_from_help(&self.help(), args)
    }

    /// plugin 的設定項目 (key, 說明, 預設值)。完整的設定名稱是 `<plugin名稱>.<key>`,
    /// 值存在設定檔中, 啟動時與修改時會呼叫 [`Plugin::apply_setting`] 套用。
    fn settings(&self) -> Vec<(&str, &str, String)> {
        Vec::new()
    }

    /// 套用一個設定值; 值不合法時回傳錯誤訊息(不會被儲存)
    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        let _ = value;
        Err(format!("不支援的設定 '{key}'"))
    }

    /// panel 預設位置(百分比), 可用 `panel rect` 改變
    fn default_rect(&self) -> PanelRect {
        PanelRect::FULL
    }

    /// 處理 `<plugin名稱> <args...>` 形式的指令
    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let _ = args;
        ctx.warn(format!("{}: 沒有可用的子指令", self.name()));
    }

    /// 接收系統廣播的事件
    fn on_event(&mut self, event: &Event) {
        let _ = event;
    }

    /// 處理按鍵, 回傳 true 表示已處理(不再往下傳)。
    ///
    /// panel 有 focus 時會收到按鍵; focus 在 CLI 時, CLI 沒處理的按鍵(例如 PgUp)也會傳給顯示中的 panel。
    /// 注意 Esc、`?`、F10、Ctrl-N、Ctrl-P、Ctrl-C 是系統保留鍵, panel 收不到。
    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> bool {
        let _ = (key, ctx);
        false
    }

    /// 定時呼叫(約每 250ms), 用來更新資料
    fn on_tick(&mut self, ctx: &mut Ctx) {
        let _ = ctx;
    }

    /// 繪製 panel, `focused` 表示目前是否有 focus
    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool);

    // ---- 網頁(PC 桌面與手機), 見 `webui` ----

    /// 網頁上的名稱、圖示與顏色; None 時用 plugin 名稱與通用圖示
    fn web_meta(&self) -> Option<WebMeta> {
        None
    }

    /// 狀態卡片(由前端通用程式繪製); 有卡片的 plugin 會出現在 Dock 與手機總覽
    fn web_card(&self) -> Option<Card> {
        None
    }

    /// 自訂畫面的前端模組名稱(例如 "system"), 前端要有對應的 view
    fn web_view(&self) -> Option<&str> {
        None
    }
}

/// 從 help 表推導下一個字的候選。
///
/// 例如 help 有 `("history clear", "清除指令歷史")`, 那麼 `args = ["history"]`
/// 時會得到 `("clear", "清除指令歷史")`。只有當候選之後緊接著參數(或沒有字)時才附上描述。
pub fn complete_from_help(help: &[(&str, &str)], args: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (usage, desc) in help {
        let words: Vec<&str> = usage.split_whitespace().filter(|w| *w != "|").collect();
        if words.len() <= args.len() || words[..args.len()] != *args {
            continue;
        }
        let word = words[args.len()];
        let next_is_param = words.get(args.len() + 1).is_none_or(|w| is_placeholder(w));
        let desc = if next_is_param { *desc } else { "" };
        match out.iter_mut().find(|(w, _)| w == word) {
            Some(existing) if existing.1.is_empty() => existing.1 = desc.to_string(),
            Some(_) => {}
            None => out.push((word.to_string(), desc.to_string())),
        }
    }
    out
}

/// 是否為參數佔位(例如 `<name>`、`[cmd]`), 只用於說明, 不會被 Tab 補完
pub fn is_placeholder(word: &str) -> bool {
    word.starts_with('<') || word.starts_with('[')
}

/// panel 的外框, 有 focus 時加粗並變色, 讓所有 panel 外觀一致
pub fn panel_block(title: impl Into<String>, focused: bool) -> Block<'static> {
    let title = format!(" {} ", title.into());
    let block = Block::default().borders(Borders::ALL);
    if focused {
        block
            .title(format!("{title}● "))
            .border_type(BorderType::Thick)
            .border_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
    } else {
        block
            .title(title)
            .border_style(Style::default().fg(Color::Cyan))
    }
}

/// 解析設定的布林值(只接受 true / false, 讓設定檔的值保持一致)
pub fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("請輸入 true 或 false".into()),
    }
}

/// 解析設定的整數, 並檢查範圍
pub fn parse_usize(value: &str, min: usize, max: usize) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(format!("請輸入 {min}~{max} 的整數")),
    }
}
