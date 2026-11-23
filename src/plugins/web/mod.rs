//! Web plugin: 內建 web 伺服器, 可以用 CLI 啟動/關閉。
//!
//! - 預設使用 https, 同一個 port 收到 http 時會轉址到 https([`server`])
//! - 每個請求都會判斷來源是 PC(大畫面)或 mobile, 再依裝置產生頁面([`page`])
//! - 請求紀錄透過 channel 傳回, 在 `on_tick` 時輸出到 Message

mod books_api;
mod cli_api;
mod files_api;
mod music_api;
mod page;
mod pdfjs;
pub mod server;
mod sysmon;
mod wallpaper_api;
mod weather_api;
mod worldclock_api;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::plugin::{Ctx, Plugin, parse_bool, parse_usize};
use crate::remote;
use crate::webui::{Button, Card, Status, WebMeta};
use server::{Config, Handle};

const DEFAULT_PORT: u16 = 9760;
const DEFAULT_CERT: &str = "web_cert.pem";
const DEFAULT_KEY: &str = "web_key.pem";

struct Running {
    handle: Handle,
    /// 啟動時使用的設定
    config: Config,
    /// 可以連線的網址(啟動時算好, 狀態卡片不用每次查網卡)
    urls: Vec<String>,
}

pub struct Web {
    config: Config,
    log_requests: bool,
    running: Option<Running>,
    tx: Sender<String>,
    rx: Receiver<String>,
    /// 下次 tick 時提示設定要 restart 才生效
    restart_notice: bool,
    /// 網頁 CLI 的請求送到主迴圈
    remote: remote::Sender,
    services: server::Services,
}

impl Web {
    pub fn new(remote: remote::Sender, services: server::Services) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            config: Config {
                port: DEFAULT_PORT,
                https: true,
                cert: DEFAULT_CERT.into(),
                key: DEFAULT_KEY.into(),
            },
            log_requests: true,
            running: None,
            tx,
            rx,
            restart_notice: false,
            remote,
            services,
        }
    }

    /// 執行中, 但設定已經和啟動時不同
    fn restart_needed(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| r.config != self.config)
    }

    fn start(&mut self, ctx: &mut Ctx) {
        if let Some(r) = &self.running {
            ctx.warn(format!("web 已經在 port {} 執行中", r.config.port));
            return;
        }
        match server::start(
            &self.config,
            self.tx.clone(),
            self.remote.clone(),
            self.services.clone(),
        ) {
            Ok((handle, notes)) => {
                for n in notes {
                    ctx.info(n);
                }
                self.running = Some(Running {
                    handle,
                    urls: Self::urls(&self.config),
                    config: self.config.clone(),
                });
                self.restart_notice = false;
                let scheme = if self.config.https {
                    "https (http 會轉到 https)"
                } else {
                    "http"
                };
                ctx.info(format!("web 已啟動, port {}, {scheme}", self.config.port));
                self.show_urls(ctx);
            }
            Err(e) => ctx.error(format!("web 無法啟動: {e}")),
        }
    }

    fn urls(cfg: &Config) -> Vec<String> {
        let scheme = if cfg.https { "https" } else { "http" };
        let port = cfg.port;
        let mut urls = vec![format!("{scheme}://localhost:{port}/")];
        urls.extend(
            server::local_ips()
                .iter()
                .map(|ip| format!("{scheme}://{ip}:{port}/")),
        );
        urls
    }

    fn show_urls(&self, ctx: &mut Ctx) {
        let urls = match &self.running {
            Some(r) => r.urls.clone(),
            None => Self::urls(&self.config),
        };
        for url in urls {
            ctx.info(format!("  {url}"));
        }
    }

    /// 把伺服器傳回的請求紀錄輸出到 Message
    fn drain(&mut self, ctx: &mut Ctx) {
        while let Ok(line) = self.rx.try_recv() {
            if self.log_requests {
                ctx.info(line);
            }
        }
    }

    fn stop(&mut self) -> bool {
        match self.running.take() {
            Some(mut r) => {
                r.handle.stop();
                true
            }
            None => false,
        }
    }

    /// 修改需要 restart 才生效的設定
    fn set_server(&mut self, f: impl FnOnce(&mut Config)) {
        f(&mut self.config);
        self.restart_notice = self.restart_needed();
    }
}

