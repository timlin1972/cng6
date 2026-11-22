//! App: 管理 plugin、panel 顯示狀態、指令分派與版面配置。
//!
//! CLI 是多層次的: 單獨輸入一個群組名稱(例如 `panel`)就會進入該層,
//! 之後輸入的指令都相對於該層解讀(`toggle message` 等同 `panel toggle message`)。

use std::collections::{HashMap, VecDeque};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Clear;

use unicode_width::UnicodeWidthStr;

use crate::help_view::HelpView;
use crate::menu_view::{ItemKind, MenuEvent, MenuItem, MenuView};
use crate::parse::{quote_arg, split_args};
use crate::plugin::{
    Action, Ctx, Event, Level, PanelRect, Plugin, SettingRow, complete_from_help, is_placeholder,
};
use crate::plugins::{
    cli::Cli,
    menu::Menu,
    message::Message,
    music::{self, Music},
    settings::SettingsPanel,
    system::SystemInfo,
    wallpaper::{self, Wallpaper},
    weather::{self, Weather},
    web::Web,
    worldclock::{self, WorldClock},
};
use crate::remote::{self, Candidate, Op, Reply, Request};
use crate::settings::Settings;
use crate::webui::{AppInfo, WebMeta};

const CLI: &str = "cli";
/// 系統設定檔
const SETTINGS_FILE: &str = "settings.conf";
/// 單次處理最多執行的指令數, 防止 script 互相 source 造成無窮迴圈
const MAX_EXECUTE: usize = 10_000;

/// 只能在 TUI 使用的指令(網頁 CLI 不能執行, 也不會出現在補完)
const TUI_ONLY: &[&str] = &["panel", "focus", "menu", "cli", "message"];

/// 最多記住幾個遠端連線的層次
const MAX_SESSIONS: usize = 64;

/// 核心(非 plugin)的指令群組
const CORE_GROUPS: &[(&str, &str)] = &[("panel", "panel 顯示控制"), ("plugin", "plugin 資訊")];

/// 在任何層次都可以使用的指令
const GLOBAL_HELP: &[(&str, &str)] = &[
    ("help [cmd]", "顯示說明"),
    ("?", "(按鍵) 立即列出游標位置可用的指令"),
    ("..", "回上一層"),
    ("exit", "回上一層, 在最上層時離開程式"),
    ("/", "回最上層"),
    ("/<cmd>", "從最上層執行指令, 例如 /panel list"),
    ("quit | q", "離開程式 (Ctrl-C 亦可)"),
    ("restart", "重新啟動 cng6, 載入新編譯的版本"),
    ("echo <text>", "輸出文字"),
    ("source <file>", "執行 script 檔"),
];

/// 最上層的單一指令(非群組)
const TOP_CMDS: &[(&str, &str)] = &[("focus [name]", "查看/切換 focus 到 panel 或 cli")];

/// 按鍵說明
const KEY_HELP: &[(&str, &str)] = &[
    ("Esc", "panel 有 focus 時: 回到 CLI; CLI: 清除輸入/回上一層"),
    (
        "Ctrl-N / Ctrl-P",
        "切換 focus 到下一個/上一個 panel (含 CLI)",
    ),
    (
        "?",
        "CLI: 列出游標位置可用的指令; panel: 顯示該 panel 的說明",
    ),
    ("Tab", "補完"),
    ("↑ / ↓", "指令歷史"),
    ("PgUp / PgDn", "捲動 Message"),
    ("F10", "開啟/關閉選單"),
    ("Ctrl-C", "離開程式"),
];

/// 全域指令(給 `?` 與 Tab 使用)
const GLOBAL_CMDS: &[(&str, &str)] = &[
    ("help", "顯示說明"),
    ("..", "回上一層"),
    ("exit", "回上一層, 在最上層時離開程式"),
    ("quit", "離開程式"),
    ("restart", "重新啟動 cng6, 載入新編譯的版本"),
    ("echo", "輸出文字"),
    ("source", "執行 script 檔"),
];

const PANEL_HELP: &[(&str, &str)] = &[
    ("list", "列出所有 panel 的顯示狀態與位置"),
    ("show <name> [x1 y1 x2 y2]", "顯示 panel, 可同時指定位置"),
    ("hide <name>", "隱藏 panel"),
    (
        "toggle <name> [x1 y1 x2 y2]",
        "切換 panel 顯示, 可同時指定位置",
    ),
    ("rect <name> [x1 y1 x2 y2]", "查看/設定 panel 位置"),
    ("reset <name>", "panel 位置恢復預設"),
    ("help <name>", "顯示 panel 的使用說明視窗"),
];
/// 位置參數的說明(給 `?` 使用)
const RECT_PARAMS: &[(&str, &str)] = &[
    ("<x1>", "左上角 x (0~100%)"),
    ("<y1>", "左上角 y (0~100%)"),
    ("<x2>", "右下角 x (0~100%)"),
    ("<y2>", "右下角 y (0~100%)"),
];
const PLUGIN_HELP: &[(&str, &str)] = &[("list", "列出所有 plugin")];

pub struct App {
    plugins: Vec<Box<dyn Plugin>>,
    /// 目前顯示的 panel(依顯示順序)
    visible: Vec<String>,
    /// 用指令設定過的 panel 位置, 沒有設定的使用 plugin 的預設值
    rects: HashMap<String, PanelRect>,
    /// 目前有 focus 的 panel, None 表示 CLI
    focus: Option<String>,
    /// 開啟中的說明視窗(modal)
    help_view: Option<HelpView>,
    /// 開啟中的選單視窗(modal)
    menu: Option<MenuView>,
    /// 系統設定(設定檔)
    settings: Settings,
    /// CLI 目前所在的層次, 空的表示最上層
    path: Vec<String>,
    running: bool,
    /// restart 指令: 結束後重新啟動
    restart: bool,
    /// 網頁 CLI 送來的請求
    remote_rx: remote::Receiver,
    /// 每個遠端連線所在的層次
    sessions: HashMap<String, Vec<String>>,
    /// 執行遠端請求時, 收集輸出(不送到 TUI)
    capture: Option<Reply>,
}

