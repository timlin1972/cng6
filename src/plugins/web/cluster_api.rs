//! Cluster 的網頁 API
//!
//! - `GET  /api/cluster`                    → 群組裡每台機器的狀態、通知、剪貼簿
//! - `POST /api/cluster/clip`               → 送到共用剪貼簿 `{text}`
//! - `POST /api/cluster/send`               → 把本機的檔案傳到另一台 `{node, root, paths}`
//! - `ANY  /api/cluster/node/{name}/{*rest}` → 轉給另一台的網頁 API(遠端 CLI、遠端檔案)

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::plugins::cluster::Shared;
use crate::plugins::cluster::bus::LIMIT;
use crate::plugins::files::{self, fsops};

#[derive(Clone)]
pub struct ClusterState {
    pub shared: Shared,
    pub files: files::Shared,
}

pub async fn info(State(st): State<ClusterState>) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(st.shared.snapshot())).into_response()
}

#[derive(Deserialize)]
pub struct ClipReq {
    text: String,
}

pub async fn clip(State(st): State<ClusterState>, Json(req): Json<ClipReq>) -> Response {
    match st.shared.clip(req.text) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// 轉給另一台: `/api/cluster/node/pi4/api/files/list?root=0` → pi4 的 `/api/files/list?root=0`
pub async fn proxy(
    State(st): State<ClusterState>,
    Path((name, rest)): Path<(String, String)>,
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let query = parts.uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let mut fwd = Request::builder()
        .method(parts.method.clone())
        .uri(format!("/{rest}{query}"));
    for (k, v) in parts.headers.iter() {
        fwd = fwd.header(k, v);
    }
    let Ok(fwd) = fwd.body(body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match st.shared.bus.http(&name, fwd).await {
        Ok(res) => res.into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SendReq {
    node: String,
    root: usize,
    paths: Vec<String>,
}

#[derive(Serialize)]
struct SendRes {
    message: String,
    errors: Vec<String>,
}

/// 對方收到的檔案放在第一個根目錄的這個資料夾
const INBOX: &str = "收到的檔案";

/// 把本機的檔案傳到另一台(放在對方的「收到的檔案」)
pub async fn send(State(st): State<ClusterState>, Json(req): Json<SendReq>) -> Response {
    let Some(root) = st.files.root(req.root) else {
        return (StatusCode::BAD_REQUEST, "沒有這個根目錄").into_response();
    };
    let bus = st.shared.bus.clone();
    // 對方的資料夾(已經有就沒關係)
    let mkdir = serde_json::json!({ "op": "mkdir", "root": 0, "path": "", "name": INBOX });
    if let Err(e) = bus
        .call(&req.node, Method::POST, "/api/files/op", Some("application/json"), mkdir.to_string().into_bytes())
        .await
    {
        return (StatusCode::BAD_GATEWAY, e).into_response();
    }
    let mut sent = Vec::new();
    let mut errors = Vec::new();
    for p in &req.paths {
        let path = match fsops::resolve(&root.path, p) {
            Ok(path) if path.is_file() => path,
            Ok(_) => {
                errors.push(format!("{p}: 資料夾請先壓縮成 zip 再傳"));
                continue;
            }
            Err(e) => {
                errors.push(format!("{p}: {e}"));
                continue;
            }
        };
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > LIMIT {
            errors.push(format!("{p}: 超過 {} MB 的上限", LIMIT / 1024 / 1024));
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // 對方連線短暫中斷時重試(每次重新讀檔)
        let mut last = String::new();
        let mut ok = false;
        for attempt in 1..=3 {
            match upload_once(&bus, &req.node, &path, &name).await {
                Ok(()) => {
                    ok = true;
                    break;
                }
                Err((retry, e)) => {
                    last = e;
                    if !retry {
                        break;
                    }
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
                    }
                }
            }
        }
        if ok {
            sent.push(name);
        } else {
            errors.push(format!("{name}: {last}"));
        }
    }
    Json(SendRes {
        message: format!("已傳送 {} 個檔案到 {} 的「{INBOX}」", sent.len(), req.node),
        errors,
    })
    .into_response()
}

/// 上傳一個檔案到 `node` 的「收到的檔案」; 失敗時回傳 (可以重試, 原因)
async fn upload_once(
    bus: &std::sync::Arc<crate::plugins::cluster::bus::Bus>,
    node: &str,
    path: &std::path::Path,
    name: &str,
) -> Result<(), (bool, String)> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| (false, e.to_string()))?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(file, 128 * 1024));
    let uri = format!("/api/files/upload?root=0&path={}&name={}", enc(INBOX), enc(name));
    let fwd = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("content-type", "application/octet-stream")
        .body(body)
        .map_err(|e| (false, e.to_string()))?;
    let res = bus.http(node, fwd).await.map_err(|e| (true, e))?;
    if res.status().is_success() {
        return Ok(());
    }
    let code = res.status();
    let text = axum::body::to_bytes(res.into_body(), 4096).await.unwrap_or_default();
    let text = String::from_utf8_lossy(&text).into_owned();
    // 傳輸途中斷線(對方回「上傳中斷」)可以重試; 其他錯誤(檔名不合法等)不行
    let retry = text.contains("上傳中斷") || code.is_server_error();
    Err((retry, format!("{code} {text}")))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
