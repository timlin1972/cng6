//! 背景 thread: 連到 MQTT broker, 定時送出本機狀態, 接收群組裡其他機器的狀態。
//!
//! - status 用保留訊息(retain), 新加入的機器馬上看到其他機器最後的狀態
//! - 連線時設定遺囑(LWT): 斷線時 broker 自動送出「離線」
//! - 設定改變時重新連線

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;

use super::alerts::{Alert, Alerts, Limits};
use super::bus::{Bus, Conn};
use super::crypto::Cipher;
use super::node::{Collector, Info, Message};
use super::sync::{Deps, Sync, SyncMsg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Off,
    Server,
    Client,
}

impl Role {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "" => Ok(Role::Off),
            "server" => Ok(Role::Server),
            "client" => Ok(Role::Client),
            _ => Err(format!("角色要是 off / server / client, 不是 '{s}'")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Off => "off",
            Role::Server => "server",
            Role::Client => "client",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub role: Role,
    pub name: String,
    /// client 要連的 server 名稱
    pub server: String,
    pub key: String,
    /// `host:port`
    pub broker: String,
    /// 送出狀態的間隔(秒)
    pub interval: u64,
}

impl Config {
    /// 群組名稱(server 的名稱)
    pub fn group(&self) -> &str {
        match self.role {
            Role::Server => &self.name,
            _ => &self.server,
        }
    }

    /// 還缺什麼設定
    pub fn missing(&self) -> Option<&'static str> {
        match self.role {
            Role::Off => Some("角色是 off(cluster.role 設成 server 或 client 才會連線)"),
            _ if self.key.trim().is_empty() => Some("還沒有設定金鑰 cluster.key(每台要相同)"),
            Role::Client if self.server.trim().is_empty() => {
                Some("client 要設定 server 的名稱 cluster.server")
            }
            _ => None,
        }
    }

    fn host_port(&self) -> Result<(String, u16), String> {
        let b = self.broker.trim();
        match b.rsplit_once(':') {
            Some((h, p)) => Ok((
                h.to_string(),
                p.parse().map_err(|_| format!("broker 的 port 不對: '{b}'"))?,
            )),
            None => Ok((b.to_string(), 8883)),
        }
    }
}

/// 群組裡的一台機器
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub name: String,
    pub role: String,
    pub online: bool,
    /// 這台(本機)
    pub me: bool,
    /// 最後收到的時間(本機時鐘, Unix 秒)
    pub last_seen: i64,
    #[serde(skip)]
    pub session: String,
    #[serde(skip)]
    pub ts: i64,
    pub info: Option<Info>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct State {
    pub connected: bool,
    /// 最近一次的連線錯誤
    pub error: Option<String>,
    /// 沒有連線的原因(設定不完整)
    pub idle: Option<String>,
    pub nodes: BTreeMap<String, Node>,
    /// 目前有問題的
    pub alerts_active: Vec<Alert>,
    /// 最近的通知
    pub alerts: Vec<Alert>,
    /// 共用剪貼簿(新的在前)
    pub clips: Vec<Clip>,
}

pub type SharedState = Arc<RwLock<State>>;

/// 共用剪貼簿的一筆
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct Clip {
    pub from: String,
    pub ts: i64,
    pub text: String,
}

/// 剪貼簿保留幾筆、每筆多大
const CLIP_KEEP: usize = 20;
pub const CLIP_MAX: usize = 64 * 1024;

pub enum Cmd {
    Config(Config),
    /// 清除一台機器留在 broker 上的狀態
    Forget(String),
    /// 在別台執行 CLI 指令(to = 名稱或 all)
    Run { to: String, line: String },
    /// 送到共用剪貼簿
    Clip(String),
}

