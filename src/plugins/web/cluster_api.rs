//! Cluster 的網頁 API: `GET /api/cluster` → 群組裡每台機器的狀態

use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::plugins::cluster::Shared;

pub async fn info(State(shared): State<Shared>) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(shared.snapshot())).into_response()
}
