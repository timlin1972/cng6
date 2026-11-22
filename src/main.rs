mod app;
mod help_view;
mod menu_view;
mod parse;
mod plugin;
mod plugins;
mod remote;
mod settings;
mod webui;

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;

/// plugin on_tick 的間隔
const TICK: Duration = Duration::from_millis(250);

/// 檢查網頁 CLI 請求的間隔(等待按鍵時最多等這麼久)
const REMOTE_POLL: Duration = Duration::from_millis(20);

/// 預設的啟動 script, 可用第一個命令列參數指定其他檔案
const STARTUP_SCRIPT: &str = "startup.cng";

/// 由 utils/cng6.sh 啟動時會設定這個環境變數; restart 時用這個結束碼讓它重新編譯、啟動
const SUPERVISED_ENV: &str = "CNG6_SUPERVISED";
const RESTART_EXIT_CODE: i32 = 3;

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    if let Ok(true) = result {
        restart();
    }
    result.map(|_| ())
}

/// 重新啟動: 由 utils/cng6.sh 管理時交給它(會先重新編譯), 否則直接重新執行同一個執行檔
fn restart() -> ! {
    // 讓網頁 restart 指令的回應先送出去
    std::thread::sleep(Duration::from_millis(300));
    if std::env::var_os(SUPERVISED_ENV).is_some() {
        std::process::exit(RESTART_EXIT_CODE);
    }
    use std::os::unix::process::CommandExt;
    // 執行檔被重新編譯取代時, /proc/self/exe 會顯示 "... (deleted)"
    let exe = std::env::current_exe()
        .map(|p| {
            let s = p.to_string_lossy();
            std::path::PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s))
        })
        .unwrap_or_else(|_| "cng6".into());
    let err = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("無法重新啟動 {}: {err}", exe.display());
    std::process::exit(1);
}

/// 執行主迴圈, 回傳是否要重新啟動
fn run(terminal: &mut DefaultTerminal) -> io::Result<bool> {
    let mut app = App::new();
    // 先讀設定檔, 再執行啟動 script
    app.load_settings();

    let script = std::env::args()
        .nth(1)
        .unwrap_or_else(|| STARTUP_SCRIPT.to_string());
    if Path::new(&script).exists() {
        app.execute(&format!("source {script}"));
    }

    let mut last_tick = Instant::now();
    let mut dirty = true;
    while app.running() {
        // 有變化才重繪
        if dirty {
            terminal.draw(|f| app.render(f))?;
            dirty = false;
        }
        // 等待按鍵, 最多等到下一次 tick; 期間定時處理網頁 CLI 的請求
        let timeout = TICK.saturating_sub(last_tick.elapsed()).min(REMOTE_POLL);
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.on_key(key);
            }
            // 視窗大小改變等事件也要重繪
            dirty = true;
        }
        if app.handle_remote() {
            dirty = true;
        }
        if last_tick.elapsed() >= TICK {
            app.tick();
            last_tick = Instant::now();
            dirty = true;
        }
    }
    Ok(app.restart_requested())
}