/// 背景 thread 需要的其他東西
pub struct Extra {
    pub bus: Arc<Bus>,
    pub deps: Deps,
    pub limits: Arc<RwLock<Limits>>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 用 ring 的 TLS 設定(不需要 aws-lc-rs), 信任內建的根憑證
fn tls_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto::ring};
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    Arc::new(
        ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("TLS 版本")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

pub fn spawn(state: SharedState, rx: UnboundedReceiver<Cmd>, log: Sender<(bool, String)>, extra: Extra) {
    std::thread::Builder::new()
        .name("cluster".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run(state, rx, log, extra));
        })
        .expect("cluster thread");
}

/// 主迴圈: 等到設定完整再連線, 設定改變就重連
async fn run(state: SharedState, mut rx: UnboundedReceiver<Cmd>, log: Sender<(bool, String)>, extra: Extra) {
    let mut w = Worker {
        state: state.clone(),
        log: log.clone(),
        bus: extra.bus,
        limits: extra.limits,
        sync: Sync::new(extra.deps),
        alerts: Alerts::default(),
        collector: Collector::new(),
    };
    let mut next: Option<Config> = None;
    loop {
        // 啟動時設定會一項一項套用: 等設定穩定下來再連線
        let Some(c) = settle(next.take(), &mut rx).await else {
            return;
        };
        {
            let mut st = state.write().unwrap();
            st.nodes.clear();
            st.connected = false;
            st.error = None;
            st.idle = c.missing().map(String::from);
            // 本機一定列在裡面
            if c.role != Role::Off {
                st.nodes
                    .insert(c.name.clone(), self_node(&c, "", w.collector.collect(), false));
            }
        }
        if c.missing().is_some() {
            continue;
        }
        match session(&c, &mut rx, &mut w).await {
            Some(n) => {
                let _ = log.send((false, "cluster: 設定已改變, 重新連線".into()));
                next = Some(n);
            }
            None => return,
        }
    }
}

/// 從 `first`(或等第一筆)開始, 收設定直到 0.8 秒內沒有新的
async fn settle(first: Option<Config>, rx: &mut UnboundedReceiver<Cmd>) -> Option<Config> {
    let mut cur = first;
    loop {
        let wait = if cur.is_some() {
            Duration::from_millis(800)
        } else {
            Duration::from_secs(3600)
        };
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(Cmd::Config(c))) => cur = Some(c),
            Ok(Some(_)) => {}
            Ok(None) => return None,
            Err(_) if cur.is_some() => return cur,
            Err(_) => {}
        }
    }
}

fn self_node(c: &Config, session: &str, info: Info, online: bool) -> Node {
    Node {
        name: c.name.clone(),
        role: c.role.as_str().into(),
        online,
        me: true,
        last_seen: now(),
        session: session.into(),
        ts: now(),
        info: Some(info),
    }
}

/// 背景 thread 的狀態
struct Worker {
    state: SharedState,
    log: Sender<(bool, String)>,
    bus: Arc<Bus>,
    limits: Arc<RwLock<Limits>>,
    sync: Sync,
    alerts: Alerts,
    collector: Collector,
}

impl Worker {
    fn info(&self, msg: String) {
        let _ = self.log.send((false, msg));
    }
    fn warn(&self, msg: String) {
        let _ = self.log.send((true, msg));
    }

    /// 檢查異常, 有變化就通知
    fn check_alerts(&mut self, c: &Config) {
        let now = now();
        let limit = c.interval as i64 * 3 + 5;
        let limits = *self.limits.read().unwrap();
        let nodes = self.state.read().unwrap().nodes.clone();
        let out = self.alerts.check(
            &nodes,
            |n| n.online && !n.me && now - n.last_seen > limit,
            &limits,
            now,
        );
        for (bad, text) in out {
            if bad {
                self.warn(format!("cluster: ⚠ {text}"));
            } else {
                self.info(format!("cluster: {text}"));
            }
        }
        let mut st = self.state.write().unwrap();
        st.alerts_active = self.alerts.active();
        st.alerts = self.alerts.history.clone();
    }
}

