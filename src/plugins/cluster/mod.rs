//! Cluster plugin: 多台機器透過 MQTT 互相知道彼此的狀態。
//!
//! 每台設定角色(cluster.role):
//! - server: 名稱(cluster.name, 預設 hostname)就是群組名稱
//! - client: 設定要加入的 server 名稱(cluster.server)
//!
//! 每台要設定同一把金鑰(cluster.key): 資料加密, topic 也由金鑰推導,
//! 所以可以放心使用公開的 broker(預設 broker.emqx.io)。

pub mod crypto;
pub mod mqtt;
pub mod node;

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, RwLock};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::plugin::{Ctx, PanelRect, Plugin, complete_from_help, panel_block, parse_usize};
use crate::webui::{Card, Status, WebMeta};
use mqtt::{Cmd, Config, Node, Role, SharedState};

const DEFAULT_BROKER: &str = "broker.emqx.io:8883";
const DEFAULT_INTERVAL: u64 = 10;
/// 超過幾次間隔沒有收到就算「沒有回應」
const STALE_FACTOR: i64 = 3;

/// 和網頁伺服器共用
#[derive(Clone)]
pub struct Shared {
    state: SharedState,
    cfg: Arc<RwLock<Config>>,
}

/// 給網頁的資料
#[derive(Serialize)]
pub struct Snapshot {
    pub role: Role,
    pub name: String,
    pub server: String,
    pub broker: String,
    pub interval: u64,
    pub connected: bool,
    pub error: Option<String>,
    pub idle: Option<String>,
    pub now: i64,
    pub nodes: Vec<NodeView>,
}

#[derive(Serialize)]
pub struct NodeView {
    #[serde(flatten)]
    pub node: Node,
    /// 上線但太久沒有收到資料
    pub stale: bool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            state: Default::default(),
            cfg: Arc::new(RwLock::new(Config {
                role: Role::Off,
                name: node::hostname(),
                server: String::new(),
                key: String::new(),
                broker: DEFAULT_BROKER.into(),
                interval: DEFAULT_INTERVAL,
            })),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let cfg = self.cfg.read().unwrap().clone();
        let st = self.state.read().unwrap();
        let now = chrono::Utc::now().timestamp();
        let limit = cfg.interval as i64 * STALE_FACTOR + 5;
        // 本機在最前面, 接著 server, 其他依名稱
        let mut nodes: Vec<NodeView> = st
            .nodes
            .values()
            .map(|n| NodeView {
                stale: n.online && !n.me && now - n.last_seen > limit,
                node: n.clone(),
            })
            .collect();
        nodes.sort_by_key(|v| (!v.node.me, v.node.role != "server", v.node.name.clone()));
        Snapshot {
            role: cfg.role,
            name: cfg.name.clone(),
            server: cfg.server.clone(),
            broker: cfg.broker.clone(),
            interval: cfg.interval,
            connected: st.connected,
            error: st.error.clone(),
            idle: st.idle.clone(),
            now,
            nodes,
        }
    }
}

pub struct Cluster {
    shared: Shared,
    tx: UnboundedSender<Cmd>,
    log: Receiver<(bool, String)>,
}

impl Cluster {
    pub fn new(shared: Shared) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (log_tx, log) = mpsc::channel();
        mqtt::spawn(shared.state.clone(), rx, log_tx);
        let me = Self { shared, tx, log };
        me.push();
        me
    }

    fn cfg(&self) -> Config {
        self.shared.cfg.read().unwrap().clone()
    }

    /// 把目前的設定送給背景 thread
    fn push(&self) {
        let _ = self.tx.send(Cmd::Config(self.cfg()));
    }
}

pub fn human_bytes(b: u64) -> String {
    const U: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut v = b as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

pub fn human_duration(secs: i64) -> String {
    let s = secs.max(0);
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d} 天 {h} 小時")
    } else if h > 0 {
        format!("{h} 小時 {m} 分")
    } else if m > 0 {
        format!("{m} 分鐘")
    } else {
        format!("{s} 秒")
    }
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 }
}

/// 一台機器的狀態文字
fn status_text(v: &NodeView, now: i64) -> (&'static str, Color, String) {
    let ago = human_duration(now - v.node.last_seen);
    if v.node.me {
        ("●", Color::Green, "這台".into())
    } else if !v.node.online {
        ("○", Color::DarkGray, format!("離線({ago}前)"))
    } else if v.stale {
        ("●", Color::Yellow, format!("沒有回應({ago})"))
    } else {
        ("●", Color::Green, "上線".into())
    }
}

