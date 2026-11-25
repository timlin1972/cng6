//! 機器之間的請求/回應: 把 HTTP 請求透過加密的 MQTT 轉給另一台,
//! 由那台用自己的網頁 API(本機的 Router)處理, 再把回應分段傳回來。
//!
//! 遠端 CLI、遠端檔案、檔案傳送、同步桌布圖片都走這裡。
//!
//! - 每一則訊息(加密前): `[4 bytes: header 長度][header JSON][資料]`, 送到收件者的收件匣
//! - 大的內容切成 [`CHUNK`] 一段, 依序編號
//! - 公開的 broker 有沒公開的流量限制, 送太快會被丟掉訊息, 所以:
//!   - 送出方限速([`RATE`]), 最多領先 [`WINDOW`] 段還沒被確認
//!   - 接收方每 [`ACK_EVERY`] 段回報一次; 發現少了一段就請對方從那段重送(Go-Back-N)
//!   - 送出方太久沒收到回報也會重送; 重送多次還是不行才放棄

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use rumqttc::{AsyncClient, QoS};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_stream::StreamExt;
use tower::ServiceExt;

use super::crypto::Cipher;
use crate::plugins::web::server::{Peer, PeerAddr};

/// 每一段的大小
pub const CHUNK: usize = 128 * 1024;
/// 單一請求或回應的大小上限
pub const LIMIT: u64 = 100 * 1024 * 1024;
/// 送出方最多領先幾段還沒被確認
const WINDOW: u32 = 8;
/// 接收方每幾段回報一次
const ACK_EVERY: u32 = 2;
/// 送出的速度上限(bytes/秒)與一次最多送多少
const RATE: f64 = 384.0 * 1024.0;
const BURST: f64 = 256.0 * 1024.0;
/// 多久沒收到回報就重送, 最多重送幾次
const RESEND_AFTER: Duration = Duration::from_secs(5);
const MAX_RESENDS: u32 = 8;
/// 還沒確認的資料很少時(例如終端機的輸入、小的回應), 重送的等待從這麼久開始, 每次加長到 RESEND_AFTER:
/// 一次來回不到 1 秒, broker 偶爾丟掉一則訊息時不用等滿 5 秒
const RESEND_SMALL: Duration = Duration::from_millis(1500);
const SMALL: usize = 16 * 1024;
/// 請求的時間差超過這個就拒絕(防止重放)
const MAX_SKEW: i64 = 300;
/// 等待回應的時間
const FIRST_TIMEOUT: Duration = Duration::from_secs(45);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

/// 可以轉過來的路徑(cluster 自己的 API 不行, 避免繞圈)
fn allowed(path: &str) -> bool {
    ["/api/", "/files/", "/books/", "/music/", "/wallpapers/"]
        .iter()
        .any(|p| path.starts_with(p))
        && (!path.starts_with("/api/cluster") || path == "/api/cluster/send")
}

