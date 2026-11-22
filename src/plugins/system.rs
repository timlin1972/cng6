//! System plugin: 顯示系統資訊(主機、CPU、記憶體、溫度、磁碟…), 定時更新。

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sysinfo::{Components, Disks, System};

use crate::plugin::{Ctx, PanelRect, Plugin, panel_block};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const BAR_WIDTH: usize = 20;

pub struct SystemInfo {
    sys: System,
    components: Components,
    disks: Disks,
    host: String,
    os: String,
    kernel: String,
    arch: String,
    cpu_brand: String,
    interval: Duration,
    last_refresh: Instant,
}

impl SystemInfo {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let cpu_brand = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "?".into());
        Self {
            sys,
            components: Components::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            host: System::host_name().unwrap_or_else(|| "?".into()),
            os: System::long_os_version().unwrap_or_else(|| "?".into()),
            kernel: System::kernel_version().unwrap_or_else(|| "?".into()),
            arch: System::cpu_arch(),
            cpu_brand,
            interval: Duration::from_secs(1),
            last_refresh: Instant::now(),
        }
    }

    fn set_interval(&mut self, sec: &str) -> Result<(), String> {
        match sec.parse::<f64>() {
            Ok(s) if (0.5..=3600.0).contains(&s) => {
                self.interval = Duration::from_secs_f64(s);
                Ok(())
            }
            _ => Err("請輸入 0.5~3600 的秒數".into()),
        }
    }

    fn refresh(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.components.refresh(false);
        self.disks.refresh(false);
        self.last_refresh = Instant::now();
    }

    /// 組出要顯示的內容, 面板與 `system info` 共用
    fn lines(&self) -> Vec<Line<'static>> {
        let label = |s: &str| Span::styled(format!("{s:<7}"), Style::default().fg(Color::Cyan));
        let mut out = Vec::new();

        out.push(Line::from(vec![
            label("Host"),
            Span::raw(self.host.clone()),
        ]));
        out.push(Line::from(vec![label("OS"), Span::raw(self.os.clone())]));
        out.push(Line::from(vec![
            label("Kernel"),
            Span::raw(format!("{} ({})", self.kernel, self.arch)),
        ]));
        out.push(Line::from(vec![
            label("Uptime"),
            Span::raw(fmt_uptime(System::uptime())),
        ]));
        let load = System::load_average();
        out.push(Line::from(vec![
            label("Load"),
            Span::raw(format!(
                "{:.2} {:.2} {:.2}",
                load.one, load.five, load.fifteen
            )),
        ]));

        // CPU
        let cpus = self.sys.cpus();
        out.push(Line::from(vec![
            label("CPU"),
            Span::raw(format!("{} x{}", self.cpu_brand, cpus.len())),
        ]));
        let usage = self.sys.global_cpu_usage() as f64;
        out.push(bar_line(label(""), usage, format!("{usage:5.1}%")));
        let per_core: Vec<Span> = cpus
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                let u = c.cpu_usage() as f64;
                [
                    Span::raw(format!("{i}:")),
                    Span::styled(format!("{u:>3.0}% "), Style::default().fg(level_color(u))),
                ]
            })
            .collect();
        out.push(Line::from([vec![label("")], per_core].concat()));

        // 記憶體
        let (used, total) = (self.sys.used_memory(), self.sys.total_memory());
        out.push(bar_line(
            label("Mem"),
            pct(used, total),
            format!("{:.1}/{:.1} GiB", used as f64 / GIB, total as f64 / GIB),
        ));
        let (used, total) = (self.sys.used_swap(), self.sys.total_swap());
        if total > 0 {
            out.push(bar_line(
                label("Swap"),
                pct(used, total),
                format!("{:.1}/{:.1} GiB", used as f64 / GIB, total as f64 / GIB),
            ));
        }

        // 溫度
        for c in self.components.iter() {
            if let Some(t) = c.temperature() {
                let color = if t >= 80.0 {
                    Color::Red
                } else if t >= 65.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                out.push(Line::from(vec![
                    label("Temp"),
                    Span::styled(format!("{t:.1}°C"), Style::default().fg(color)),
                    Span::styled(
                        format!("  {}", c.label()),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }

        // 磁碟
        for d in self.disks.iter().filter(|d| d.total_space() > 0) {
            let total = d.total_space();
            let used = total - d.available_space();
            out.push(bar_line(
                label("Disk"),
                pct(used, total),
                format!(
                    "{:.1}/{:.1} GiB  {}",
                    used as f64 / GIB,
                    total as f64 / GIB,
                    d.mount_point().display()
                ),
            ));
        }
        out
    }
}

impl Plugin for SystemInfo {
    fn name(&self) -> &str {
        "system"
    }

    fn description(&self) -> &str {
        "顯示系統資訊"
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("info", "把目前的系統資訊輸出到 Message"),
            ("refresh", "立即更新"),
            ("interval [sec]", "查看/設定更新間隔(秒)"),
        ]
    }

    fn default_rect(&self) -> PanelRect {
        PanelRect {
            x1: 0,
            y1: 0,
            x2: 50,
            y2: 50,
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["info"] => {
                self.refresh();
                for line in self.lines() {
                    ctx.info(line.to_string());
                }
            }
            ["refresh"] => self.refresh(),
            ["interval"] => ctx.info(format!("更新間隔: {} 秒", self.interval.as_secs_f64())),
            ["interval", sec] => match self.set_interval(sec) {
                Ok(()) => ctx.info(format!(
                    "更新間隔: {sec} 秒 (只影響這次執行, 要保存請用 settings set system.interval {sec})"
                )),
                Err(e) => ctx.error(format!("system interval: {e}")),
            },
            _ => ctx.error(format!("system: 不認識的指令 '{}'", args.join(" "))),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![("interval", "System 更新間隔(秒), 0.5~3600", "1".into())]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "interval" => self.set_interval(value),
            _ => Err(format!("不支援的設定 '{key}'")),
        }
    }

    fn on_tick(&mut self, _ctx: &mut Ctx) {
        if self.last_refresh.elapsed() >= self.interval {
            self.refresh();
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = panel_block("System", focused);
        frame.render_widget(Paragraph::new(self.lines()).block(block), area);
    }
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    }
}

fn level_color(p: f64) -> Color {
    if p >= 90.0 {
        Color::Red
    } else if p >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// `label [██████░░░░] text`
fn bar_line(label: Span<'static>, p: f64, text: String) -> Line<'static> {
    let filled = ((p / 100.0 * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    Line::from(vec![
        label,
        Span::styled(
            "█".repeat(filled),
            Style::default()
                .fg(level_color(p))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "░".repeat(BAR_WIDTH - filled),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(format!(" {text}")),
    ])
}

fn fmt_uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}