impl Plugin for Cluster {
    fn name(&self) -> &str {
        "cluster"
    }

    fn description(&self) -> &str {
        "多台機器透過 MQTT 互相顯示狀態"
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("status", "列出群組裡的機器"),
            ("info <name>", "一台機器的詳細狀態"),
            ("forget <name>", "移除一台已經不用的機器(清掉 broker 上的紀錄)"),
            ("genkey", "產生一把新的隨機金鑰(cluster.key)"),
            ("reconnect", "重新連線"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["info" | "forget"] => self
                .shared
                .state
                .read()
                .unwrap()
                .nodes
                .values()
                .filter(|n| !n.me)
                .map(|n| (n.name.clone(), n.role.clone()))
                .collect(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            ("role", "角色: off / server / client", "off".into()),
            ("name", "這台的名稱(空白 = hostname)", String::new()),
            ("server", "client 要加入的 server 名稱", String::new()),
            ("key", "共用金鑰, 群組裡每台要相同(cluster genkey 產生)", String::new()),
            ("broker", "MQTT broker(host:port, 8883 走 TLS)", DEFAULT_BROKER.into()),
            ("interval", "送出狀態的間隔(秒), 2~3600", DEFAULT_INTERVAL.to_string()),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        let v = value.trim();
        {
            let mut cfg = self.shared.cfg.write().unwrap();
            match key {
                "role" => cfg.role = Role::parse(v)?,
                "name" => {
                    if v.contains(['/', '+', '#']) {
                        return Err("名稱不能有 / + #".into());
                    }
                    cfg.name = if v.is_empty() { node::hostname() } else { v.to_string() };
                }
                "server" => cfg.server = v.to_string(),
                "key" => cfg.key = v.to_string(),
                "broker" => {
                    if v.is_empty() {
                        return Err("broker 不能是空的".into());
                    }
                    cfg.broker = v.to_string();
                }
                "interval" => cfg.interval = parse_usize(v, 2, 3600)? as u64,
                _ => return Err(format!("不支援的設定 '{key}'")),
            }
        }
        self.push();
        Ok(())
    }

    fn default_rect(&self) -> PanelRect {
        PanelRect {
            x1: 50,
            y1: 0,
            x2: 100,
            y2: 50,
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["status"] => {
                let snap = self.shared.snapshot();
                ctx.info(header(&snap));
                for v in &snap.nodes {
                    let (_, _, st) = status_text(v, snap.now);
                    let load = v
                        .node
                        .info
                        .as_ref()
                        .map(|i| format!("  CPU {:.0}%  load {:.2}", i.cpu_usage, i.load[0]))
                        .unwrap_or_default();
                    ctx.info(format!("  {} [{}] {st}{load}", v.node.name, v.node.role));
                }
            }
            ["info", name] => {
                let snap = self.shared.snapshot();
                match snap.nodes.iter().find(|v| v.node.name == *name) {
                    Some(v) => {
                        for l in detail_lines(v, snap.now) {
                            ctx.info(l);
                        }
                    }
                    None => ctx.error(format!("群組裡沒有 '{name}'")),
                }
            }
            ["forget", name] => {
                if *name == self.cfg().name {
                    ctx.error("不能移除自己");
                    return;
                }
                let _ = self.tx.send(Cmd::Forget(name.to_string()));
                ctx.info(format!("已移除 {name}(如果它還在線上, 下次送出狀態時會再出現)"));
            }
            ["genkey"] => {
                let key = crypto::Cipher::new("").random_hex(16);
                ctx.info(format!("新的金鑰: {key}"));
                ctx.info(format!("每台都要設定: settings set cluster.key {key}"));
            }
            ["reconnect"] => {
                // 送一個不同的設定再送回來, 讓背景 thread 重新連線
                let mut c = self.cfg();
                let orig = c.clone();
                c.interval += 1;
                let _ = self.tx.send(Cmd::Config(c));
                let _ = self.tx.send(Cmd::Config(orig));
                ctx.info("cluster: 重新連線中…");
            }
            _ => ctx.error("用法: 輸入 'help cluster' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        while let Ok((err, msg)) = self.log.try_recv() {
            if err {
                ctx.warn(msg);
            } else {
                ctx.info(msg);
            }
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let snap = self.shared.snapshot();
        let mut lines = vec![Line::from(Span::styled(
            header(&snap),
            Style::default().fg(Color::Cyan),
        ))];
        if let Some(e) = &snap.error {
            lines.push(Line::from(Span::styled(format!("錯誤: {e}"), Style::default().fg(Color::Red))));
        }
        for v in &snap.nodes {
            let (dot, color, st) = status_text(v, snap.now);
            let mut spans = vec![
                Span::styled(format!("{dot} "), Style::default().fg(color)),
                Span::styled(v.node.name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!(" [{}] ", v.node.role), Style::default().fg(Color::DarkGray)),
                Span::styled(st, Style::default().fg(color)),
            ];
            if let Some(i) = v.node.info.as_ref().filter(|_| v.node.online || v.node.me) {
                spans.push(Span::raw(format!(
                    "  CPU {:>3.0}%  Mem {:>3.0}%  Disk {:>3.0}%{}  up {}",
                    i.cpu_usage,
                    pct(i.mem_used, i.mem_total),
                    pct(i.disk_used, i.disk_total),
                    i.temp.map(|t| format!("  {t:.0}°C")).unwrap_or_default(),
                    human_duration(i.uptime as i64),
                )));
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(panel_block("Cluster", focused)),
            area,
        );
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("Cluster", "network", "teal"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("cluster")
    }

    fn web_card(&self) -> Option<Card> {
        let snap = self.shared.snapshot();
        let others: Vec<_> = snap.nodes.iter().filter(|v| !v.node.me).collect();
        let online = others.iter().filter(|v| v.node.online && !v.stale).count();
        let status = if snap.role == Role::Off {
            Status::Off
        } else if snap.error.is_some() && !snap.connected {
            Status::Critical
        } else if !snap.connected || others.iter().any(|v| v.stale) {
            Status::Warning
        } else {
            Status::Good
        };
        let summary = match (snap.role, &snap.idle) {
            (Role::Off, _) => "未啟用".to_string(),
            (_, Some(why)) => why.clone(),
            _ if !snap.connected => "連線中…".into(),
            _ => format!("{} · 其他 {} 台, {online} 台上線", snap.role.as_str(), others.len()),
        };
        let mut card = Card::new().status(status).summary(summary);
        if !others.is_empty() {
            card = card.table(others.iter().map(|v| {
                let (_, _, st) = status_text(v, snap.now);
                let extra = v
                    .node
                    .info
                    .as_ref()
                    .filter(|_| v.node.online && !v.stale)
                    .map(|i| format!(" · CPU {:.0}%", i.cpu_usage))
                    .unwrap_or_default();
                (v.node.name.clone(), format!("{st}{extra}"))
            }));
        }
        Some(card)
    }
}

/// 第一行: 角色、名稱、連線狀態
fn header(s: &Snapshot) -> String {
    match (s.role, &s.idle) {
        (Role::Off, _) => "角色 off(cluster.role 設成 server 或 client 啟用)".into(),
        (_, Some(why)) => why.clone(),
        _ => format!(
            "{} {}{} · {}",
            s.role.as_str(),
            s.name,
            if s.role == Role::Client { format!(" → {}", s.server) } else { String::new() },
            if s.connected { format!("已連線 {}", s.broker) } else { format!("連線中 {}", s.broker) }
        ),
    }
}

fn detail_lines(v: &NodeView, now: i64) -> Vec<String> {
    let (_, _, st) = status_text(v, now);
    let mut out = vec![format!("{} [{}] {st}", v.node.name, v.node.role)];
    if let Some(i) = &v.node.info {
        out.push(format!("  系統   {} · {} ({})", i.os, i.kernel, i.arch));
        out.push(format!("  CPU    {} x{} · {:.0}% · load {:.2} {:.2} {:.2}", i.cpu, i.cores, i.cpu_usage, i.load[0], i.load[1], i.load[2]));
        out.push(format!("  記憶體 {} / {} ({:.0}%)", human_bytes(i.mem_used), human_bytes(i.mem_total), pct(i.mem_used, i.mem_total)));
        out.push(format!("  磁碟   {} / {} ({:.0}%)", human_bytes(i.disk_used), human_bytes(i.disk_total), pct(i.disk_used, i.disk_total)));
        if let Some(t) = i.temp {
            out.push(format!("  溫度   {t:.1}°C"));
        }
        out.push(format!("  開機   {} · cng6 {} (執行 {})", human_duration(i.uptime as i64), i.version, human_duration(now - i.started)));
        if !i.ips.is_empty() {
            out.push(format!("  IP     {}", i.ips.join(", ")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(human_duration(5), "5 秒");
        assert_eq!(human_duration(125), "2 分鐘");
        assert_eq!(human_duration(3 * 3600 + 60), "3 小時 1 分");
        assert_eq!(human_duration(2 * 86400 + 3600), "2 天 1 小時");
        assert_eq!(human_bytes(1536), "1.5 KB");
    }
}
