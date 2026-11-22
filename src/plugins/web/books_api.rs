//! 電子書的網頁 API
//!
//! - `GET  /api/books`                 → 書庫
//! - `GET  /api/books/{file}`          → 書本內容結構(章節、目錄)與閱讀進度
//! - `PUT  /api/books/{file}/progress` → 儲存閱讀進度與書籤
//! - `GET  /books/res/{file}/{*path}`  → EPUB 裡的檔案(章節 XHTML、圖片、CSS、字型)
//! - `GET  /books/txt/{file}/{n}`      → TXT 第 n 章(XHTML)
//! - `GET  /books/page/{file}/{n}`     → 漫畫第 n 頁
//! - `GET  /books/file/{file}`         → 原始檔(PDF, 支援 Range)
//! - `GET  /books/cover/{file}`        → 封面
//! - `PUT  /api/books/upload/{name}`   → 上傳
//! - `GET  /books/fonts/{name}.css`    → 閱讀用字型(明體、楷體), Pi 代抓 Google Fonts 並快取
//! - `GET  /books/fonts/f/{*path}`     → 字型分片(woff2)
//!
//! 刪除透過 `/api/cli` 執行 `books delete`, 和 CLI 走同一條路。

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::server::Peer;
use crate::plugins::books::Shared;
use crate::plugins::books::library::{self, Book, Content, Kind, Library, Progress};

/// 上傳大小上限
pub const MAX_UPLOAD: usize = 500 * 1024 * 1024;

#[derive(Clone)]
pub struct BooksState {
    pub shared: Shared,
    pub log: Sender<String>,
}

/// 書裡的內容不執行 script
const CSP: &str = "script-src 'none'; object-src 'none'";

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

#[derive(Serialize)]
struct List {
    dir: String,
    books: Vec<Book>,
}

pub async fn list(State(st): State<BooksState>) -> Response {
    let shared = st.shared.clone();
    match blocking(move || List {
        dir: shared.dir().display().to_string(),
        books: shared.books(),
    })
    .await
    {
        Some(l) => ([(header::CACHE_CONTROL, "no-store")], Json(l)).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Serialize)]
struct Detail {
    file: String,
    title: String,
    author: String,
    #[serde(flatten)]
    content: Content,
    progress: Progress,
}

pub async fn detail(State(st): State<BooksState>, Path(file): Path<String>) -> Response {
    let shared = st.shared.clone();
    let res = blocking(move || {
        let dir = shared.dir();
        let (title, author, content) = shared.lib().content(&dir, &file)?;
        let progress = shared.lib().progress(&dir, &file);
        Some(Detail {
            file,
            title,
            author,
            content,
            progress,
        })
    })
    .await
    .flatten();
    match res {
        Some(d) => ([(header::CACHE_CONTROL, "no-store")], Json(d)).into_response(),
        None => not_found(),
    }
}

pub async fn save_progress(
    State(st): State<BooksState>,
    Path(file): Path<String>,
    Json(mut p): Json<Progress>,
) -> Response {
    if !library::is_safe_name(&file) || library::kind_of(&file).is_none() {
        return not_found();
    }
    p.updated = chrono::Utc::now().timestamp().max(0) as u64;
    p.percent = p.percent.clamp(0.0, 1.0);
    p.pos = p.pos.clamp(0.0, 1.0);
    let shared = st.shared.clone();
    match blocking(move || {
        let dir = shared.dir();
        if !dir.join(&file).is_file() {
            return Err("沒有這本書".to_string());
        }
        shared.lib().save_progress(&dir, &file, p)
    })
    .await
    {
        Some(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Some(Err(e)) => (StatusCode::BAD_REQUEST, e).into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn mime_of(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("xhtml" | "xht") => "application/xhtml+xml; charset=utf-8",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("xml" | "opf" | "ncx") => "application/xml",
        _ => "application/octet-stream",
    }
}

/// 從 zip 讀一個檔案
fn zip_entry(path: &PathBuf, name: &str) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
    let mut e = zip.by_name(name).ok()?;
    let mut buf = Vec::with_capacity(e.size() as usize);
    e.read_to_end(&mut buf).ok()?;
    Some(buf)
}

#[derive(Deserialize)]
pub struct ResQuery {
    /// as=html: XHTML 格式有錯時, 改用比較寬鬆的 HTML 解析
    #[serde(rename = "as")]
    as_: Option<String>,
}

pub async fn resource(
    State(st): State<BooksState>,
    Path((file, path)): Path<(String, String)>,
    Query(q): Query<ResQuery>,
) -> Response {
    if !library::is_safe_name(&file)
        || !matches!(library::kind_of(&file), Some(Kind::Epub | Kind::Mobi))
    {
        return not_found();
    }
    let epub = Library::epub_path(&st.shared.dir(), &file);
    let name = path.clone();
    let Some(data) = blocking(move || zip_entry(&epub, &name)).await.flatten() else {
        return not_found();
    };
    let mime =
        if q.as_.as_deref() == Some("html") && mime_of(&path).starts_with("application/xhtml") {
            "text/html; charset=utf-8"
        } else {
            mime_of(&path)
        };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_SECURITY_POLICY, CSP),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        data,
    )
        .into_response()
}