/// 要轉送的 header(其他的不需要)
const PASS_REQ: &[&str] = &["content-type", "range", "accept"];
const PASS_RES: &[&str] = &[
    "content-type",
    "content-length",
    "content-range",
    "accept-ranges",
    "content-disposition",
    "content-security-policy",
    "x-content-type-options",
    "cache-control",
    "last-modified",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Kind {
    /// 請求開始
    Req {
        method: String,
        uri: String,
        headers: Vec<(String, String)>,
    },
    /// 請求內容的一段
    Body,
    /// 回應開始
    Res {
        status: u16,
        headers: Vec<(String, String)>,
    },
    /// 回應內容的一段
    Rbody,
    /// 收到第 seq 段(含)以前的了
    Ack,
    /// 少了第 seq 段, 請從那段重送
    Nack,
    /// 失敗
    Err { msg: String },
    /// 請求方不要了
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Header {
    from: String,
    to: String,
    id: String,
    ts: i64,
    seq: u32,
    more: bool,
    #[serde(flatten)]
    kind: Kind,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn encode(h: &Header, data: &[u8]) -> Vec<u8> {
    let json = serde_json::to_vec(h).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + json.len() + data.len());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(data);
    out
}

fn decode(plain: &[u8]) -> Option<(Header, Bytes)> {
    let len = u32::from_be_bytes(plain.get(..4)?.try_into().ok()?) as usize;
    let json = plain.get(4..4 + len)?;
    let h = serde_json::from_slice(json).ok()?;
    Some((h, Bytes::copy_from_slice(&plain[4 + len..])))
}

/// 送出速度限制(token bucket), 同一條連線共用
struct Pacer {
    tokens: f64,
    last: Instant,
}

/// 連線中的 MQTT
#[derive(Clone)]
pub struct Conn {
    pub client: AsyncClient,
    pub cipher: Arc<Cipher>,
    pub group: String,
    pub me: String,
    pacer: Arc<Mutex<Pacer>>,
}

impl Conn {
    pub fn new(client: AsyncClient, cipher: Arc<Cipher>, group: String, me: String) -> Self {
        Self {
            client,
            cipher,
            group,
            me,
            pacer: Arc::new(Mutex::new(Pacer {
                tokens: BURST,
                last: Instant::now(),
            })),
        }
    }

    /// 等到可以送出 n bytes
    async fn pace(&self, n: usize) {
        loop {
            let wait = {
                let mut p = self.pacer.lock().unwrap();
                let t = Instant::now();
                p.tokens = (p.tokens + t.duration_since(p.last).as_secs_f64() * RATE).min(BURST);
                p.last = t;
                if p.tokens >= n as f64 || p.tokens >= BURST {
                    p.tokens -= n as f64;
                    return;
                }
                (n as f64 - p.tokens) / RATE
            };
            tokio::time::sleep(Duration::from_secs_f64(wait.min(2.0))).await;
        }
    }

    async fn send(&self, to: &str, h: &Header, data: &[u8]) -> Result<(), String> {
        if !data.is_empty() {
            self.pace(data.len()).await;
        }
        let topic = self.cipher.inbox_topic(&self.group, to);
        let payload = self.cipher.seal(&topic, &encode(h, data));
        self.client
            .publish(topic, QoS::AtLeastOnce, false, payload)
            .await
            .map_err(|e| e.to_string())
    }

    /// 不等待的送出(在同步的程式裡用, 只用在沒有資料的小訊息)
    fn try_send(&self, to: &str, h: &Header) {
        let topic = self.cipher.inbox_topic(&self.group, to);
        let payload = self.cipher.seal(&topic, &encode(h, &[]));
        let _ = self.client.try_publish(topic, QoS::AtLeastOnce, false, payload);
    }

    fn header(&self, to: &str, id: &str, seq: u32, more: bool, kind: Kind) -> Header {
        Header {
            from: self.me.clone(),
            to: to.to_string(),
            id: id.to_string(),
            ts: now(),
            seq,
            more,
            kind,
        }
    }
}

/// 接收方的回報
#[derive(Debug, Clone, Copy)]
enum Fb {
    Ack(u32),
    Nack(u32),
}

/// 分段送出(請求內容或回應內容), 保留還沒被確認的段, 需要時重送
struct Outgoing<'a> {
    conn: &'a Conn,
    to: &'a str,
    id: &'a str,
    /// Body 或 Rbody
    kind: Kind,
    fb: UnboundedReceiver<Fb>,
    buf: VecDeque<(u32, bool, Vec<u8>)>,
    next: u32,
    acked: u32,
    resends: u32,
    head: Header,
}

impl<'a> Outgoing<'a> {
    /// head: 這段內容的開頭(請求的 Req 或回應的 Res)。broker 可能丟掉它,
    /// 對方一段都還沒確認時, 重送會連開頭一起送
    fn new(conn: &'a Conn, to: &'a str, id: &'a str, kind: Kind, fb: UnboundedReceiver<Fb>, head: Header) -> Self {
        Self {
            conn,
            to,
            id,
            kind,
            fb,
            head,
            buf: VecDeque::new(),
            next: 1,
            acked: 0,
            resends: 0,
        }
    }

    async fn send_one(&self, seq: u32, more: bool, data: &[u8]) -> Result<(), String> {
        let h = self.conn.header(self.to, self.id, seq, more, self.kind.clone());
        self.conn.send(self.to, &h, data).await
    }

    /// 送出一段; 最後一段(more = false)會等到對方全部收到
    async fn push(&mut self, data: Vec<u8>, more: bool) -> Result<(), String> {
        let seq = self.next;
        self.next += 1;
        self.send_one(seq, more, &data).await?;
        self.buf.push_back((seq, more, data));
        while seq.saturating_sub(self.acked) >= WINDOW || (!more && self.acked < seq) {
            self.wait().await?;
        }
        Ok(())
    }

    async fn wait(&mut self) -> Result<(), String> {
        let unacked: usize = self.buf.iter().map(|(_, _, d)| d.len()).sum();
        let after = if unacked <= SMALL {
            (RESEND_SMALL * (self.resends + 1)).min(RESEND_AFTER)
        } else {
            RESEND_AFTER
        };
        match tokio::time::timeout(after, self.fb.recv()).await {
            Ok(Some(Fb::Ack(s))) => self.ack(s),
            Ok(Some(Fb::Nack(s))) => {
                self.ack(s.saturating_sub(1));
                self.retry().await?;
            }
            Ok(None) => return Err("連線中斷".into()),
            Err(_) => self.retry().await?,
        }
        Ok(())
    }

    fn ack(&mut self, s: u32) {
        if s > self.acked {
            self.acked = s;
            self.resends = 0;
            self.buf.retain(|(q, _, _)| *q > s);
        }
    }

    /// 重送還沒被確認的段
    async fn retry(&mut self) -> Result<(), String> {
        self.resends += 1;
        if self.resends > MAX_RESENDS {
            return Err(format!("{} 沒有回應(重送多次失敗)", self.to));
        }
        // 一段都沒確認: 對方可能沒收到開頭(對方會忽略重複的開頭)
        if self.acked == 0 {
            self.conn.send(self.to, &self.head, &[]).await?;
        }
        for (seq, more, data) in self.buf.clone() {
            self.send_one(seq, more, &data).await?;
        }
        Ok(())
    }
}

/// 請求方收到的回應片段
struct Piece {
    seq: u32,
    more: bool,
    kind: Kind,
    data: Bytes,
}

/// 比 Req 先到的請求內容: (第一段收到的時間, 片段)
type EarlyBody = (Instant, Vec<(Header, Bytes)>);

/// 正在接收的遠端請求內容
struct Incoming {
    tx: UnboundedSender<Result<Bytes, io::Error>>,
    next: u32,
    /// 已經為哪一段要求重送過(避免一直要求)
    nacked: u32,
}

#[derive(Default)]
pub struct Bus {
    conn: RwLock<Option<Conn>>,
    /// 本機送出、等待回應的請求: id → 片段
    pending: Mutex<HashMap<String, UnboundedSender<Piece>>>,
    /// 處理中的遠端請求, 還在接收內容: from:id → 內容
    serving: Mutex<HashMap<String, Incoming>>,
    /// 正在送出內容(上傳或回應), 等對方回報: 對方:id → 回報
    feedback: Mutex<HashMap<String, UnboundedSender<Fb>>>,
    /// 請求方取消的
    cancelled: Mutex<HashSet<String>>,
    /// 處理過的請求(防止重放): from:id → 時間
    seen: Mutex<HashMap<String, i64>>,
    /// 比 Req 先到的請求內容(broker 可能丟掉或延遲 Req): from:id → (收到時間, 片段)
    early: Mutex<HashMap<String, EarlyBody>>,
    /// 本機的網頁 API
    router: OnceLock<Router>,
}

impl Bus {
    pub fn set_conn(&self, c: Option<Conn>) {
        let gone = c.is_none();
        *self.conn.write().unwrap() = c;
        if gone {
            // 斷線: 進行中的傳輸都失敗(丟掉 channel, 等待的一方會收到中斷)
            self.pending.lock().unwrap().clear();
            for (_, inc) in self.serving.lock().unwrap().drain() {
                let _ = inc.tx.send(Err(io::Error::other("連線中斷")));
            }
            self.feedback.lock().unwrap().clear();
        }
    }

    /// 是否有進行中的請求或傳輸(自動更新會等它們結束再重新啟動)
    pub fn busy(&self) -> bool {
        !self.pending.lock().unwrap().is_empty()
            || !self.serving.lock().unwrap().is_empty()
            || !self.feedback.lock().unwrap().is_empty()
    }

    pub fn conn(&self) -> Option<Conn> {
        self.conn.read().unwrap().clone()
    }

    pub fn set_router(&self, r: Router) {
        let _ = self.router.set(r);
    }

    fn feedback_channel(&self, key: &str) -> UnboundedReceiver<Fb> {
        let (tx, rx) = unbounded_channel();
        self.feedback.lock().unwrap().insert(key.to_string(), tx);
        rx
    }

    // ------------------------------------------------------------ 請求方

    /// 送出一個 HTTP 請求給 `to`, 回傳它的回應(內容邊收邊給)
    pub async fn http(self: &Arc<Self>, to: &str, req: Request<Body>) -> Result<Response<Body>, String> {
        let conn = self.conn().ok_or("cluster 沒有連線")?;
        if to == conn.me {
            return Err("不能轉給自己".into());
        }
        let id = conn.cipher.random_hex(8);
        let (tx, mut rx) = unbounded_channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let fb_key = format!("{to}:{id}");
        let fb = self.feedback_channel(&fb_key);
        let guard = Pending {
            bus: self.clone(),
            conn: conn.clone(),
            to: to.to_string(),
            id: id.clone(),
            done: false,
        };

        let (parts, body) = req.into_parts();
        let headers = parts
            .headers
            .iter()
            .filter(|(k, _)| PASS_REQ.contains(&k.as_str()))
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let uri = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string();
        let method = parts.method.to_string();
        let head = conn.header(to, &id, 0, true, Kind::Req { method, uri, headers });
        conn.send(to, &head, &[]).await?;

        // 請求內容(上傳): 每段標 more, 最後一段(可能是空的)標結束
        {
            let mut out = Outgoing::new(&conn, to, &id, Kind::Body, fb, head);
            let mut stream = body.into_data_stream();
            let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
            let mut total = 0u64;
            while let Some(b) = stream.next().await {
                let b = b.map_err(|e| format!("讀取請求內容失敗: {e}"))?;
                total += b.len() as u64;
                if total > LIMIT {
                    return Err(format!("超過 {} MB 的上限", LIMIT / 1024 / 1024));
                }
                buf.extend_from_slice(&b);
                while buf.len() >= CHUNK {
                    let chunk: Vec<u8> = buf.drain(..CHUNK).collect();
                    out.push(chunk, true).await?;
                }
            }
            out.push(buf, false).await?;
        }
        self.feedback.lock().unwrap().remove(&fb_key);

        // 回應開始。broker 可能讓內容比開頭(Res)先到, 或丟掉開頭(對方會重送): 內容先放著
        let deadline = tokio::time::Instant::now() + FIRST_TIMEOUT;
        let mut early: Vec<Piece> = Vec::new();
        let first = loop {
            let p = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .map_err(|_| format!("{to} 沒有回應(可能離線或還沒更新到支援這個功能的版本)"))?
                .ok_or("連線中斷")?;
            match p.kind {
                Kind::Res { .. } | Kind::Err { .. } => break p,
                Kind::Rbody if early.len() < WINDOW as usize * 4 => early.push(p),
                _ => {}
            }
        };
        let (status, headers) = match first.kind {
            Kind::Res { status, headers } => (status, headers),
            Kind::Err { msg } => return Err(msg),
            _ => return Err("回應格式錯誤".into()),
        };
        let mut res = Response::builder()
            .status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
        for (k, v) in headers {
            if let (Ok(k), Ok(v)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                res = res.header(k, v);
            }
        }
        // 回應內容: 邊收邊給
        early.sort_by_key(|p| p.seq);
        let stream = response_stream(guard, rx, first.more, early);
        res.body(Body::from_stream(stream)).map_err(|e| e.to_string())
    }

    /// 簡單的請求: 回傳 (狀態碼, 內容)
    pub async fn call(
        self: &Arc<Self>,
        to: &str,
        method: Method,
        uri: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> Result<(u16, Vec<u8>), String> {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        let req = req.body(Body::from(body)).map_err(|e| e.to_string())?;
        let res = self.http(to, req).await?;
        let status = res.status().as_u16();
        let mut out = Vec::new();
        let mut s = res.into_body().into_data_stream();
        while let Some(b) = s.next().await {
            out.extend_from_slice(&b.map_err(|e| e.to_string())?);
        }
        Ok((status, out))
    }

    // ------------------------------------------------------------ 收到訊息

    /// 處理收件匣的一則訊息(已解密)
    pub fn on_message(self: &Arc<Self>, plain: &[u8]) {
        let Some((h, data)) = decode(plain) else {
            return;
        };
        let Some(conn) = self.conn() else { return };
        if h.to != conn.me {
            return;
        }
        let key = format!("{}:{}", h.from, h.id);
        if test_drop(&h) {
            return;
        }
        match h.kind {
            Kind::Req { method, uri, headers } => {
                // 防止重放: 時間要接近, 也不能是處理過的
                let t = now();
                {
                    let mut seen = self.seen.lock().unwrap();
                    seen.retain(|_, ts| t - *ts < MAX_SKEW * 2);
                    if (h.ts - t).abs() > MAX_SKEW || seen.contains_key(&key) {
                        return;
                    }
                    seen.insert(key.clone(), t);
                }
                let (tx, rx) = unbounded_channel();
                self.serving.lock().unwrap().insert(
                    key.clone(),
                    Incoming {
                        tx,
                        next: 1,
                        nacked: 0,
                    },
                );
                let early = self.early.lock().unwrap().remove(&key);
                let bus = self.clone();
                let (from, id) = (h.from.clone(), h.id.clone());
                let c2 = conn.clone();
                tokio::spawn(async move {
                    bus.serve(c2, from, id, method, uri, headers, rx).await;
                });
                if let Some((_, frames)) = early {
                    let mut frames = frames;
                    frames.sort_by_key(|(h, _)| h.seq);
                    for (h, data) in frames {
                        self.on_body(&conn, &key, h, data);
                    }
                }
            }
            Kind::Body => self.on_body(&conn, &key, h, data),
            Kind::Cancel => {
                self.cancelled.lock().unwrap().insert(key.clone());
                self.serving.lock().unwrap().remove(&key);
                self.feedback.lock().unwrap().remove(&key);
            }
            Kind::Ack | Kind::Nack => {
                let fb = if matches!(h.kind, Kind::Ack) {
                    Fb::Ack(h.seq)
                } else {
                    Fb::Nack(h.seq)
                };
                if let Some(tx) = self.feedback.lock().unwrap().get(&key) {
                    let _ = tx.send(fb);
                }
            }
            kind @ (Kind::Res { .. } | Kind::Rbody | Kind::Err { .. }) => {
                if let Some(tx) = self.pending.lock().unwrap().get(&h.id) {
                    let _ = tx.send(Piece {
                        seq: h.seq,
                        more: h.more,
                        kind,
                        data,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------ 處理遠端請求

    /// 請求內容的一段
    fn on_body(&self, conn: &Conn, key: &str, h: Header, data: Bytes) {
        let mut serving = self.serving.lock().unwrap();
        let Some(inc) = serving.get_mut(key) else {
            drop(serving);
            if self.seen.lock().unwrap().contains_key(key) {
                // 已經處理完(例如重送的最後一段): 告訴對方都收到了
                if !h.more {
                    conn.try_send(&h.from, &conn.header(&h.from, &h.id, h.seq, false, Kind::Ack));
                }
            } else {
                // Req 還沒到(broker 延遲或丟掉了): 先放著, Req 到了再處理
                let mut early = self.early.lock().unwrap();
                early.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(30));
                if early.len() < 64 {
                    let e = early.entry(key.to_string()).or_insert_with(|| (Instant::now(), Vec::new()));
                    if e.1.len() < WINDOW as usize * 2 {
                        e.1.push((h, data));
                    }
                }
            }
            return;
        };
        let reply = |seq: u32, kind: Kind| {
            conn.try_send(&h.from, &conn.header(&h.from, &h.id, seq, false, kind));
        };
        if h.seq < inc.next {
            reply(inc.next - 1, Kind::Ack); // 重複收到: 再回報一次
            return;
        }
        if h.seq > inc.next {
            // 少了一段: 請對方從那段重送(同一段只要求一次)
            if inc.nacked != inc.next {
                inc.nacked = inc.next;
                reply(inc.next, Kind::Nack);
            }
            return;
        }
        inc.next += 1;
        if !data.is_empty() {
            let _ = inc.tx.send(Ok(data));
        }
        if h.seq.is_multiple_of(ACK_EVERY) || !h.more {
            reply(h.seq, Kind::Ack);
        }
        if !h.more {
            serving.remove(key);
        }
        }


    #[allow(clippy::too_many_arguments)]
    async fn serve(
        self: Arc<Self>,
        conn: Conn,
        from: String,
        id: String,
        method: String,
        uri: String,
        headers: Vec<(String, String)>,
        body: UnboundedReceiver<Result<Bytes, io::Error>>,
    ) {
        let key = format!("{from}:{id}");
        let err = |seq: u32, msg: String| conn.header(&from, &id, seq, false, Kind::Err { msg });
        let path = uri.split('?').next().unwrap_or("");
        if !allowed(path) {
            let _ = conn.send(&from, &err(0, format!("不允許的路徑 {path}")), &[]).await;
            return;
        }
        let Some(router) = self.router.get().cloned() else {
            let _ = conn.send(&from, &err(0, "這台還沒準備好網頁 API".into()), &[]).await;
            return;
        };
        let mut req = Request::builder()
            .method(Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET))
            .uri(&uri);
        for (k, v) in &headers {
            if PASS_REQ.contains(&k.to_ascii_lowercase().as_str()) {
                req = req.header(k.as_str(), v.as_str());
            }
        }
        let body = Body::from_stream(tokio_stream::wrappers::UnboundedReceiverStream::new(body));
        let Ok(mut req) = req.body(body) else {
            let _ = conn.send(&from, &err(0, "請求格式錯誤".into()), &[]).await;
            return;
        };
        req.extensions_mut()
            .insert(Peer(PeerAddr::Cluster(from.clone())));
        // Router 不會失敗(錯誤型別是 Infallible)
        let res = match router.oneshot(req).await {
            Ok(r) => r,
            Err(e) => match e {},
        };
        // 請求內容還沒收完就有回應(例如上傳被拒絕): 告訴對方不用再送了
        if self.serving.lock().unwrap().remove(&key).is_some() {
            conn.try_send(&from, &conn.header(&from, &id, u32::MAX, false, Kind::Ack));
        }

        let (parts, body) = res.into_parts();
        let start = conn.header(
            &from,
            &id,
            0,
            true,
            Kind::Res {
                status: parts.status.as_u16(),
                headers: pass_headers(&parts.headers),
            },
        );
        let fb = self.feedback_channel(&key);
        let _cleanup = Cleanup {
            bus: self.clone(),
            key: key.clone(),
        };
        if conn.send(&from, &start, &[]).await.is_err() {
            return;
        }
        let mut out = Outgoing::new(&conn, &from, &id, Kind::Rbody, fb, start);
        let mut stream = body.into_data_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
        let mut total = 0u64;
        while let Some(b) = stream.next().await {
            let b = match b {
                Ok(b) => b,
                Err(e) => {
                    let _ = conn.send(&from, &err(out.next, e.to_string()), &[]).await;
                    return;
                }
            };
            total += b.len() as u64;
            if total > LIMIT {
                let msg = format!("超過 {} MB 的上限", LIMIT / 1024 / 1024);
                let _ = conn.send(&from, &err(out.next, msg), &[]).await;
                return;
            }
            buf.extend_from_slice(&b);
            while buf.len() >= CHUNK {
                if self.cancelled.lock().unwrap().contains(&key) {
                    return;
                }
                let chunk: Vec<u8> = buf.drain(..CHUNK).collect();
                if out.push(chunk, true).await.is_err() {
                    return;
                }
            }
        }
        let _ = out.push(buf, false).await;
    }
}

/// 測試用: 環境變數 CNG6_TEST_DROP=N 時, 故意丟掉每第 N 段內容(模擬 broker 丟訊息)
fn test_drop(h: &Header) -> bool {
    static EVERY: OnceLock<u64> = OnceLock::new();
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let every = *EVERY.get_or_init(|| {
        std::env::var("CNG6_TEST_DROP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    });
    if every > 0
        && matches!(h.kind, Kind::Body | Kind::Rbody)
        && (COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1).is_multiple_of(every)
    {
        return true;
    }
    // CNG6_TEST_DROP_HEAD=N: 丟掉每第 N 個開頭(Req / Res)
    static HEAD_EVERY: OnceLock<u64> = OnceLock::new();
    static HEAD_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let every = *HEAD_EVERY.get_or_init(|| {
        std::env::var("CNG6_TEST_DROP_HEAD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    });
    every > 0
        && matches!(h.kind, Kind::Req { .. } | Kind::Res { .. })
        && (HEAD_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1).is_multiple_of(every)
}

fn pass_headers(h: &HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .filter(|(k, _)| PASS_RES.contains(&k.as_str()))
        .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
        .collect()
}

/// 處理完遠端請求時清掉記錄
struct Cleanup {
    bus: Arc<Bus>,
    key: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.bus.feedback.lock().unwrap().remove(&self.key);
        self.bus.cancelled.lock().unwrap().remove(&self.key);
    }
}

/// 等待中的請求; 沒收完就被丟掉時通知對方不用再送
struct Pending {
    bus: Arc<Bus>,
    conn: Conn,
    to: String,
    id: String,
    done: bool,
}

impl Pending {
    /// 收完了: 結束時不用通知對方取消
    fn finish(mut self) {
        self.done = true;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.bus.pending.lock().unwrap().remove(&self.id);
        self.bus
            .feedback
            .lock()
            .unwrap()
            .remove(&format!("{}:{}", self.to, self.id));
        if !self.done {
            let h = self.conn.header(&self.to, &self.id, 0, false, Kind::Cancel);
            self.conn.try_send(&self.to, &h);
        }
    }
}

/// 回應內容的 stream: 依序收下, 回報收到哪裡, 少了就要求重送
fn response_stream(
    guard: Pending,
    mut rx: UnboundedReceiver<Piece>,
    more: bool,
    early: Vec<Piece>,
) -> impl tokio_stream::Stream<Item = Result<Bytes, io::Error>> + Send + 'static {
    let (tx, out) = tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(4);
    tokio::spawn(async move {
        if !more {
            guard.finish();
            return;
        }
        let reply = |g: &Pending, seq: u32, kind: Kind| {
            g.conn.try_send(&g.to, &g.conn.header(&g.to, &g.id, seq, false, kind));
        };
        let mut expect = 1;
        let mut nacked = 0;
        let mut early = early.into_iter();
        loop {
            // 比開頭先到的內容先處理
            let next = match early.next() {
                Some(p) => Ok(Some(p)),
                None => tokio::time::timeout(CHUNK_TIMEOUT, rx.recv()).await,
            };
            let p = match next {
                Ok(Some(p)) => p,
                Ok(None) => {
                    let _ = tx.send(Err(io::Error::other("連線中斷"))).await;
                    return;
                }
                Err(_) => {
                    let _ = tx.send(Err(io::Error::other("對方沒有回應"))).await;
                    return;
                }
            };
            match p.kind {
                Kind::Err { msg } => {
                    guard.finish();
                    let _ = tx.send(Err(io::Error::other(msg))).await;
                    return;
                }
                Kind::Rbody if p.seq < expect => reply(&guard, expect - 1, Kind::Ack),
                Kind::Rbody if p.seq > expect => {
                    if nacked != expect {
                        nacked = expect;
                        reply(&guard, expect, Kind::Nack);
                    }
                }
                Kind::Rbody => {
                    expect += 1;
                    if p.seq.is_multiple_of(ACK_EVERY) || !p.more {
                        reply(&guard, p.seq, Kind::Ack);
                    }
                    if !p.data.is_empty() && tx.send(Ok(p.data)).await.is_err() {
                        return; // 不要了: guard 通知對方
                    }
                    if !p.more {
                        guard.finish();
                        return;
                    }
                }
                _ => {} // 重複的 Res 等等: 略過
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let h = Header {
            from: "a".into(),
            to: "b".into(),
            id: "1".into(),
            ts: 5,
            seq: 2,
            more: true,
            kind: Kind::Rbody,
        };
        let (h2, data) = decode(&encode(&h, b"xyz")).unwrap();
        assert_eq!((h2.from.as_str(), h2.seq, h2.more), ("a", 2, true));
        assert!(matches!(h2.kind, Kind::Rbody));
        assert_eq!(&data[..], b"xyz");
        assert!(decode(b"\0\0").is_none());
        assert!(decode(b"\0\0\0\x09{}").is_none());
    }

    #[test]
    fn allowed_paths() {
        assert!(allowed("/api/files/list"));
        assert!(allowed("/files/raw"));
        assert!(allowed("/api/cli"));
        assert!(allowed("/wallpapers/a.jpg"));
        assert!(!allowed("/api/cluster/node/x/api/cli"));
        assert!(allowed("/api/cluster/send"));
        assert!(!allowed("/api/cluster/clip"));
        assert!(!allowed("/"));
        assert!(!allowed("/api"));
    }
}
