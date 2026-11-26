//! 待辦清單的網頁 API。
//!
//! - GET    /api/todo           {items}(排序好的)
//! - POST   /api/todo           {title, priority, due?, notes?} → item
//! - PUT    /api/todo/{id}      {title?, priority?, done?, due?, notes?} → item
//! - DELETE /api/todo/{id}
//! - POST   /api/todo/clear     清掉已完成的

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::plugins::todo::{Patch, Shared};

fn err(code: StatusCode, msg: String) -> Response {
    (code, Json(serde_json::json!({ "error": msg }))).into_response()
}

pub async fn list(State(t): State<Shared>) -> Response {
    Json(serde_json::json!({ "items": t.list() })).into_response()
}

#[derive(Deserialize)]
pub struct NewItem {
    title: String,
    #[serde(default = "mid")]
    priority: u8,
    #[serde(default)]
    due: Option<String>,
    #[serde(default)]
    notes: String,
}

fn mid() -> u8 {
    2
}

pub async fn add(State(t): State<Shared>, Json(n): Json<NewItem>) -> Response {
    match t.add(&n.title, n.priority, n.due, &n.notes) {
        Ok(i) => Json(i).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn update(State(t): State<Shared>, Path(id): Path<String>, Json(p): Json<Patch>) -> Response {
    match t.update(&id, p) {
        Ok(i) => Json(i).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn remove(State(t): State<Shared>, Path(id): Path<String>) -> Response {
    match t.remove(&id) {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

pub async fn clear(State(t): State<Shared>) -> Response {
    match t.clear_done() {
        Ok(n) => Json(serde_json::json!({ "cleared": n })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}