pub async fn txt_chapter(
    State(st): State<BooksState>,
    Path((file, n)): Path<(String, usize)>,
) -> Response {
    let shared = st.shared.clone();
    match blocking(move || shared.lib().txt_chapter_html(&shared.dir(), &file, n))
        .await
        .flatten()
    {
        Some(html) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CONTENT_SECURITY_POLICY, CSP),
            ],
            html,
        )
            .into_response(),
        None => not_found(),
    }
}

pub async fn comic_page(
    State(st): State<BooksState>,
    Path((file, n)): Path<(String, usize)>,
) -> Response {
    let shared = st.shared.clone();
    let res = blocking(move || {
        let dir = shared.dir();
        let name = shared.lib().comic_page(&dir, &file, n)?;
        let data = zip_entry(&dir.join(&file), &name)?;
        Some((mime_of(&name), data))
    })
    .await
    .flatten();
    match res {
        Some((mime, data)) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "private, max-age=86400"),
            ],
            data,
        )
            .into_response(),
        None => not_found(),
    }
}

pub async fn raw_file(
    State(st): State<BooksState>,
    Path(file): Path<String>,
    req: Request,
) -> Response {
    if !library::is_safe_name(&file) || library::kind_of(&file).is_none() {
        return not_found();
    }
    match ServeFile::new(st.shared.dir().join(&file))
        .oneshot(req)
        .await
    {
        Ok(res) => res.map(Body::new).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn cover(State(st): State<BooksState>, Path(file): Path<String>) -> Response {
    let shared = st.shared.clone();
    let res = blocking(move || {
        let dir = shared.dir();
        let name = shared.lib().cover(&dir, &file)?;
        let zip_path = match library::kind_of(&file)? {
            Kind::Comic => dir.join(&file),
            _ => Library::epub_path(&dir, &file),
        };
        let data = zip_entry(&zip_path, &name)?;
        Some((mime_of(&name), data))
    })
    .await
    .flatten();
    match res {
        Some((mime, data)) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "private, max-age=86400"),
            ],
            data,
        )
            .into_response(),
        None => not_found(),
    }
}

#[derive(Serialize)]
struct Uploaded {
    name: String,
}

pub async fn upload(
    State(st): State<BooksState>,
    Extension(Peer(peer)): Extension<Peer>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    if !library::is_safe_name(&name) || library::kind_of(&name).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "只接受 epub / mobi / azw / azw3 / txt / pdf / cbz / zip",
        )
            .into_response();
    }
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "檔案是空的").into_response();
    }
    let dir = st.shared.dir();
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("無法建立資料夾: {e}"),
        )
            .into_response();
    }
    // 同名時加上 -1、-2…
    let mut final_name = name.clone();
    if dir.join(&name).exists() {
        let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
        final_name = (1..)
            .map(|i| format!("{stem}-{i}.{ext}"))
            .find(|n| !dir.join(n).exists())
            .unwrap();
    }
    let tmp = dir.join(format!(".{final_name}.uploading"));
    let result = async {
        tokio::fs::write(&tmp, &body).await?;
        tokio::fs::rename(&tmp, dir.join(&final_name)).await
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("無法寫入: {e}")).into_response();
    }
    let _ = st.log.send(format!(
        "web {}: 上傳電子書 {final_name} ({} KB)",
        peer.ip(),
        body.len() / 1024
    ));
    Json(Uploaded { name: final_name }).into_response()
}