impl App {
    pub fn new() -> Self {
        let (remote_tx, remote_rx) = remote::channel();
        let mut app = Self {
            plugins: Vec::new(),
            visible: Vec::new(),
            rects: HashMap::new(),
            focus: None,
            help_view: None,
            menu: None,
            settings: Settings::new(SETTINGS_FILE),
            path: Vec::new(),
            running: true,
            restart: false,
            remote_rx,
            sessions: HashMap::new(),
            capture: None,
        };
        // 預設必備的 plugin
        app.register(Box::new(Cli::new()));
        app.register(Box::new(Message::new()));
        // 其他 plugin
        app.register(Box::new(SystemInfo::new()));
        app.register(Box::new(SettingsPanel::new()));
        app.register(Box::new(Menu));
        // 桌布: plugin 負責設定與指令, web 讀取同一份狀態
        let wallpaper = wallpaper::Shared::new();
        app.register(Box::new(Wallpaper::new(wallpaper.clone())));
        // 音樂: plugin 負責下載與檔案管理, web 提供播放
        let music = music::Shared::new();
        app.register(Box::new(Music::new(music.clone())));
        // 天氣: plugin 在背景更新, web 讀取同一份資料
        let weather = weather::Shared::new();
        app.register(Box::new(Weather::new(weather.clone())));
        let worldclock = worldclock::Shared::new();
        app.register(Box::new(WorldClock::new(worldclock.clone())));
        app.register(Box::new(Web::new(
            remote_tx,
            crate::plugins::web::server::Services {
                wallpaper,
                music,
                weather,
                worldclock,
            },
        )));
        app
    }

    /// 註冊新的 plugin。新功能只要實作 `Plugin` 再在這裡註冊即可。
    /// 每個 plugin 的名稱自動成為一個 CLI 群組(層次)。
    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// 結束時是否要重新啟動(restart 指令)
    pub fn restart_requested(&self) -> bool {
        self.restart
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.plugins.iter().position(|p| p.name() == name)
    }

    fn broadcast(&mut self, ev: &Event) {
        // 執行遠端請求時: 輸出與補完結果回給網頁, 層次與歷史不影響 TUI
        if let Some(cap) = self.capture.as_mut() {
            match ev {
                Event::Output(level, text) => return cap.output.push((*level, text.clone())),
                Event::Completed { replacement, .. } => {
                    cap.replacement = Some(replacement.clone());
                    return;
                }
                Event::PathChanged(_) | Event::AddHistory(_) => return,
                _ => {}
            }
        }
        for p in self.plugins.iter_mut() {
            p.on_event(ev);
        }
    }

    /// 是否為可以進入的群組
    fn is_group(&self, path: &[&str]) -> bool {
        match path {
            [name] => {
                CORE_GROUPS.iter().any(|(g, _)| g == name)
                    || self.find(name).is_some_and(|i| self.plugins[i].is_group())
            }
            _ => false,
        }
    }

    /// 是否為一般 panel(可以顯示、focus)
    fn is_panel(&self, name: &str) -> bool {
        self.find(name).is_some_and(|i| self.plugins[i].is_panel())
    }

    /// panel 目前的位置
    /// 優先順序: `panel rect` 指令設定的位置 > 設定檔的預設位置 > plugin 的預設位置
    fn rect_of(&self, name: &str) -> PanelRect {
        if let Some(r) = self.rects.get(name) {
            return *r;
        }
        self.default_rect_of(name)
    }

    /// panel 的預設位置(設定檔 > plugin 的預設位置), `panel reset` 會回到這個位置
    fn default_rect_of(&self, name: &str) -> PanelRect {
        let from_settings = self
            .settings
            .get(&format!("panel.{name}.rect"))
            .and_then(|v| PanelRect::parse(&v.split_whitespace().collect::<Vec<_>>()).ok());
        from_settings.unwrap_or_else(|| {
            self.find(name)
                .map(|i| self.plugins[i].default_rect())
                .unwrap_or(PanelRect::FULL)
        })
    }

    fn set_path(&mut self, path: Vec<String>) {
        self.path = path;
        let ev = Event::PathChanged(self.path.clone());
        self.broadcast(&ev);
    }

    fn prompt(&self) -> String {
        format!("{}>", self.path.join(" "))
    }

    // ---------------------------------------------------------------- input

    pub fn on_key(&mut self, key: KeyEvent) {
        let mut ctx = Ctx::new();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // 系統保留鍵, 不會傳給 plugin
        match key.code {
            KeyCode::Char('c') if ctrl => {
                ctx.execute("quit");
                self.process(ctx.take());
                return;
            }
            _ if self.help_view.is_some() => {
                // 說明畫面開啟時, 按鍵都給它
                if self.help_view.as_mut().is_some_and(|h| h.on_key(key)) {
                    self.help_view = None;
                }
                return;
            }
            // F10: 開啟/關閉選單(說明視窗開著時除外)
            KeyCode::F(10) if self.menu.is_some() => {
                self.menu = None;
                return;
            }
            KeyCode::F(10) => {
                self.process(vec![Action::OpenMenu]);
                return;
            }
            _ if self.menu.is_some() => {
                // 選單開啟時, 按鍵都給它
                self.menu_key(key);
                return;
            }
            KeyCode::Char('?') if self.focus.is_some() => {
                let name = self.focus.clone().unwrap_or_default();
                self.help_view = Some(self.build_help(&name));
                return;
            }
            KeyCode::Char('n') if ctrl => return self.cycle_focus(1),
            KeyCode::Char('p') if ctrl => return self.cycle_focus(-1),
            KeyCode::Esc if self.focus.is_some() => {
                self.focus = None;
                return;
            }
            _ => {}
        }

        match self.focus.clone() {
            // panel 有 focus: 只有該 panel 收到按鍵
            Some(name) => {
                if let Some(i) = self.find(&name) {
                    self.plugins[i].on_key(key, &mut ctx);
                }
            }
            // CLI 有 focus: CLI 優先處理, 沒處理的按鍵(例如 PgUp)再交給顯示中的 panel
            None => {
                let order = std::iter::once(CLI.to_string()).chain(self.visible.iter().cloned());
                for name in order.collect::<Vec<_>>() {
                    if let Some(i) = self.find(&name)
                        && self.plugins[i].on_key(key, &mut ctx)
                    {
                        break;
                    }
                }
            }
        }
        self.process(ctx.take());
    }

