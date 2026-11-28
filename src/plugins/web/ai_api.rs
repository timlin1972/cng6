//! AI 的網頁 API(/api/ai/...)。
//!
//! API key 不在這台時(ai.node 指向別台), 整個請求經由 cluster 轉給那台處理;
//! 只有設定 key(/api/ai/key)永遠在這台做 —— 在哪台設定, AI 就放在哪台。

use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::plugins::ai::{self, Image, Shared};

/// 圖片會讓請求變大(最多 4 張, 每張 4 MB)
const BODY_LIMIT: usize = 24 * 1024 * 1024;
/// long-poll 最多等多久
const WAIT_SECS: u64 = 20;

pub fn router(ai: Shared) -> Router {
    Router::new()
        .route("/api/ai", get(info))
        .route("/api/ai/key", post(set_key))
        .route("/api/ai/models", get(models))
        .route("/api/ai/ask", post(ask))
        .route("/api/ai/chats", post(create))
        .route("/api/ai/chats/{id}", get(view).delete(delete).patch(rename))
        .route("/api/ai/chats/{id}/send", post(send))
        .route("/api/ai/chats/{id}/stop", post(stop))
        .route("/api/ai/chats/{id}/retry", post(retry))
        .route("/api/ai/chats/{id}/img/{c}/{p}", get(image))
        .route_layer(axum::middleware::from_fn_with_state(ai.clone(), dispatch))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(ai)
}

/// 在這台處理, 或轉給放 API key 的那台
async fn dispatch(State(ai): State<Shared>, req: Request, next: axum::middleware::Next) -> Response {
    let hop = req.headers().contains_key(ai::HOP_HEADER);
    let key_route = req.uri().path() == "/api/ai/key";
    match ai.remote() {
        Some(node) if !hop && !key_route => forward(&ai, &node, req).await,
        _ => next.run(req).await,
    }
}

async fn forward(ai: &Shared, node: &str, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path_and_query().map_or("/api/ai", |p| p.as_str()).to_string();
    let mut fwd = axum::http::Request::builder().method(parts.method.clone()).uri(path);
    for (k, v) in parts.headers.iter() {
        fwd = fwd.header(k, v);
    }
    let Ok(fwd) = fwd.header(ai::HOP_HEADER, "1").body(body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match ai.cluster().bus.http(node, fwd).await {
        Ok(res) => res.into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("AI 在 {node}, 目前連不到: {e}")),
    }
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

fn done(r: Result<(), String>) -> Response {
    match r {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct After {
    after: Option<u64>,
}

/// 有 after 而且版本還一樣時, 等到有變化(long-poll)
async fn wait(ai: &Shared, after: Option<u64>) -> u64 {
    match after {
        Some(a) if a == ai.version() => ai.wait(a, WAIT_SECS).await,
        _ => ai.version(),
    }
}

async fn info(State(ai): State<Shared>, Query(q): Query<After>) -> Response {
    let ver = wait(&ai, q.after).await;
    Json(json!({
        "configured": ai.configured(),
        "model": ai.model(),
        "node": ai.me(),
        "chats": ai.list(),
        "ver": ver,
        "max_images": ai::MAX_IMAGES,
        "max_image_bytes": ai::MAX_IMAGE_BYTES,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct KeyReq {
    key: String,
}

async fn set_key(State(ai): State<Shared>, Json(r): Json<KeyReq>) -> Response {
    match tokio::task::spawn_blocking(move || ai.set_key(&r.key)).await {
        Ok(r) => done(r),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn models(State(ai): State<Shared>) -> Response {
    match tokio::task::spawn_blocking(move || ai.models()).await {
        Ok(Ok(m)) => Json(json!({ "models": m })).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_GATEWAY, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct AskReq {
    chat: Option<String>,
    text: String,
}

async fn ask(State(ai): State<Shared>, Json(r): Json<AskReq>) -> Response {
    match ai.ask(r.chat, &r.text).await {
        Ok((chat, text)) => Json(json!({ "chat": chat, "text": text })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize, Default)]
struct SendReq {
    #[serde(default)]
    text: String,
    #[serde(default)]
    images: Vec<Image>,
    /// 顯示用的文字(例如「摘要這封信」), 實際送給 AI 的是 text
    display: Option<String>,
}

async fn create(State(ai): State<Shared>, body: Option<Json<SendReq>>) -> Response {
    let r = body.map(|b| b.0).unwrap_or_default();
    let id = ai.create();
    if (!r.text.trim().is_empty() || !r.images.is_empty())
        && let Err(e) = ai.send(&id, &r.text, r.images, r.display)
    {
        let _ = ai.delete(&id);
        return err(StatusCode::BAD_REQUEST, e);
    }
    Json(json!({ "id": id })).into_response()
}

async fn view(
    State(ai): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<After>,
    headers: axum::http::HeaderMap,
) -> Response {
    let mut ver = wait(&ai, q.after).await;
    // 經由 cluster 來的: 每次往返比較慢(MQTT), 多等一下把幾段文字一起送, 減少請求次數
    if q.after.is_some() && headers.contains_key(ai::HOP_HEADER) && ver != q.after.unwrap_or(0) {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        ver = ai.version();
    }
    match ai.view(&id) {
        Some(v) => Json(json!({ "ver": ver, "chat": v })).into_response(),
        None => err(StatusCode::NOT_FOUND, "找不到這個對話"),
    }
}

async fn delete(State(ai): State<Shared>, Path(id): Path<String>) -> Response {
    done(ai.delete(&id))
}

#[derive(Deserialize)]
struct RenameReq {
    title: String,
}

async fn rename(State(ai): State<Shared>, Path(id): Path<String>, Json(r): Json<RenameReq>) -> Response {
    done(ai.rename(&id, &r.title))
}

async fn send(State(ai): State<Shared>, Path(id): Path<String>, Json(r): Json<SendReq>) -> Response {
    done(ai.send(&id, &r.text, r.images, r.display))
}

async fn stop(State(ai): State<Shared>, Path(id): Path<String>) -> Response {
    ai.stop(&id);
    done(Ok(()))
}

async fn retry(State(ai): State<Shared>, Path(id): Path<String>) -> Response {
    done(ai.retry(&id))
}

async fn image(State(ai): State<Shared>, Path((id, c, p)): Path<(String, usize, usize)>) -> Response {
    let Some((mime, data)) = ai.image(&id, c, p) else {
        return err(StatusCode::NOT_FOUND, "找不到圖片");
    };
    let Some(bytes) = super::push::b64url_decode(&data) else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "圖片格式不對");
    };
    let mime = if mime.starts_with("image/") && !mime.contains("svg") { mime } else { "application/octet-stream".into() };
    (
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, max-age=86400".into())],
        bytes,
    )
        .into_response()
}
