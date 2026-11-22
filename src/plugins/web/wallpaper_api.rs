//! 桌布相關的網頁 API
//!
//! - `GET    /api/wallpaper`        → 目前桌布、版本與資料夾裡的圖片
//! - `GET    /wallpapers/{name}`    → 圖片檔
//! - `PUT    /api/wallpaper/{name}` → 上傳(body 為圖片內容), 同名時自動改名
//! - `DELETE /api/wallpaper/{name}` → 刪除
//!
//! 換桌布不在這裡: 網頁透過 `/api/cli` 執行 `wallpaper set`, 和 CLI 走同一條路。

use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::server::Peer;
use crate::plugins::wallpaper::{DEFAULT, Shared, is_safe_name};

/// 上傳大小上限
pub const MAX_UPLOAD: usize = 30 * 1024 * 1024;

#[derive(Clone)]
pub struct WpState {
    pub shared: Shared,
    pub log: Sender<String>,
}

#[derive(Serialize)]
struct Item {
    name: String,
    url: String,
    size: u64,
}

#[derive(Serialize)]
struct Info {
    showing: String,
    /// 目前桌布的網址, 內建漸層時為 null
    url: Option<String>,
    version: u64,
    dir: String,
    list: Vec<Item>,
}

/// 圖片網址, 加上修改時間讓瀏覽器在檔案換掉時重新下載
fn url_of(dir: &std::path::Path, name: &str) -> (String, u64) {
    let meta = std::fs::metadata(dir.join(name)).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let encoded: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    (format!("/wallpapers/{encoded}?v={mtime}"), size)
}

pub async fn info(State(st): State<WpState>) -> Response {
    let s = st.shared.get();
    let list: Vec<Item> = st
        .shared
        .list()
        .into_iter()
        .map(|name| {
            let (url, size) = url_of(&s.dir, &name);
            Item { name, url, size }
        })
        .collect();
    let url = (s.showing != DEFAULT && s.dir.join(&s.showing).is_file())
        .then(|| url_of(&s.dir, &s.showing).0);
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(Info {
            showing: s.showing,
            url,
            version: s.version,
            dir: s.dir.display().to_string(),
            list,
        }),
    )
        .into_response()
}

fn content_type(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

pub async fn image(State(st): State<WpState>, Path(name): Path<String>) -> Response {
    if !is_safe_name(&name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(st.shared.get().dir.join(&name)).await {
        Ok(data) => (
            [
                (header::CONTENT_TYPE, content_type(&name)),
                // 網址帶有修改時間, 可以放心快取
                (header::CACHE_CONTROL, "public, max-age=604800"),
            ],
            data,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// 同名檔案存在時改成 `name-1.jpg`、`name-2.jpg`…
fn unique_name(dir: &std::path::Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (1..)
        .map(|i| format!("{stem}-{i}.{ext}"))
        .find(|n| !dir.join(n).exists())
        .unwrap()
}

#[derive(Serialize)]
struct Uploaded {
    name: String,
}

pub async fn upload(
    State(st): State<WpState>,
    Extension(Peer(peer)): Extension<Peer>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    if !is_safe_name(&name) {
        return (
            StatusCode::BAD_REQUEST,
            "只接受 jpg / png / webp / gif / avif 圖片",
        )
            .into_response();
    }
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "檔案是空的").into_response();
    }
    let dir = st.shared.get().dir;
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("無法建立資料夾 {}: {e}", dir.display()),
        )
            .into_response();
    }
    let name = unique_name(&dir, &name);
    // 先寫到暫存檔再改名, 避免網頁讀到寫一半的圖片
    let tmp = dir.join(format!(".{name}.uploading"));
    let result = async {
        tokio::fs::write(&tmp, &body).await?;
        tokio::fs::rename(&tmp, dir.join(&name)).await
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("無法寫入: {e}")).into_response();
    }
    let _ = st.log.send(format!(
        "web {}: 上傳桌布 {name} ({} KB)",
        peer.ip(),
        body.len() / 1024
    ));
    Json(Uploaded { name }).into_response()
}

pub async fn delete(
    State(st): State<WpState>,
    Extension(Peer(peer)): Extension<Peer>,
    Path(name): Path<String>,
) -> Response {
    if !is_safe_name(&name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::remove_file(st.shared.get().dir.join(&name)).await {
        Ok(()) => {
            let _ = st.log.send(format!("web {}: 刪除桌布 {name}", peer.ip()));
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, format!("無法刪除: {e}")).into_response(),
    }
}
