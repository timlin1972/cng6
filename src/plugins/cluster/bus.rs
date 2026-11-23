//! 機器之間的請求/回應: 把 HTTP 請求透過加密的 MQTT 轉給另一台,
//! 由那台用自己的網頁 API(本機的 Router)處理, 再把回應分段傳回來。
//!
//! 遠端 CLI、遠端檔案、檔案傳送、同步桌布圖片都走這裡。
//!
//! - 每一則訊息(加密前): `[4 bytes: header 長度][header JSON][資料]`, 送到收件者的收件匣
//! - 大的內容切成 [`CHUNK`] 一段, 依序編號; 收到的順序不對就中止, 不會產生壞掉的檔案
//! - 流量控制: 接收方每收到 [`ACK_EVERY`] 段回報一次, 送出方最多領先 [`WINDOW`] 段,
//!   避免一次塞太多訊息給 broker

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

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
const WINDOW: u32 = 16;
/// 接收方每幾段回報一次
const ACK_EVERY: u32 = 4;
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
        && !path.starts_with("/api/cluster")
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
    /// 收到第 seq 段了(流量控制)
    Ack,
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

/// 連線中的 MQTT
#[derive(Clone)]
pub struct Conn {
    pub client: AsyncClient,
    pub cipher: Arc<Cipher>,
    pub group: String,
    pub me: String,
}

impl Conn {
    async fn send(&self, to: &str, h: &Header, data: &[u8]) -> Result<(), String> {
        let topic = self.cipher.inbox_topic(&self.group, to);
        let payload = self.cipher.seal(&topic, &encode(h, data));
        self.client
            .publish(topic, QoS::AtLeastOnce, false, payload)
            .await
            .map_err(|e| e.to_string())
    }