impl Plugin for Web {
    fn name(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Web 伺服器"
    }

    /// 背景服務, 沒有畫面
    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("start", "啟動 web 伺服器"),
            ("stop", "關閉 web 伺服器"),
            ("restart", "重新啟動(套用新的設定)"),
            ("status", "顯示狀態與連線網址"),
        ]
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            (
                "port",
                "Web 伺服器 port, 1~65535 (執行中修改需 restart)",
                DEFAULT_PORT.to_string(),
            ),
            (
                "https",
                "使用 https, http 連線會轉到 https (執行中修改需 restart)",
                "true".into(),
            ),
            (
                "cert",
                "https 憑證檔 (PEM), 和私鑰都不存在時自動產生自簽憑證",
                DEFAULT_CERT.into(),
            ),
            ("key", "https 私鑰檔 (PEM)", DEFAULT_KEY.into()),
            ("log_requests", "在 Message 顯示每個網頁請求", "true".into()),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "port" => {
                let port = parse_usize(value, 1, 65535)? as u16;
                self.set_server(|c| c.port = port);
            }
            "https" => {
                let https = parse_bool(value)?;
                self.set_server(|c| c.https = https);
            }
            "cert" => self.set_server(|c| c.cert = PathBuf::from(value)),
            "key" => self.set_server(|c| c.key = PathBuf::from(value)),
            "log_requests" => self.log_requests = parse_bool(value)?,
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["start"] => self.start(ctx),
            ["stop"] => {
                let stopped = self.stop();
                self.drain(ctx);
                if stopped {
                    ctx.info("web 已關閉");
                } else {
                    ctx.warn("web 沒有在執行");
                }
            }
            ["restart"] => {
                self.stop();
                self.drain(ctx);
                self.start(ctx);
            }
            ["status"] => match &self.running {
                Some(r) => {
                    let scheme = if r.config.https { "https" } else { "http" };
                    ctx.info(format!("web 執行中, port {}, {scheme}", r.config.port));
                    self.show_urls(ctx);
                    if self.restart_needed() {
                        ctx.warn("設定已變更, 執行 web restart 後生效");
                    }
                }
                None => ctx.info(format!("web 未啟動 (port {})", self.config.port)),
            },
            _ => ctx.error("用法: 輸入 'help web' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        if std::mem::take(&mut self.restart_notice) {
            ctx.warn("web 設定已變更, 執行 web restart 後生效");
        }
        self.drain(ctx);
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("Web", "web", "purple"))
    }

    fn web_card(&self) -> Option<Card> {
        let card = match &self.running {
            Some(r) => {
                let scheme = if r.config.https { "https" } else { "http" };
                let mut card = Card::new()
                    .status(Status::Good)
                    .summary(format!("執行中 · {scheme} · port {}", r.config.port))
                    .table([
                        ("狀態", "執行中".to_string()),
                        ("port", r.config.port.to_string()),
                        ("協定", scheme.to_string()),
                    ]);
                if self.restart_needed() {
                    card = card.text("設定已變更, 重新啟動後生效", Some(Status::Warning));
                }
                for url in r.urls.iter().filter(|u| !u.contains("localhost")) {
                    card = card.link(url.clone(), url.clone());
                }
                card.buttons(vec![
                    Button::new("重新啟動", "/web restart").confirm(),
                    Button::new("停止", "/web stop").confirm(),
                ])
            }
            None => Card::new()
                .status(Status::Off)
                .summary(format!("未啟動 · port {}", self.config.port))
                .text("web 伺服器沒有在執行", Some(Status::Off))
                .buttons(vec![Button::new("啟動", "/web start")]),
        };
        Some(card.toggle(
            "在 Message 顯示網頁請求",
            "web.log_requests",
            self.log_requests,
        ))
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}
