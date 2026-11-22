mod app;
mod help_view;
mod menu_view;
mod parse;
mod plugin;
mod plugins;
mod settings;

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;

/// plugin on_tick 的間隔
const TICK: Duration = Duration::from_millis(250);

/// 預設的啟動 script, 可用第一個命令列參數指定其他檔案
const STARTUP_SCRIPT: &str = "startup.cng";

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
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
    while app.running() {
        terminal.draw(|f| app.render(f))?;
        // 等待按鍵, 最多等到下一次 tick
        let timeout = TICK.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
        if last_tick.elapsed() >= TICK {
            app.tick();
            last_tick = Instant::now();
        }
    }
    Ok(())
}