// ---------------------------------------------------------------- 閱讀用字型
//
// 手機大多沒有中文明體、楷體, 所以由 Pi 向 Google Fonts 下載(依用到的字分片下載),
// 存在 books/.cache/fonts/, 之後都從 Pi 拿: 手機不直接連 Google, 下載過的離線也能用。

const FONT_CSS_HOST: &str = "https://fonts.googleapis.com/css2";
const FONT_FILE_HOST: &str = "https://fonts.gstatic.com/";
/// 用新版瀏覽器的 User-Agent, Google Fonts 才會給 woff2 與 unicode-range 分片
const FONT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";

fn font_family(name: &str) -> Option<&'static str> {
    match name {
        "serif" => Some("Noto Serif TC:wght@400;700"),
        "kai" => Some("LXGW WenKai TC:wght@400;700"),
        _ => None,
    }
}

fn font_dir(shared: &Shared) -> PathBuf {
    shared.dir().join(".cache").join("fonts")
}

fn font_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent(FONT_UA)
        .build()
        .new_agent()
}

/// 寫入快取: 先寫暫存檔再改名, 同時有兩個請求也不會讀到寫一半的檔案
fn save_cache(path: &std::path::Path, data: &[u8]) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("f");
    let tmp = parent.join(format!(".{name}.{n}.tmp"));
    if std::fs::write(&tmp, data).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn fetch_font_css(family: &str) -> Result<String, String> {
    let css = font_agent()
        .get(FONT_CSS_HOST)
        .query("family", family)
        .query("display", "swap")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if !css.contains("@font-face") {
        return Err("不是字型 CSS".into());
    }
    // 分片改從 Pi 拿
    Ok(css.replace(FONT_FILE_HOST, "/books/fonts/f/"))
}

pub async fn font_css(State(st): State<BooksState>, Path(name): Path<String>) -> Response {
    let Some(family) = name.strip_suffix(".css").and_then(font_family) else {
        return not_found();
    };
    let path = font_dir(&st.shared).join(&name);
    let log = st.log.clone();
    let res = blocking(move || {
        if let Ok(css) = std::fs::read_to_string(&path) {
            return Some(css);
        }
        match fetch_font_css(family) {
            Ok(css) => {
                save_cache(&path, css.as_bytes());
                Some(css)
            }
            Err(e) => {
                let _ = log.send(format!("books: 無法下載字型 {family}: {e}"));
                None
            }
        }
    })
    .await
    .flatten();
    match res {
        Some(css) => (
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            css,
        )
            .into_response(),
        // 下載不到(沒網路): 給空的 CSS, 閱讀器退回裝置上的字型
        None => (
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            "",
        )
            .into_response(),
    }
}

/// 分片路徑只接受 `s/<字型>/<版本>/<檔名>.woff2` 這種格式
fn safe_font_path(path: &str) -> bool {
    path.starts_with("s/")
        && path.ends_with(".woff2")
        && path.len() < 300
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}

pub async fn font_file(State(st): State<BooksState>, Path(path): Path<String>) -> Response {
    if !safe_font_path(&path) {
        return not_found();
    }
    let file = font_dir(&st.shared).join("f").join(&path);
    let res = blocking(move || {
        if let Ok(data) = std::fs::read(&file) {
            return Some(data);
        }
        let mut res = font_agent()
            .get(format!("{FONT_FILE_HOST}{path}"))
            .call()
            .ok()?;
        let data = res
            .body_mut()
            .with_config()
            .limit(20 * 1024 * 1024)
            .read_to_vec()
            .ok()?;
        save_cache(&file, &data);
        Some(data)
    })
    .await
    .flatten();
    match res {
        Some(data) => (
            [
                (header::CONTENT_TYPE, "font/woff2"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            data,
        )
            .into_response(),
        None => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_paths() {
        assert!(safe_font_path("s/notoseriftc/v37/XLYgIZb5bJ-_x.12.woff2"));
        assert!(!safe_font_path("s/../../etc/passwd.woff2"));
        assert!(!safe_font_path("s/a/b.css"));
        assert!(!safe_font_path("x/a/b.woff2"));
        assert!(!safe_font_path("s/a b.woff2"));
        assert_eq!(font_family("serif"), Some("Noto Serif TC:wght@400;700"));
        assert_eq!(font_family("../x"), None);
    }
}
