//! `POST /api/cli`: 網頁 CLI 的指令請求, 交給主迴圈的 `App` 執行後回傳結果。

use std::time::Duration;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::server::Peer;
use crate::plugin::{Level, SettingRow};
use crate::remote::{self, Candidate, Op, Request};
use crate::webui::AppInfo;

#[derive(Deserialize)]
pub struct CliRequest {
    session: String,
    /// "exec" | "complete" | "describe"
    op: String,
    line: String,
}

#[derive(Serialize)]
struct OutLine {
    level: &'static str,
    text: String,
}

#[derive(Serialize)]
struct CliResponse {
    path: Vec<String>,
    output: Vec<OutLine>,
    replacement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<Vec<SettingRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    apps: Option<Vec<AppInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<Vec<Candidate>>,
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error => "error",
        Level::Command => "command",
    }
}

pub async fn cli(
    State(tx): State<remote::Sender>,
    Extension(Peer(peer)): Extension<Peer>,
    Json(req): Json<CliRequest>,
) -> Response {
    let op = match req.op.as_str() {
        "exec" => Op::Exec,
        "complete" => Op::Complete,
        "describe" => Op::Describe,
        "settings" => Op::Settings,
        "apps" => Op::Apps,
        "candidates" => Op::Candidates,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "op 必須是 exec / complete / describe / settings / apps / candidates",
            )
                .into_response();
        }
    };
    let (reply_tx, reply_rx) = oneshot::channel();
    let sent = tx.send(Request {
        session: req.session.chars().take(64).collect(),
        peer: peer.ip().to_string(),
        op,
        line: req.line,
        reply: reply_tx,
    });
    if sent.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "cng6 沒有在執行").into_response();
    }
    match tokio::time::timeout(Duration::from_secs(10), reply_rx).await {
        Ok(Ok(reply)) => Json(CliResponse {
            path: reply.path,
            output: reply
                .output
                .into_iter()
                .map(|(level, text)| OutLine {
                    level: level_name(level),
                    text,
                })
                .collect(),
            replacement: reply.replacement,
            settings: reply.settings,
            apps: reply.apps,
            candidates: reply.candidates,
        })
        .into_response(),
        _ => (StatusCode::GATEWAY_TIMEOUT, "指令沒有回應").into_response(),
    }
}
