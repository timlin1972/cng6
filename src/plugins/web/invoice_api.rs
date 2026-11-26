//! 電子發票的網頁 API。
//!
//! - GET  /api/invoice                 {progress, items, unknown, periods}
//! - POST /api/invoice/scan            {full}: 檢查新信 / 全部重新掃描
//! - POST /api/invoice/delete          {mails: [[帳號, 信件 id]]}: 把信移到 Gmail 垃圾桶
//! - POST /api/invoice/manual          {number, date, amount?, note?, account?, msg?}
//! - POST /api/invoice/remove          {key}: 移除手動加入的
//! - POST /api/invoice/claimed         {key, claimed}
//! - POST /api/invoice/dismiss         {account, msg}: 不是發票

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::plugins::invoice::Shared;
use crate::plugins::invoice::lottery::Period;

fn ok() -> Response {
    Json(serde_json::json!({ "ok": true })).into_response()
}

fn err(code: StatusCode, msg: String) -> Response {
    (code, Json(serde_json::json!({ "error": msg }))).into_response()
}

pub async fn info(State(s): State<Shared>) -> Response {
    let periods: Vec<serde_json::Value> = s
        .periods()
        .into_iter()
        .map(|(k, w)| {
            let p = Period::parse_key(&k);
            serde_json::json!({
                "key": k,
                "label": p.map(|p| p.label()).unwrap_or_default(),
                "special": w.special,
                "grand": w.grand,
                "first": w.first,
            })
        })
        .collect();
    Json(serde_json::json!({
        "progress": s.progress(),
        "items": s.views(),
        "unknown": s.unknown(),
        "periods": periods,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct ScanBody {
    #[serde(default)]
    full: bool,
}

pub async fn scan(State(s): State<Shared>, Json(b): Json<ScanBody>) -> Response {
    s.request_scan(b.full);
    ok()
}

#[derive(Deserialize)]
pub struct DeleteBody {
    mails: Vec<(String, String)>,
}

pub async fn delete(State(s): State<Shared>, Json(b): Json<DeleteBody>) -> Response {
    match s.delete_mails(b.mails).await {
        Ok(n) => Json(serde_json::json!({ "deleted": n })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct ManualBody {
    number: String,
    date: String,
    #[serde(default)]
    amount: Option<u64>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    msg: Option<String>,
}

pub async fn manual(State(s): State<Shared>, Json(b): Json<ManualBody>) -> Response {
    match s.add_manual(&b.number, &b.date, b.amount, &b.note, b.account, b.msg) {
        Ok(()) => ok(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct KeyBody {
    key: String,
    #[serde(default)]
    claimed: bool,
}

pub async fn remove(State(s): State<Shared>, Json(b): Json<KeyBody>) -> Response {
    match s.remove_manual(&b.key) {
        Ok(()) => ok(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

pub async fn claimed(State(s): State<Shared>, Json(b): Json<KeyBody>) -> Response {
    match s.set_claimed(&b.key, b.claimed) {
        Ok(()) => ok(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

#[derive(Deserialize)]
pub struct DismissBody {
    account: String,
    msg: String,
}

pub async fn dismiss(State(s): State<Shared>, Json(b): Json<DismissBody>) -> Response {
    s.dismiss(&b.account, &b.msg);
    ok()
}