/// 一次連線; 設定改變時回傳新設定, channel 關閉時回傳 None
async fn session(c: &Config, rx: &mut UnboundedReceiver<Cmd>, w: &mut Worker) -> Option<Config> {
    let state = w.state.clone();
    let (host, port) = match c.host_port() {
        Ok(hp) => hp,
        Err(e) => {
            state.write().unwrap().error = Some(e.clone());
            w.warn(format!("cluster: {e}"));
            return wait_config(c, rx).await;
        }
    };
    let cipher = Arc::new(Cipher::new(&c.key));
    let group = c.group().to_string();
    let my_topic = cipher.status_topic(&group, &c.name);
    let my_inbox = cipher.inbox_topic(&group, &c.name);
    let clip_topic = cipher.clip_topic(&group);
    let sess = cipher.random_hex(8);
    let msg = |online: bool, info: Option<Info>| -> Vec<u8> {
        let m = Message {
            name: c.name.clone(),
            role: c.role.as_str().into(),
            server: group.clone(),
            session: sess.clone(),
            ts: now(),
            online,
            info,
        };
        cipher.seal(&my_topic, &serde_json::to_vec(&m).unwrap_or_default())
    };

    let mut opts = MqttOptions::new(format!("cng6-{}", cipher.random_hex(6)), host.clone(), port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    // 遠端檔案一段 128KB, 加上標頭與加密
    opts.set_max_packet_size(512 * 1024, 512 * 1024);
    opts.set_last_will(LastWill::new(&my_topic, msg(false, None), QoS::AtLeastOnce, true));
    if port != 1883 {
        opts.set_transport(Transport::tls_with_config(TlsConfiguration::Rustls(tls_config())));
    }
    let (client, mut eventloop) = AsyncClient::new(opts, 64);
    let conn = Conn::new(client.clone(), cipher.clone(), group.clone(), c.name.clone());
    let mut tick = tokio::time::interval(Duration::from_secs(c.interval.max(2)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sync_tick = tokio::time::interval(Duration::from_secs(5));
    sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut backoff = 2u64;
    let mut warned_dup = false;
    let mut last_err = String::new();

    let result = loop {
        tokio::select! {
            ev = eventloop.poll() => match ev {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    backoff = 2;
                    last_err.clear();
                    {
                        let mut st = state.write().unwrap();
                        st.connected = true;
                        st.error = None;
                    }
                    w.info(format!(
                        "cluster: 已連線 {host}:{port}, 角色 {}, 名稱 {}{}",
                        c.role.as_str(), c.name,
                        if c.role == Role::Client { format!(", server {}", c.server) } else { String::new() }
                    ));
                    for t in [cipher.status_filter(&group), my_inbox.clone(), cipher.sync_filter(&group), clip_topic.clone()] {
                        let _ = client.subscribe(t, QoS::AtLeastOnce).await;
                    }
                    let info = w.collector.collect();
                    let _ = client.publish(my_topic.clone(), QoS::AtLeastOnce, true, msg(true, Some(info.clone()))).await;
                    state.write().unwrap().nodes.insert(c.name.clone(), self_node(c, &sess, info, true));
                    w.bus.set_conn(Some(conn.clone()));
                    w.sync.connected();
                }
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    let topic = p.topic.as_str();
                    if topic == my_inbox {
                        if let Some(plain) = cipher.open(topic, &p.payload) {
                            w.bus.on_message(&plain);
                        }
                    } else if topic.ends_with("/status") {
                        handle(&cipher, c, &group, &sess, topic, &p.payload, p.retain, &state, &w.log, &mut warned_dup);
                    } else if topic == clip_topic {
                        on_clip(&cipher, topic, &p.payload, p.retain, c, w);
                    } else if topic.contains("/sync/")
                        && let Some(m) = cipher.open(topic, &p.payload).and_then(|b| serde_json::from_slice::<SyncMsg>(&b).ok())
                        && let Some(note) = w.sync.on_message(&conn, &w.bus, topic, m)
                    {
                        w.info(format!("cluster: {note}"));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    w.bus.set_conn(None);
                    let text = e.to_string();
                    let was = std::mem::replace(&mut state.write().unwrap().connected, false);
                    state.write().unwrap().error = Some(text.clone());
                    // 同樣的錯誤只記一次
                    if was || text != last_err {
                        w.warn(format!("cluster: 無法連線 {host}:{port}: {text}, {backoff} 秒後重試"));
                    }
                    last_err = text;
                    // 等待重試, 期間設定改變就換新設定
                    let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                    tokio::pin!(sleep);
                    let changed = loop {
                        tokio::select! {
                            _ = &mut sleep => break None,
                            cmd = rx.recv() => match cmd {
                                Some(Cmd::Config(n)) if n != *c => break Some(Some(n)),
                                Some(_) => {}
                                None => break Some(None),
                            }
                        }
                    };
                    if let Some(r) = changed {
                        break r;
                    }
                    backoff = (backoff * 2).min(60);
                }
            },
            _ = tick.tick() => {
                let info = w.collector.collect();
                let connected = state.read().unwrap().connected;
                state.write().unwrap().nodes.insert(c.name.clone(), self_node(c, &sess, info.clone(), connected));
                if connected {
                    // QoS 1: broker 限流時最先丟掉 QoS 0, 狀態送不到會被誤認為「沒有回應」
                    let _ = client.try_publish(my_topic.clone(), QoS::AtLeastOnce, true, msg(true, Some(info)));
                }
                w.check_alerts(c);
            }
            _ = sync_tick.tick() => {
                if state.read().unwrap().connected {
                    w.sync.poll(&conn, now());
                }
            }
            cmd = rx.recv() => match cmd {
                Some(Cmd::Config(n)) if n != *c => {
                    // 正常離開: 先告訴大家離線
                    let _ = client.publish(my_topic.clone(), QoS::AtLeastOnce, true, msg(false, None)).await;
                    let _ = client.disconnect().await;
                    let _ = tokio::time::timeout(Duration::from_secs(2), async {
                        while eventloop.poll().await.is_ok() {}
                    }).await;
                    break Some(n);
                }
                Some(Cmd::Config(_)) => {}
                Some(Cmd::Forget(name)) => {
                    // 清掉保留訊息: 送一個空的 retained message
                    let topic = cipher.status_topic(&group, &name);
                    let _ = client.publish(topic, QoS::AtLeastOnce, true, Vec::<u8>::new()).await;
                    state.write().unwrap().nodes.remove(&name);
                }
                Some(Cmd::Run { to, line }) => run_remote(c, w, &to, &line),
                Some(Cmd::Clip(text)) => {
                    let clip = Clip { from: c.name.clone(), ts: now(), text };
                    let payload = cipher.seal(&clip_topic, &serde_json::to_vec(&clip).unwrap_or_default());
                    let _ = client.publish(clip_topic.clone(), QoS::AtLeastOnce, true, payload).await;
                    add_clip(&state, clip);
                }
                None => break None,
            }
        }
    };
    w.bus.set_conn(None);
    result
}

fn add_clip(state: &SharedState, clip: Clip) -> bool {
    let mut st = state.write().unwrap();
    if st.clips.iter().any(|x| x.ts == clip.ts && x.from == clip.from) {
        return false;
    }
    st.clips.insert(0, clip);
    st.clips.sort_by(|a, b| b.ts.cmp(&a.ts));
    st.clips.truncate(CLIP_KEEP);
    true
}

fn on_clip(cipher: &Cipher, topic: &str, payload: &[u8], retained: bool, c: &Config, w: &Worker) {
    let Some(clip) = cipher
        .open(topic, payload)
        .and_then(|b| serde_json::from_slice::<Clip>(&b).ok())
    else {
        return;
    };
    if clip.text.len() > CLIP_MAX {
        return;
    }
    let from = clip.from.clone();
    let preview: String = clip.text.chars().take(40).collect();
    if add_clip(&w.state, clip) && !retained && from != c.name {
        w.info(format!("cluster: 剪貼簿 (來自 {from}): {preview}"));
    }
}

/// 在別台執行 CLI 指令, 結果輸出到 Message
fn run_remote(c: &Config, w: &Worker, to: &str, line: &str) {
    let targets: Vec<String> = if to == "all" {
        w.state
            .read()
            .unwrap()
            .nodes
            .values()
            .filter(|n| !n.me && n.online)
            .map(|n| n.name.clone())
            .collect()
    } else {
        vec![to.to_string()]
    };
    if targets.is_empty() {
        w.warn("cluster: 沒有上線的機器".into());
        return;
    }
    for t in targets {
        let bus = w.bus.clone();
        let log = w.log.clone();
        let body = serde_json::json!({ "session": format!("cluster-{}", c.name), "op": "exec", "line": line });
        tokio::spawn(async move {
            let res = bus
                .call(&t, axum::http::Method::POST, "/api/cli", Some("application/json"), body.to_string().into_bytes())
                .await;
            match res {
                Ok((200, data)) => {
                    let v: serde_json::Value = serde_json::from_slice(&data).unwrap_or_default();
                    let lines = v["output"].as_array().cloned().unwrap_or_default();
                    if lines.is_empty() {
                        let _ = log.send((false, format!("[{t}] (完成, 沒有輸出)")));
                    }
                    for l in lines {
                        let text = l["text"].as_str().unwrap_or("");
                        let err = matches!(l["level"].as_str(), Some("error" | "warn"));
                        let _ = log.send((err, format!("[{t}] {text}")));
                    }
                }
                Ok((code, data)) => {
                    let _ = log.send((true, format!("[{t}] 失敗 ({code}): {}", String::from_utf8_lossy(&data))));
                }
                Err(e) => {
                    let _ = log.send((true, format!("[{t}] {e}")));
                }
            }
        });
    }
}

/// 連線前的設定錯誤: 等到設定改變
async fn wait_config(c: &Config, rx: &mut UnboundedReceiver<Cmd>) -> Option<Config> {
    loop {
        match rx.recv().await {
            Some(Cmd::Config(n)) if n != *c => return Some(n),
            Some(_) => {}
            None => return None,
        }
    }
}

/// 收到一筆 status
#[allow(clippy::too_many_arguments)]
fn handle(
    cipher: &Cipher,
    c: &Config,
    group: &str,
    my_session: &str,
    topic: &str,
    payload: &[u8],
    retained: bool,
    state: &SharedState,
    log: &Sender<(bool, String)>,
    warned_dup: &mut bool,
) {
    // 空的保留訊息: 被 forget 清掉了
    if payload.is_empty() {
        return;
    }
    let Some(plain) = cipher.open(topic, payload) else {
        return; // 金鑰不同或被竄改
    };
    let Ok(m) = serde_json::from_slice::<Message>(&plain) else {
        return;
    };
    // topic 必須是這個名稱的 topic, 也必須是同一個群組
    if cipher.status_topic(group, &m.name) != topic || m.server != group {
        return;
    }
    if m.name == c.name {
        // 同名的另一台機器
        if m.session != my_session && m.online && !retained && !*warned_dup {
            *warned_dup = true;
            let _ = log.send((true, format!("cluster: 有另一台機器也叫 '{}', 請用 cluster.name 改名", c.name)));
        }
        return;
    }
    let mut st = state.write().unwrap();
    let t = now();
    let prev = st.nodes.get(&m.name).cloned();
    if !m.online {
        // 離線通知: 是同一次連線的, 或比目前的資料新才算
        if let Some(p) = &prev
            && p.session != m.session
            && m.ts < p.ts
        {
            return;
        }
    } else if let Some(p) = &prev
        && p.session == m.session
        && m.ts < p.ts
    {
        return; // 舊資料
    }
    let was_online = prev.as_ref().is_some_and(|p| p.online);
    let node = Node {
        name: m.name.clone(),
        role: m.role.clone(),
        online: m.online,
        me: false,
        // 保留訊息可能是很久以前的: 用送出時間(不超過現在)
        last_seen: if retained { m.ts.min(t) } else { t },
        session: m.session.clone(),
        ts: m.ts,
        info: m.info.clone().or_else(|| prev.as_ref().and_then(|p| p.info.clone())),
    };
    st.nodes.insert(m.name.clone(), node);
    drop(st);
    if m.online && !was_online && !retained {
        let _ = log.send((false, format!("cluster: {} ({}) 上線", m.name, m.role)));
    } else if !m.online && was_online {
        let _ = log.send((false, format!("cluster: {} 離線", m.name)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(role: Role, name: &str, server: &str) -> Config {
        Config {
            role,
            name: name.into(),
            server: server.into(),
            key: "k".into(),
            broker: "broker.emqx.io:8883".into(),
            interval: 10,
        }
    }

    #[test]
    fn config_checks() {
        assert!(cfg(Role::Off, "a", "").missing().is_some());
        assert!(cfg(Role::Server, "a", "").missing().is_none());
        assert!(cfg(Role::Client, "a", "").missing().is_some());
        assert!(cfg(Role::Client, "a", "b").missing().is_none());
        let mut c = cfg(Role::Server, "a", "");
        c.key = " ".into();
        assert!(c.missing().is_some());
        assert_eq!(cfg(Role::Server, "a", "").group(), "a");
        assert_eq!(cfg(Role::Client, "a", "b").group(), "b");
        assert_eq!(cfg(Role::Server, "a", "").host_port().unwrap(), ("broker.emqx.io".into(), 8883));
        assert_eq!(Role::parse("Server").unwrap(), Role::Server);
        assert!(Role::parse("boss").is_err());
    }

    #[test]
    fn handles_status_and_offline() {
        let (tx, rx) = std::sync::mpsc::channel();
        let state: SharedState = Default::default();
        let server = cfg(Role::Server, "pi5", "");
        let client = cfg(Role::Client, "laptop", "pi5");
        let cipher = Cipher::new("k");
        let topic = cipher.status_topic("pi5", "laptop");
        let send = |online: bool, session: &str, ts: i64, retained: bool| {
            let m = Message {
                name: client.name.clone(),
                role: "client".into(),
                server: "pi5".into(),
                session: session.into(),
                ts,
                online,
                info: online.then(Info::default),
            };
            let payload = cipher.seal(&topic, &serde_json::to_vec(&m).unwrap());
            let mut dup = false;
            handle(&cipher, &server, "pi5", "mine", &topic, &payload, retained, &state, &tx, &mut dup);
        };
        send(true, "s1", 100, false);
        assert!(state.read().unwrap().nodes["laptop"].online);
        assert!(rx.try_recv().unwrap().1.contains("上線"));
        // 舊連線的遺囑不會蓋掉新連線
        send(true, "s2", 200, false);
        send(false, "s1", 150, false);
        assert!(state.read().unwrap().nodes["laptop"].online);
        // 這次連線的遺囑: 離線, 保留最後的狀態
        send(false, "s2", 210, false);
        let n = state.read().unwrap().nodes["laptop"].clone();
        assert!(!n.online && n.info.is_some());
        // 別的金鑰、別的 topic 都不接受
        let other = Cipher::new("x");
        let bad = other.seal(&topic, b"{}");
        let mut dup = false;
        handle(&cipher, &server, "pi5", "mine", &topic, &bad, false, &state, &tx, &mut dup);
        assert_eq!(state.read().unwrap().nodes.len(), 1);
    }
}
