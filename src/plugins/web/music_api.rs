//! 音樂相關的網頁 API
//!
//! - `GET /api/music`               → 曲目、播放清單、下載進度
//! - `GET /music/file/{name}`       → 音樂檔(支援 Range, 可以拖曳進度)
//! - `GET /music/cover/{name}`      → 曲目的封面(封面檔或內嵌封面)
//! - `GET /music/lyrics/{name}`     → 歌詞檔(srt / lrc / vtt)
//!
//! 下載、刪除與播放清單的修改透過 `/api/cli` 執行 `music ...` 指令, 和 CLI 走同一條路。

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::plugins::music::library::{self, Track};
use crate::plugins::music::{JobView, Shared};

#[derive(Serialize)]
struct Playlist {
    name: String,
    files: Vec<String>,
}

#[derive(Serialize)]
struct Info {
    version: u64,
    dir: String,
    tracks: Vec<Track>,
    playlists: Vec<Playlist>,
    downloads: Vec<JobView>,
}

pub async fn info(State(shared): State<Shared>) -> Response {
    // 掃描資料夾會讀檔, 放到 blocking thread
    let res = tokio::task::spawn_blocking(move || {
        let dir = shared.dir();
        Info {
            version: shared.version(),
            dir: dir.display().to_string(),
            tracks: shared.tracks(),
            playlists: library::playlists(&dir)
                .into_iter()
                .map(|(name, files)| Playlist { name, files })
                .collect(),
            downloads: shared.jobs(),
        }
    })
    .await;
    match res {
        Ok(info) => ([(header::CACHE_CONTROL, "no-store")], Json(info)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// 用 tower-http 送出檔案(處理 Range、Last-Modified、Content-Type)
async fn serve(
    path: std::path::PathBuf,
    req: Request,
    content_type: Option<&'static str>,
) -> Response {
    let svc = match content_type {
        Some(ct) => ServeFile::new_with_mime(path, &ct.parse().unwrap()),
        None => ServeFile::new(path),
    };
    match svc.oneshot(req).await {
        Ok(res) => res.map(Body::new).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn audio_mime(name: &str) -> Option<&'static str> {
    // 有些格式 mime_guess 判斷不出來或不是瀏覽器認得的
    match name
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("opus") => Some("audio/ogg"),
        Some("oga" | "ogg") => Some("audio/ogg"),
        Some("m4a") => Some("audio/mp4"),
        Some("webm") => Some("audio/webm"),
        Some("flac") => Some("audio/flac"),
        _ => None,
    }
}

pub async fn file(
    State(shared): State<Shared>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    if !library::is_safe_name(&name) || !library::is_audio(&name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(shared.dir().join(&name), req, audio_mime(&name)).await
}

pub async fn cover(
    State(shared): State<Shared>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    if !library::is_safe_name(&name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dir = shared.dir();
    let track = tokio::task::spawn_blocking({
        let shared = shared.clone();
        let name = name.clone();
        move || shared.tracks().into_iter().find(|t| t.file == name)
    })
    .await
    .ok()
    .flatten();
    let Some(track) = track else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match track.cover.as_deref() {
        Some("embedded") => {
            let path = dir.join(&name);
            match tokio::task::spawn_blocking(move || library::embedded_cover(&path)).await {
                Ok(Some((data, mime))) => (
                    [
                        (header::CONTENT_TYPE, mime),
                        (header::CACHE_CONTROL, "max-age=86400".to_string()),
                    ],
                    data,
                )
                    .into_response(),
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }
        Some(file) => serve(dir.join(file), req, None).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn lyrics(
    State(shared): State<Shared>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    let ok = library::is_safe_name(&name)
        && matches!(
            name.rsplit('.')
                .next()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("srt" | "lrc" | "vtt")
        );
    if !ok {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(
        shared.dir().join(&name),
        req,
        Some("text/plain; charset=utf-8"),
    )
    .await
}
