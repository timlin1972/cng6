//! 終端機的網頁 API(長輪詢, 透過 cluster 代理也能用)。
//!
//! - GET    /api/term                      {enabled, sessions}
//! - POST   /api/term                      {cols, rows} → {id}
//! - GET    /api/term/{id}/out?from=N      等到有輸出(最多 20 秒)→ {data(base64), next, ended}
//! - POST   /api/term/{id}/in              {data, client, seq}: 鍵盤輸入(依序號寫入, 網頁不用等上一個)
//! - POST   /api/term/{id}/resize          {cols, rows}
//! - DELETE /api/term/{id}
//! - GET    /xterm/{file}                  xterm.js(內嵌, 離線也能用)

use std::time::Duration;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::server::Peer;
use crate::plugins::terminal::Shared;

/// 長輪詢最多等多久(cluster 代理等第一個回應的上限是 45 秒)
const WAIT: Duration = Duration::from_secs(20);

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

pub async fn list(State(t): State<Shared>) -> Response {
    Json(serde_json::json!({ "enabled": t.enabled(), "sessions": t.list() })).into_response()
}

#[derive(Deserialize)]
pub struct Size {
    cols: u16,
    rows: u16,
}

pub async fn open(State(t): State<Shared>, peer: Option<Extension<Peer>>, Json(s): Json<Size>) -> Response {
    // 誰開的(紀錄用): 網頁的 IP, 或 cluster:<機器>
    let who = peer.map_or_else(|| "網頁".to_string(), |p| p.0.0.ip());
    match tokio::task::spawn_blocking(move || t.open(s.cols, s.rows, &who)).await {
        Ok(Ok(id)) => Json(serde_json::json!({ "id": id })).into_response(),
        Ok(Err(e)) => err(StatusCode::FORBIDDEN, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct OutQuery {
    #[serde(default)]
    from: u64,
}

pub async fn output(State(t): State<Shared>, Path(id): Path<u32>, Query(q): Query<OutQuery>) -> Response {
    let Some(s) = t.get(id) else {
        return err(StatusCode::NOT_FOUND, "這個終端機已經不在了");
    };
    let c = s.wait_output(q.from, WAIT).await;
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "data": super::page::base64(&c.data),
            "next": c.next,
            "ended": c.ended,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct Input {
    data: String,
    /// 有序號時依序寫入(請求可能不照順序到)
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    seq: Option<u64>,
}

pub async fn input(State(t): State<Shared>, Path(id): Path<u32>, Json(i): Json<Input>) -> Response {
    let Some(s) = t.get(id) else {
        return err(StatusCode::NOT_FOUND, "這個終端機已經不在了");
    };
    // shell 沒在讀的時候寫入可能會卡住: 放到 blocking thread
    let write = move || match (i.client, i.seq) {
        (Some(c), Some(n)) => s.write_ordered(&c, n, i.data.into_bytes()),
        _ => s.write_input(i.data.as_bytes()),
    };
    match tokio::task::spawn_blocking(write).await {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(Err(e)) => err(StatusCode::GONE, e.to_string()),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn resize(State(t): State<Shared>, Path(id): Path<u32>, Json(sz): Json<Size>) -> Response {
    let Some(s) = t.get(id) else {
        return err(StatusCode::NOT_FOUND, "這個終端機已經不在了");
    };
    match s.resize(sz.cols.clamp(10, 500), sz.rows.clamp(4, 300)) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::GONE, e.to_string()),
    }
}

pub async fn close(State(t): State<Shared>, Path(id): Path<u32>) -> Response {
    if t.close(id) {
        Json(serde_json::json!({ "ok": true })).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "這個終端機已經不在了")
    }
}

/// xterm.js(@xterm/xterm 5.5.0、@xterm/addon-fit 0.10.0, MIT), 檔案在 assets/xterm/
pub async fn xterm_file(Path(name): Path<String>) -> Response {
    let (ctype, body): (&str, &[u8]) = match name.as_str() {
        "xterm.js" => ("text/javascript; charset=utf-8", include_bytes!("assets/xterm/xterm.js")),
        "xterm.css" => ("text/css; charset=utf-8", include_bytes!("assets/xterm/xterm.css")),
        "addon-fit.js" => ("text/javascript; charset=utf-8", include_bytes!("assets/xterm/addon-fit.js")),
        _ => return err(StatusCode::NOT_FOUND, "沒有這個檔案"),
    };
    (
        [
            (header::CONTENT_TYPE, ctype),
            (header::CACHE_CONTROL, "public, max-age=604800"),
        ],
        body,
    )
        .into_response()
}