    /// 不等待的送出(在同步的程式裡用)
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

/// 請求方收到的回應片段
struct Piece {
    seq: u32,
    more: bool,
    kind: Kind,
    data: Bytes,
}

/// 正在接收的遠端請求內容
struct Incoming {
    tx: UnboundedSender<Result<Bytes, io::Error>>,
    next: u32,
}

#[derive(Default)]
pub struct Bus {
    conn: RwLock<Option<Conn>>,
    /// 本機送出、等待回應的請求: id → 片段
    pending: Mutex<HashMap<String, UnboundedSender<Piece>>>,
    /// 處理中的遠端請求, 還在接收內容: from:id → 內容
    serving: Mutex<HashMap<String, Incoming>>,
    /// 處理中的遠端請求, 正在送出回應: from:id → 對方確認到第幾段
    acks: Mutex<HashMap<String, UnboundedSender<u32>>>,
    /// 請求方取消的
    cancelled: Mutex<HashSet<String>>,
    /// 處理過的請求(防止重放): from:id → 時間
    seen: Mutex<HashMap<String, i64>>,
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
            self.acks.lock().unwrap().clear();
        }
    }

    pub fn conn(&self) -> Option<Conn> {
        self.conn.read().unwrap().clone()
    }

    pub fn set_router(&self, r: Router) {
        let _ = self.router.set(r);
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
        conn.send(to, &conn.header(to, &id, 0, true, Kind::Req { method, uri, headers }), &[])
            .await?;

        // 請求內容(上傳): 每段標 more, 最後一段(可能是空的)標結束
        let mut early: Option<Piece> = None; // 上傳中就收到的回應(例如被拒絕)
        let mut acked = 0u32;
        let mut stream = body.into_data_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
        let mut seq = 1u32;
        let mut total = 0u64;
        let mut ended = false;
        while !ended && early.is_none() {
            match stream.next().await {
                Some(Ok(b)) => {
                    total += b.len() as u64;
                    if total > LIMIT {
                        return Err(format!("超過 {} MB 的上限", LIMIT / 1024 / 1024));
                    }
                    buf.extend_from_slice(&b);
                }
                Some(Err(e)) => return Err(format!("讀取請求內容失敗: {e}")),
                None => ended = true,
            }
            while buf.len() >= CHUNK || (ended && early.is_none()) {
                let n = buf.len().min(CHUNK);
                let chunk: Vec<u8> = buf.drain(..n).collect();
                let more = !(ended && buf.is_empty());
                conn.send(to, &conn.header(to, &id, seq, more, Kind::Body), &chunk)
                    .await?;
                // 流量控制: 領先太多就等對方確認
                while seq.saturating_sub(acked) >= WINDOW {
                    let p = tokio::time::timeout(CHUNK_TIMEOUT, rx.recv())
                        .await
                        .map_err(|_| format!("{to} 沒有回應"))?
                        .ok_or("連線中斷")?;
                    match p.kind {
                        Kind::Ack => acked = acked.max(p.seq),
                        _ => {
                            early = Some(p);
                            break;
                        }
                    }
                }
                seq += 1;
                if !more || early.is_some() {
                    break;
                }
            }
        }

        // 回應開始(略過上傳時的確認)
        let first = match early {
            Some(p) => p,
            None => loop {
                let p = tokio::time::timeout(FIRST_TIMEOUT, rx.recv())
                    .await
                    .map_err(|_| format!("{to} 沒有回應(可能離線或還沒更新到支援這個功能的版本)"))?
                    .ok_or("連線中斷")?;
                if !matches!(p.kind, Kind::Ack) {
                    break p;
                }
            },
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
        let stream = response_stream(guard, rx, first.more);
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
                self.serving
                    .lock()
                    .unwrap()
                    .insert(key, Incoming { tx, next: 1 });
                let bus = self.clone();
                tokio::spawn(async move {
                    bus.serve(conn, h.from, h.id, method, uri, headers, rx).await;
                });
            }
            Kind::Body => {
                let mut serving = self.serving.lock().unwrap();
                let Some(inc) = serving.get_mut(&key) else {
                    return;
                };
                if h.seq != inc.next {
                    // 少了一段: 中止, 不要產生壞掉的檔案
                    let _ = inc.tx.send(Err(io::Error::other("傳輸中斷(資料不完整)")));
                    serving.remove(&key);
                    return;
                }
                inc.next += 1;
                if !data.is_empty() {
                    let _ = inc.tx.send(Ok(data));
                }
                if h.seq % ACK_EVERY == 0 || !h.more {
                    conn.try_send(&h.from, &conn.header(&h.from, &h.id, h.seq, false, Kind::Ack));
                }
                if !h.more {
                    serving.remove(&key);
                }
            }
            Kind::Cancel => {
                self.cancelled.lock().unwrap().insert(key.clone());
                self.serving.lock().unwrap().remove(&key);
                self.acks.lock().unwrap().remove(&key);
            }
            Kind::Ack => {
                // 對方確認收到了: 可能是我們上傳的(pending), 或我們送出的回應(acks)
                if let Some(tx) = self.acks.lock().unwrap().get(&key) {
                    let _ = tx.send(h.seq);
                    return;
                }
                if let Some(tx) = self.pending.lock().unwrap().get(&h.id) {
                    let _ = tx.send(Piece {
                        seq: h.seq,
                        more: false,
                        kind: Kind::Ack,
                        data,
                    });
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
        let res = match router.oneshot(req).await {
            Ok(r) => r,
            Err(e) => {
                let _ = conn.send(&from, &err(0, e.to_string()), &[]).await;
                return;
            }
        };
        // 不需要再收請求內容了(例如上傳被拒絕)
        self.serving.lock().unwrap().remove(&key);

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
        let (ack_tx, mut ack_rx) = unbounded_channel();
        self.acks.lock().unwrap().insert(key.clone(), ack_tx);
        let _cleanup = Cleanup {
            bus: self.clone(),
            key: key.clone(),
        };
        if conn.send(&from, &start, &[]).await.is_err() {
            return;
        }
        let mut stream = body.into_data_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
        let mut seq = 1u32;
        let mut acked = 0u32;
        let mut total = 0u64;
        let mut ended = false;
        while !ended {
            match stream.next().await {
                Some(Ok(b)) => {
                    total += b.len() as u64;
                    if total > LIMIT {
                        let msg = format!("超過 {} MB 的上限", LIMIT / 1024 / 1024);
                        let _ = conn.send(&from, &err(seq, msg), &[]).await;
                        return;
                    }
                    buf.extend_from_slice(&b);
                }
                Some(Err(e)) => {
                    let _ = conn.send(&from, &err(seq, e.to_string()), &[]).await;
                    return;
                }
                None => ended = true,
            }
            while buf.len() >= CHUNK || ended {
                if self.cancelled.lock().unwrap().remove(&key) {
                    return;
                }
                let n = buf.len().min(CHUNK);
                let chunk: Vec<u8> = buf.drain(..n).collect();
                let more = !(ended && buf.is_empty());
                let h = conn.header(&from, &id, seq, more, Kind::Rbody);
                if conn.send(&from, &h, &chunk).await.is_err() {
                    return;
                }
                // 流量控制
                while more && seq.saturating_sub(acked) >= WINDOW {
                    match tokio::time::timeout(CHUNK_TIMEOUT, ack_rx.recv()).await {
                        Ok(Some(a)) => acked = acked.max(a),
                        _ => return, // 對方不見了
                    }
                }
                seq += 1;
                if !more {
                    return;
                }
            }
        }
    }
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
        self.bus.acks.lock().unwrap().remove(&self.key);
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

impl Drop for Pending {
    fn drop(&mut self) {
        self.bus.pending.lock().unwrap().remove(&self.id);
        if !self.done {
            let h = self.conn.header(&self.to, &self.id, 0, false, Kind::Cancel);
            self.conn.try_send(&self.to, &h);
        }
    }
}

/// 回應內容的 stream: 檢查順序、回報確認
fn response_stream(
    mut guard: Pending,
    mut rx: UnboundedReceiver<Piece>,
    more: bool,
) -> impl tokio_stream::Stream<Item = Result<Bytes, io::Error>> + Send + 'static {
    let (tx, out) = tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(4);
    tokio::spawn(async move {
        if !more {
            guard.done = true;
            return;
        }
        let mut expect = 1;
        loop {
            let p = match tokio::time::timeout(CHUNK_TIMEOUT, rx.recv()).await {
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
                Kind::Ack => {}
                Kind::Err { msg } => {
                    guard.done = true;
                    let _ = tx.send(Err(io::Error::other(msg))).await;
                    return;
                }
                Kind::Rbody if p.seq == expect => {
                    expect += 1;
                    if p.seq % ACK_EVERY == 0 && p.more {
                        let h = guard.conn.header(&guard.to, &guard.id, p.seq, false, Kind::Ack);
                        guard.conn.try_send(&guard.to, &h);
                    }
                    if !p.data.is_empty() && tx.send(Ok(p.data)).await.is_err() {
                        return; // 不要了: guard 通知對方
                    }
                    if !p.more {
                        guard.done = true;
                        return;
                    }
                }
                _ => {
                    let _ = tx.send(Err(io::Error::other("傳輸中斷(資料不完整)"))).await;
                    return;
                }
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
        assert!(!allowed("/"));
        assert!(!allowed("/api"));
    }
}