    /// 依序切換 focus: CLI → 顯示中的 panel(依顯示順序) → CLI
    fn cycle_focus(&mut self, step: isize) {
        let mut ring: Vec<Option<String>> = vec![None];
        ring.extend(self.visible.iter().cloned().map(Some));
        let cur = ring.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        let next = (cur + step).rem_euclid(ring.len() as isize) as usize;
        self.focus = ring[next].clone();
    }

    /// 定時呼叫所有 plugin 的 on_tick
    pub fn tick(&mut self) {
        let mut ctx = Ctx::new();
        for p in self.plugins.iter_mut() {
            p.on_tick(&mut ctx);
        }
        self.process(ctx.take());
    }

    /// 執行一行指令(例如程式啟動時執行 startup script)
    pub fn execute(&mut self, line: &str) {
        self.process(vec![Action::Execute(line.to_string())]);
    }

    /// 處理網頁 CLI 送來的請求, 有處理任何請求時回傳 true(需要重繪)
    pub fn handle_remote(&mut self) -> bool {
        let mut any = false;
        while let Ok(req) = self.remote_rx.try_recv() {
            any = true;
            self.run_remote(req);
        }
        any
    }

    fn run_remote(&mut self, req: Request) {
        let session_path = self.sessions.get(&req.session).cloned().unwrap_or_default();
        if req.op == Op::Exec && !req.line.trim().is_empty() {
            // TUI 的 Message 記一行, 知道有人從網頁下了指令
            let prompt = format!("{}>", session_path.join(" "));
            self.broadcast(&Event::Output(
                Level::Info,
                format!("web {}: {prompt} {}", req.peer, req.line.trim()),
            ));
        }

        let query = req.line.clone();
        // 換成這個連線的層次執行, 結束後換回 TUI 的層次
        let tui_path = std::mem::replace(&mut self.path, session_path);
        self.capture = Some(Reply::default());
        let action = match req.op {
            // 空的 exec 只用來查詢目前層次, 不執行
            Op::Exec if req.line.trim().is_empty() => None,
            Op::Exec => Some(Action::Execute(req.line)),
            Op::Complete => Some(Action::Complete(req.line)),
            Op::Describe => Some(Action::Describe(req.line)),
            // 不執行指令, 下面直接附上結果
            Op::Settings | Op::Apps | Op::Candidates => None,
        };
        if let Some(action) = action {
            self.process(vec![action]);
        }
        // 候選要在這個連線的層次下計算(網頁不列出 TUI 專用指令)
        let candidates = (req.op == Op::Candidates).then(|| self.web_candidates(&query));
        let mut reply = self.capture.take().unwrap_or_default();
        reply.candidates = candidates;
        reply.path = std::mem::replace(&mut self.path, tui_path);
        match req.op {
            Op::Settings => reply.settings = Some(self.setting_rows()),
            Op::Apps => reply.apps = Some(self.web_apps()),
            _ => {}
        }

        if self.sessions.len() >= MAX_SESSIONS && !self.sessions.contains_key(&req.session) {
            self.sessions.clear();
        }
        self.sessions.insert(req.session, reply.path.clone());
        // 網頁可能已經關閉, 送不到也沒關係
        let _ = req.reply.send(reply);
    }

    fn process(&mut self, actions: Vec<Action>) {
        let mut queue: VecDeque<Action> = actions.into();
        let mut executed = 0;
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Output(level, text) => self.broadcast(&Event::Output(level, text)),
                Action::Execute(line) => {
                    executed += 1;
                    if executed > MAX_EXECUTE {
                        queue.retain(|a| !matches!(a, Action::Execute(_)));
                        queue.push_front(Action::Output(
                            Level::Error,
                            format!(
                                "執行超過 {MAX_EXECUTE} 個指令, 已中止(script 是否互相 source?)"
                            ),
                        ));
                        continue;
                    }
                    let mut ctx = Ctx::new();
                    ctx.push(Action::Output(
                        Level::Command,
                        format!("{} {line}", self.prompt()),
                    ));
                    self.run_line(&line, &mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::Complete(before) => {
                    let mut ctx = Ctx::new();
                    self.complete(&before, &mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::Describe(before) => {
                    let mut ctx = Ctx::new();
                    self.describe(&before, &mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::AddHistory(line) => self.broadcast(&Event::AddHistory(line)),
                Action::SetSetting(key, value) => {
                    let mut ctx = Ctx::new();
                    self.set_setting(&key, Some(&value), &mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::ResetSetting(key) => {
                    let mut ctx = Ctx::new();
                    self.set_setting(&key, None, &mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::LoadSettings => {
                    let mut ctx = Ctx::new();
                    self.reload_settings(&mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::SaveSettings => {
                    let mut ctx = Ctx::new();
                    self.save_settings(&mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::OpenMenu | Action::Quit if self.capture.is_some() => {
                    self.broadcast(&Event::Output(
                        Level::Error,
                        "這個指令只能在 TUI 使用".into(),
                    ));
                }
                Action::OpenMenu => {
                    self.menu = Some(MenuView::new());
                    let mut ctx = Ctx::new();
                    self.menu_refresh(&mut ctx);
                    requeue(&mut queue, ctx);
                }
                Action::Quit => self.running = false,
                Action::Restart => {
                    self.restart = true;
                    self.running = false;
                }
            }
        }
    }

    // ----------------------------------------------------------- completion

    /// 計算游標位置的候選。回傳 (正在輸入的字, 候選與描述), 候選已依前綴過濾。
    fn candidates<'a>(&self, before: &'a str) -> (&'a str, Vec<(String, String)>) {
        let (absolute, rest) = match before.trim_start().strip_prefix('/') {
            Some(r) => (true, r),
            None => (false, before.trim_start()),
        };
        let mut words: Vec<&str> = rest.split_whitespace().collect();
        let current = if rest.is_empty() || rest.ends_with(char::is_whitespace) {
            ""
        } else {
            words.pop().unwrap_or("")
        };

        let base = if absolute {
            Vec::new()
        } else {
            self.path.clone()
        };
        let mut full: Vec<&str> = base.iter().map(String::as_str).collect();

        let mut list = match words.split_first() {
            // 第一個字: 全域指令 + 這一層的子指令
            None => {
                let mut c = self.children(&full);
                let remote = self.capture.is_some();
                c.extend(
                    GLOBAL_CMDS
                        .iter()
                        // 網頁 CLI 不能離開程式
                        .filter(|(n, _)| !(remote && matches!(*n, "quit" | "q")))
                        .map(|(n, d)| (n.to_string(), d.to_string())),
                );
                c
            }
            Some((&("help" | "?"), args)) => {
                full.extend_from_slice(args);
                self.children(&full)
            }
            Some((&"source", [])) => {
                let mut c: Vec<(String, String)> = complete_path(current)
                    .into_iter()
                    .map(|p| (p, String::new()))
                    .collect();
                c.push(("<file>".into(), "script 檔".into()));
                c
            }
            Some((&"echo", _)) => vec![("<text>".into(), "要輸出的文字".into())],
            Some((&("quit" | "q" | "exit" | ".." | "source" | "restart"), _)) => Vec::new(),
            Some(_) => {
                full.extend_from_slice(&words);
                self.children(&full)
            }
        };
        list.retain(|(name, _)| is_placeholder(name) || name.starts_with(current));
        // 排序: 這一層的指令 → 全域指令 → 參數佔位
        let first_word = words.is_empty();
        let rank = |name: &str| {
            if is_placeholder(name) {
                2
            } else if first_word && GLOBAL_CMDS.iter().any(|(g, _)| *g == name) {
                1
            } else {
                0
            }
        };
        list.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then(a.0.cmp(&b.0)));
        list.dedup_by(|a, b| a.0 == b.0);
        (current, list)
    }

    /// Tab 補完: 補上共同前綴, 若無法再補且有多個候選, 就把候選列到 Message
    fn complete(&mut self, before: &str, ctx: &mut Ctx) {
        let (current, list) = self.candidates(before);
        let names: Vec<String> = list
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| !is_placeholder(n))
            .collect();

        let head = &before[..before.len() - current.len()];
        let replacement = match names.as_slice() {
            [] => return,
            // 目錄補完後不加空白, 方便繼續往下補
            [only] if only.ends_with('/') => format!("{head}{only}"),
            [only] => format!("{head}{only} "),
            many => {
                let common = common_prefix(many);
                if common.len() > current.len() {
                    format!("{head}{common}")
                } else {
                    ctx.info(format!("  {}", many.join("  ")));
                    return;
                }
            }
        };
        let ev = Event::Completed {
            before: before.to_string(),
            replacement,
        };
        self.broadcast(&ev);
    }

    /// `?`: 列出游標位置可用的指令與說明
    fn describe(&self, before: &str, ctx: &mut Ctx) {
        let (_, list) = self.candidates(before);
        ctx.push(Action::Output(
            Level::Command,
            format!("{} {before}?", self.prompt()),
        ));
        if list.is_empty() {
            ctx.info("  (沒有其他選項, 按 Enter 執行)");
            return;
        }
        let width = list
            .iter()
            .map(|(n, _)| n.width())
            .max()
            .unwrap_or(0)
            .max(12);
        for (name, desc) in list {
            let pad = " ".repeat(width - name.width());
            ctx.info(format!("  {name}{pad}  {desc}"));
        }
    }

    /// 某個完整路徑下一個字的候選與描述
    fn children(&self, path: &[&str]) -> Vec<(String, String)> {
        let pair = |n: &str, d: &str| (n.to_string(), d.to_string());
        match path {
            [] => {
                let mut c: Vec<_> = CORE_GROUPS.iter().map(|(g, d)| pair(g, d)).collect();
                c.push(pair("focus", "查看/切換 focus"));
                c.extend(self.plugins.iter().map(|p| pair(p.name(), p.description())));
                // 網頁 CLI 不列出 TUI 專用的指令
                if self.capture.is_some() {
                    c.retain(|(name, _)| !TUI_ONLY.contains(&name.as_str()));
                }
                c
            }
            ["panel", "show" | "toggle" | "rect", _name, rest @ ..] => RECT_PARAMS
                .get(rest.len())
                .map(|(n, d)| vec![pair(n, d)])
                .unwrap_or_default(),
            ["panel", "help"] => self
                .plugins
                .iter()
                .map(|p| pair(p.name(), p.description()))
                .collect(),
            ["panel", "show" | "hide" | "toggle" | "rect" | "reset"] => self
                .plugins
                .iter()
                .filter(|p| p.is_panel())
                .map(|p| pair(p.name(), p.description()))
                .collect(),
            ["panel", args @ ..] => complete_from_help(PANEL_HELP, args),
            ["focus"] => {
                let mut c = vec![pair(CLI, "回到 CLI")];
                c.extend(self.visible.iter().map(|v| pair(v, "panel")));
                c
            }
            ["plugin", args @ ..] => complete_from_help(PLUGIN_HELP, args),
            [name, args @ ..] => match self.find(name) {
                Some(i) => self.plugins[i].complete(args),
                None => Vec::new(),
            },
        }
    }

    // ------------------------------------------------------------- commands

    /// 解讀一行指令: 先處理全域指令, 其餘接在目前層次之後執行
    fn run_line(&mut self, line: &str, ctx: &mut Ctx) {
        let (absolute, rest) = match line.trim().strip_prefix('/') {
            Some(r) => (true, r),
            None => (false, line),
        };
        let owned = split_args(rest);
        let tokens: Vec<&str> = owned.iter().map(String::as_str).collect();
        let base = if absolute {
            Vec::new()
        } else {
            self.path.clone()
        };
        let mut full: Vec<&str> = base.iter().map(String::as_str).collect();

        let Some((&cmd, args)) = tokens.split_first() else {
            if absolute {
                self.set_path(Vec::new());
            }
            return;
        };

        match cmd {
            "help" | "?" => {
                full.extend_from_slice(args);
                self.cmd_help(&full, ctx);
            }
            "quit" | "q" => ctx.quit(),
            "restart" => {
                ctx.warn("重新啟動中…");
                ctx.push(Action::Restart);
            }
            ".." | "exit" => {
                if !self.path.is_empty() {
                    let mut p = self.path.clone();
                    p.pop();
                    self.set_path(p);
                } else if cmd == "exit" {
                    ctx.quit();
                } else {
                    ctx.warn("已在最上層");
                }
            }
            "echo" => ctx.info(args.join(" ")),
            "source" => Self::cmd_source(args, ctx),
            _ => {
                full.extend_from_slice(&tokens);
                if self.capture.is_some()
                    && let Some(first) = full.first()
                    && TUI_ONLY.contains(first)
                {
                    ctx.error(format!("'{first}' 只能在 TUI 使用"));
                    return;
                }
                if self.is_group(&full) {
                    self.set_path(full.iter().map(|s| s.to_string()).collect());
                } else {
                    self.dispatch(&full, ctx);
                }
            }
        }
    }

    /// 執行完整路徑的指令, 例如 ["panel", "show", "message"]
    fn dispatch(&mut self, full: &[&str], ctx: &mut Ctx) {
        let Some((&cmd, args)) = full.split_first() else {
            return;
        };
        match cmd {
            "panel" => self.cmd_panel(args, ctx),
            "focus" => self.cmd_focus(args, ctx),
            "plugin" => self.cmd_plugin(args, ctx),
            _ => match self.find(cmd) {
                Some(i) => self.plugins[i].handle_command(args, ctx),
                None => ctx.error(format!(
                    "不認識的指令 '{}', 輸入 'help' 查看說明",
                    full.join(" ")
                )),
            },
        }
    }

    fn help_rows(ctx: &mut Ctx, title: &str, rows: &[(&str, &str)]) {
        ctx.info(title);
        for (c, d) in rows {
            ctx.info(format!("  {c:<22} {d}"));
        }
    }

    /// `target` 是要查詢說明的完整路徑, 空的表示最上層
    fn cmd_help(&self, target: &[&str], ctx: &mut Ctx) {
        let Some(&group) = target.first() else {
            let mut groups: Vec<(&str, &str)> = CORE_GROUPS.to_vec();
            groups.extend(self.plugins.iter().map(|p| (p.name(), p.description())));
            Self::help_rows(ctx, "群組(輸入名稱進入該層):", &groups);
            Self::help_rows(ctx, "指令:", TOP_CMDS);
            Self::help_rows(ctx, "全域指令(任何層次皆可用):", GLOBAL_HELP);
            Self::help_rows(ctx, "按鍵:", KEY_HELP);
            return;
        };

        let rows: Vec<(&str, &str)> = match group {
            "panel" => PANEL_HELP.to_vec(),
            "plugin" => PLUGIN_HELP.to_vec(),
            name => match self.find(name) {
                Some(i) => self.plugins[i].help(),
                None => {
                    ctx.error(format!("沒有 '{name}' 的說明"));
                    return;
                }
            },
        };
        if rows.is_empty() {
            ctx.info(format!("{group}: 沒有子指令"));
        } else {
            Self::help_rows(ctx, &format!("{group} 的指令:"), &rows);
        }
        if self.path.first().map(String::as_str) == Some(group) {
            ctx.info("('..' 回上一層, '/help' 查看全部指令)");
        }
    }

    fn cmd_panel(&mut self, args: &[&str], ctx: &mut Ctx) {
        let (op, name, rect_args) = match args {
            ["list"] => {
                for p in self.plugins.iter().filter(|p| p.is_panel()) {
                    let name = p.name();
                    let mark = if self.visible.iter().any(|v| v == name) {
                        "[顯示]"
                    } else {
                        "[隱藏]"
                    };
                    let custom = if self.rects.contains_key(name) {
                        ""
                    } else {
                        " (預設)"
                    };
                    ctx.info(format!(
                        "  {mark} {name:<12} {}{custom}",
                        self.rect_of(name)
                    ));
                }
                return;
            }
            [op @ ("show" | "toggle" | "rect"), name, rest @ ..] => (*op, *name, rest),
            ["help", name] => {
                if self.find(name).is_some() {
                    self.help_view = Some(self.build_help(name));
                } else {
                    ctx.error(format!("沒有名為 '{name}' 的 panel"));
                }
                return;
            }
            [op @ ("hide" | "reset"), name] => (*op, *name, &[][..]),
            _ => {
                ctx.error("用法: 輸入 'help panel' 查看");
                return;
            }
        };

        if name == CLI {
            ctx.warn("cli 固定顯示在最下方, 無法變更");
            return;
        }
        if !self.is_panel(name) {
            ctx.error(format!("沒有名為 '{name}' 的 panel"));
            return;
        }

        // 有給位置就先設定位置
        if !rect_args.is_empty() {
            match PanelRect::parse(rect_args) {
                Ok(r) => {
                    self.rects.insert(name.to_string(), r);
                }
                Err(e) => {
                    ctx.error(format!("panel {op}: {e}"));
                    return;
                }
            }
        }

        let shown = self.visible.iter().any(|v| v == name);
        let show = match op {
            "show" => true,
            "hide" => false,
            "toggle" => !shown,
            "reset" => {
                self.rects.remove(name);
                ctx.info(format!(
                    "panel '{name}' 位置: {} (預設)",
                    self.rect_of(name)
                ));
                return;
            }
            _ => {
                // rect
                ctx.info(format!("panel '{name}' 位置: {}", self.rect_of(name)));
                return;
            }
        };
        // 顯示時移到最上層(重疊時蓋在其他 panel 上面)
        self.visible.retain(|v| v != name);
        if show {
            self.visible.push(name.to_string());
        } else if self.focus.as_deref() == Some(name) {
            // 隱藏有 focus 的 panel 時, focus 回到 CLI
            self.focus = None;
        }
        if show {
            ctx.info(format!("panel '{name}' 顯示, 位置: {}", self.rect_of(name)));
        } else {
            ctx.info(format!("panel '{name}' 隱藏"));
        }
    }

    fn cmd_focus(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            [] => ctx.info(format!("focus: {}", self.focus.as_deref().unwrap_or(CLI))),
            [name] if *name == CLI => self.focus = None,
            [name] if self.visible.iter().any(|v| v == name) => {
                self.focus = Some(name.to_string());
            }
            [name] if self.is_panel(name) => ctx.error(format!(
                "panel '{name}' 沒有顯示, 先用 'panel show {name}' 顯示"
            )),
            [name] => ctx.error(format!("沒有名為 '{name}' 的 panel")),
            _ => ctx.error("用法: focus [name]"),
        }
    }

    fn cmd_plugin(&self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["list"] => {
                for p in &self.plugins {
                    ctx.info(format!("  {:<12} {}", p.name(), p.description()));
                }
            }
            _ => ctx.error("用法: plugin list"),
        }
    }

    /// 讀取 script 檔, 逐行執行。空行與 `#` 開頭的行會被略過。
    fn cmd_source(args: &[&str], ctx: &mut Ctx) {
        let [path] = args else {
            ctx.error("用法: source <file>");
            return;
        };
        match std::fs::read_to_string(path) {
            Ok(content) => {
                for line in content.lines().map(str::trim) {
                    if !line.is_empty() && !line.starts_with('#') {
                        ctx.execute(line);
                    }
                }
            }
            Err(e) => ctx.error(format!("source: 無法讀取 '{path}': {e}")),
        }
    }

    /// 組出某個 panel 的說明視窗內容
    fn build_help(&self, name: &str) -> HelpView {
        let rows = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        let mut h = HelpView::new(name);
        let Some(i) = self.find(name) else {
            h.text(format!("沒有名為 '{name}' 的 panel"));
            return h;
        };
        let p = &self.plugins[i];
        let is_cli = name == CLI;

        h.heading(format!("{name} — {}", p.description()));
        h.blank();

        h.heading(if is_cli {
            "按鍵 (CLI 有 focus 時):"
        } else {
            "按鍵 (這個 panel 有 focus 時):"
        });
        h.table(&rows(&p.keys()));
        h.blank();

        h.heading("共用按鍵:");
        if !is_cli && !p.is_panel() {
            h.table(&rows(&[("Ctrl-C", "離開程式")]));
        } else if is_cli {
            h.table(&rows(&[
                ("Ctrl-N / Ctrl-P", "切換 focus 到 panel"),
                ("F10", "開啟選單"),
                ("Ctrl-C", "離開程式"),
            ]));
        } else {
            h.table(&rows(&[
                ("?", "顯示這個說明"),
                ("Esc", "回到 CLI"),
                ("Ctrl-N / Ctrl-P", "切換 focus 到下一個/上一個 panel"),
                ("F10", "開啟選單"),
                ("Ctrl-C", "離開程式"),
            ]));
        }
        h.blank();

        h.heading(format!(
            "CLI 指令 (也可以先輸入 '{name}' 進入該層再輸入子指令):"
        ));
        let cmds: Vec<(String, String)> = p
            .help()
            .iter()
            .map(|(c, d)| (format!("{name} {c}"), d.to_string()))
            .collect();
        h.table(&cmds);
        h.blank();

        if p.is_panel() {
            h.heading("panel 控制:");
            h.table(&[
                (
                    format!("panel show {name} [x1 y1 x2 y2]"),
                    "顯示, 可同時指定位置".into(),
                ),
                (format!("panel hide {name}"), "隱藏".into()),
                (
                    format!("panel rect {name} [x1 y1 x2 y2]"),
                    "查看/設定位置".into(),
                ),
                (format!("panel reset {name}"), "位置恢復預設".into()),
                (format!("focus {name}"), "切換 focus 到這個 panel".into()),
                (format!("panel help {name}"), "顯示這個說明".into()),
            ]);
            h.blank();

            let shown = self.visible.iter().any(|v| v == name);
            h.heading("狀態:");
            h.table(&[
                ("顯示".into(), if shown { "是" } else { "否" }.into()),
                ("目前位置".into(), self.rect_of(name).to_string()),
                ("預設位置".into(), self.default_rect_of(name).to_string()),
            ]);
            h.text("  位置是 x1 y1 x2 y2: 左上角到右下角, 以 CLI 上方區域的百分比 (0~100) 表示");
        }
        h
    }

    // ------------------------------------------------------------- settings

    /// 讀取設定檔並套用(程式啟動時呼叫)
    pub fn load_settings(&mut self) {
        self.process(vec![Action::LoadSettings]);
    }

    /// 所有設定的定義 (key, 說明, 預設值)
    fn setting_defs(&self) -> Vec<(String, String, String)> {
        let mut defs = Vec::new();
        for p in self.plugins.iter().filter(|p| p.is_panel()) {
            defs.push((
                format!("panel.{}.rect", p.name()),
                format!("{} panel 的預設位置 (x1 y1 x2 y2, 0~100%)", p.name()),
                p.default_rect().to_string(),
            ));
        }
        for p in &self.plugins {
            for (key, desc, default) in p.settings() {
                defs.push((format!("{}.{key}", p.name()), desc.to_string(), default));
            }
        }
        defs
    }

    /// 驗證並套用一個設定值(不儲存)
    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        if let Some(name) = key
            .strip_prefix("panel.")
            .and_then(|k| k.strip_suffix(".rect"))
            && self.is_panel(name)
        {
            // panel 位置在 rect_of 時從設定讀取, 這裡只需要驗證
            return PanelRect::parse(&value.split_whitespace().collect::<Vec<_>>()).map(|_| ());
        }
        let (plugin, sub) = key
            .split_once('.')
            .ok_or_else(|| format!("沒有設定 '{key}'"))?;
        let i = self
            .find(plugin)
            .filter(|&i| self.plugins[i].settings().iter().any(|(k, _, _)| *k == sub))
            .ok_or_else(|| format!("沒有設定 '{key}'"))?;
        self.plugins[i].apply_setting(sub, value)
    }

    /// `settings set` (value 為 Some) 與 `settings reset` (value 為 None)
    fn set_setting(&mut self, key: &str, value: Option<&str>, ctx: &mut Ctx) {
        let defs = self.setting_defs();
        let Some((_, _, default)) = defs.iter().find(|(k, _, _)| k == key) else {
            ctx.error(format!("沒有設定 '{key}', 用 'settings list' 查看"));
            return;
        };
        let value = value.unwrap_or(default).to_string();
        if let Err(e) = self.apply_setting(key, &value) {
            ctx.error(format!("{key}: {e}"));
            return;
        }
        self.settings.set(key, &value, default);
        ctx.info(format!("{key} = {value}"));
        self.save_settings(ctx);
    }

    fn save_settings(&mut self, ctx: &mut Ctx) {
        match self.settings.save(&self.setting_defs()) {
            Ok(()) => ctx.info(format!("已寫入設定檔 {}", self.settings.path())),
            Err(e) => ctx.error(e),
        }
        self.broadcast_settings();
    }

    /// 讀取設定檔: 每個設定只套用一次 — 設定檔有合法的值就用它, 否則用預設值
    fn reload_settings(&mut self, ctx: &mut Ctx) {
        let (values, warnings) = match self.settings.read() {
            Ok(Some(v)) => v,
            Ok(None) => Default::default(),
            Err(e) => {
                ctx.error(e);
                return;
            }
        };
        for w in warnings {
            ctx.warn(format!("{}: {w}", self.settings.path()));
        }
        let defs = self.setting_defs();
        self.settings.clear();
        let mut valid = std::collections::BTreeMap::new();
        for (key, _, default) in &defs {
            if let Some(value) = values.get(key) {
                match self.apply_setting(key, value) {
                    Ok(()) => {
                        valid.insert(key.clone(), value.clone());
                        continue;
                    }
                    Err(e) => ctx.warn(format!("設定 {key} = {value} 無效, 使用預設值: {e}")),
                }
            }
            let _ = self.apply_setting(key, default);
        }
        // 未知的設定保留(存檔時放在檔案最後), 也許是之後才註冊的 plugin
        for (key, value) in values {
            if !defs.iter().any(|(k, _, _)| *k == key) {
                ctx.warn(format!("未知的設定 {key}"));
                valid.insert(key, value);
            }
        }
        if !valid.is_empty() {
            ctx.info(format!(
                "已讀取設定檔 {} ({} 項)",
                self.settings.path(),
                valid.len()
            ));
        }
        self.settings.replace(valid);
        self.broadcast_settings();
    }

    fn broadcast_settings(&mut self) {
        let rows = self.setting_rows();
        self.broadcast(&Event::SettingsChanged(rows));
    }

    /// 游標前的文字之後可以接的字, 並標出選了之後還有沒有下一層
    fn web_candidates(&self, before: &str) -> Vec<Candidate> {
        let (current, list) = self.candidates(before);
        let head = &before[..before.len() - current.len()];
        list.into_iter()
            .map(|(name, desc)| {
                let placeholder = is_placeholder(&name);
                let more = !placeholder
                    && !matches!(name.as_str(), ".." | "exit" | "quit" | "q")
                    && !self.candidates(&format!("{head}{name} ")).1.is_empty();
                Candidate {
                    name,
                    desc,
                    more,
                    placeholder,
                }
            })
            .collect()
    }

    /// 所有 plugin 給網頁的描述與狀態卡片(依註冊順序)
    fn web_apps(&self) -> Vec<AppInfo> {
        let mut apps: Vec<AppInfo> = self
            .plugins
            .iter()
            .map(|p| AppInfo {
                name: p.name().to_string(),
                description: p.description().to_string(),
                meta: p
                    .web_meta()
                    .unwrap_or_else(|| WebMeta::new(p.name(), "other", "gray")),
                has_settings: !p.settings().is_empty(),
                view: p.web_view().map(str::to_string),
                card: p.web_card(),
            })
            .collect();
        // panel 位置不屬於任何 plugin, 是 App 自己的設定
        apps.push(AppInfo {
            name: "panel".into(),
            description: "TUI panel 的預設位置".into(),
            meta: WebMeta::new("Panel 位置", "panel", "orange"),
            has_settings: true,
            view: None,
            card: None,
        });
        apps
    }

    /// 所有設定的目前值
    fn setting_rows(&self) -> Vec<SettingRow> {
        self.setting_defs()
            .into_iter()
            .map(|(key, desc, default)| SettingRow {
                value: self.settings.get(&key).unwrap_or(&default).to_string(),
                key,
                default,
                desc,
            })
            .collect()
    }

    // ----------------------------------------------------------------- menu

    fn menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let mut ctx = Ctx::new();
        match menu.on_key(key) {
            MenuEvent::None => return,
            MenuEvent::Close => self.menu = None,
            MenuEvent::Help => self.help_view = Some(self.build_help("menu")),
            MenuEvent::Back => {
                if menu.path.pop().is_some() {
                    self.menu_refresh(&mut ctx);
                }
            }
            MenuEvent::Arg(text) => {
                menu.path.push(text);
                self.menu_refresh(&mut ctx);
            }
            MenuEvent::Select(i) => match menu.item(i).cloned() {
                Some(MenuItem {
                    kind: ItemKind::Exec,
                    ..
                }) => self.menu_exec(&mut ctx),
                Some(item) => {
                    menu.path.push(item.label);
                    self.menu_refresh(&mut ctx);
                }
                None => {}
            },
        }
        self.process(ctx.take());
    }

    /// 依選單目前的路徑重新產生項目; 沒有下一層時直接執行
    fn menu_refresh(&mut self, ctx: &mut Ctx) {
        let Some(path) = self.menu.as_ref().map(|m| m.path.clone()) else {
            return;
        };
        let refs: Vec<&str> = path.iter().map(String::as_str).collect();
        let items = self.menu_items(&refs);
        if !path.is_empty() && items.is_empty() {
            self.menu_exec(ctx);
        } else if let Some(m) = self.menu.as_mut() {
            m.set_items(items);
        }
    }

    /// 關閉選單並執行組好的指令(從最上層執行, 不受 CLI 目前層次影響)
    fn menu_exec(&mut self, ctx: &mut Ctx) {
        if let Some(m) = self.menu.take() {
            let words: Vec<String> = m.path.iter().map(|a| quote_arg(a)).collect();
            // 以 `/` 開頭, 從歷史叫出來時在任何層次都能執行
            let line = format!("/{}", words.join(" "));
            ctx.push(Action::AddHistory(line.clone()));
            ctx.execute(line);
        }
    }

    fn menu_items(&self, path: &[&str]) -> Vec<MenuItem> {
        let mut list: Vec<(String, String)> = match path {
            [] => {
                let mut c = self.children(&[]);
                c.retain(|(n, _)| n != "menu");
                c.push(("echo".into(), "輸出文字".into()));
                c.push(("source".into(), "執行 script 檔".into()));
                c.push(("quit".into(), "離開程式".into()));
                c
            }
            ["echo"] => vec![("<text>".into(), "要輸出的文字".into())],
            ["source"] => {
                let mut c: Vec<(String, String)> = complete_path("")
                    .into_iter()
                    .filter(|p| !p.ends_with('/'))
                    .map(|p| (p, String::new()))
                    .collect();
                c.sort();
                c.push(("<file>".into(), "輸入檔名".into()));
                c
            }
            ["echo" | "source", _, ..] | ["quit"] => Vec::new(),
            _ => self.children(path),
        };
        let mut seen = std::collections::HashSet::new();
        list.retain(|(n, _)| seen.insert(n.clone()));

        let mut items = Vec::new();
        if !list.is_empty() && self.executable(path) {
            let line: Vec<String> = path.iter().map(|a| quote_arg(a)).collect();
            items.push(MenuItem {
                label: "↵ 執行".into(),
                desc: line.join(" "),
                kind: ItemKind::Exec,
                more: false,
            });
        }
        for (label, desc) in list {
            let kind = if is_placeholder(&label) {
                ItemKind::Param
            } else {
                ItemKind::Child
            };
            let more = kind == ItemKind::Child && {
                let mut next = path.to_vec();
                next.push(&label);
                !self.children(&next).is_empty()
            };
            items.push(MenuItem {
                label,
                desc,
                kind,
                more,
            });
        }
        items
    }

    /// 這個路徑本身是否已經是可以執行的完整指令(依各個 help 表判斷)
    fn executable(&self, path: &[&str]) -> bool {
        let Some((&head, args)) = path.split_first() else {
            return false;
        };
        let table: Vec<(&str, &str)> = match head {
            "panel" => PANEL_HELP.to_vec(),
            "plugin" => PLUGIN_HELP.to_vec(),
            "focus" => vec![("[name]", "")],
            name => match self.find(name) {
                Some(i) if !self.plugins[i].is_group() => return args.is_empty(),
                Some(i) => self.plugins[i].help(),
                None => return false,
            },
        };
        table.iter().any(|(usage, _)| {
            let words: Vec<&str> = usage.split_whitespace().filter(|w| *w != "|").collect();
            // `[` 開始之後是可省略的參數群組, 要嘛全給, 要嘛都不給
            let required = words
                .iter()
                .position(|w| w.starts_with('['))
                .unwrap_or(words.len());
            (args.len() == required || args.len() == words.len())
                && args
                    .iter()
                    .zip(&words)
                    .enumerate()
                    .all(|(i, (a, w))| i >= required || is_placeholder(w) || a == w)
        })
    }

    // --------------------------------------------------------------- render

    pub fn render(&mut self, frame: &mut Frame) {
        let [panels_area, cli_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

        // 依顯示順序繪製, 重疊時後顯示的在上層
        for name in self.visible.clone() {
            let area = self.rect_of(&name).to_area(panels_area);
            if area.width == 0 || area.height == 0 {
                continue;
            }
            if let Some(i) = self.find(&name) {
                let focused = self.focus.as_deref() == Some(name.as_str());
                frame.render_widget(Clear, area);
                self.plugins[i].render(frame, area, focused);
            }
        }

        if let Some(i) = self.find(CLI) {
            self.plugins[i].render(frame, cli_area, self.focus.is_none());
        }

        if let Some(m) = self.menu.as_mut() {
            m.render(frame, frame.area());
        }

        // 說明視窗蓋在最上面
        if let Some(h) = self.help_view.as_mut() {
            h.render(frame, frame.area());
        }
    }
}

/// 把新產生的 action 插在佇列最前面, 保持輸出順序
fn requeue(queue: &mut VecDeque<Action>, mut ctx: Ctx) {
    for a in ctx.take().into_iter().rev() {
        queue.push_front(a);
    }
}

/// 多個字串的共同前綴(以 char 為單位, 避免切到多位元組字元)
fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();
    for s in &items[1..] {
        let n = prefix
            .iter()
            .zip(s.chars())
            .take_while(|(a, b)| **a == *b)
            .count();
        prefix.truncate(n);
    }
    prefix.into_iter().collect()
}

/// 檔名補完, 目錄會加上 `/`
fn complete_path(current: &str) -> Vec<String> {
    let (dir, prefix) = match current.rfind('/') {
        Some(i) => (&current[..=i], &current[i + 1..]),
        None => ("", current),
    };
    let read_dir = if dir.is_empty() { "." } else { dir };
    let Ok(entries) = std::fs::read_dir(read_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
                return None;
            }
            let slash = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{dir}{name}{slash}"))
        })
        .collect()
}
