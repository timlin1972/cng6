//! 檔案管理員的網頁 API。路徑一律是 `root`(第幾個根目錄)+ `path`(相對路徑)。
//!
//! - `GET  /api/files/roots`              → 根目錄
//! - `GET  /api/files/list?root&path`     → 資料夾內容
//! - `GET  /api/files/usage?root&path`    → 資料夾的檔案數與大小
//! - `GET  /api/files/text?root&path`     → 讀文字檔(編輯用)
//! - `PUT  /api/files/text?root&path`     → 存文字檔(檔案在別處改過時回 409)
//! - `PUT  /api/files/upload?root&path&name` → 上傳(串流寫入, 不限大小)
//! - `POST /api/files/op`                 → 新增、改名、刪除、複製、搬移、解壓縮、回收桶
//! - `GET  /api/files/trash`              → 回收桶
//! - `GET  /files/raw?root&path[&dl=1]`   → 檔案內容(預覽 / 下載, 支援 Range)
//! - `GET  /files/zip?root&path&names`    → 壓縮下載(邊壓邊送)
//!
//! 會修改檔案的操作都記錄到 Message 面板。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::server::Peer;
use crate::plugins::books::txt;
use crate::plugins::files::{Shared, fsops, trash};

#[derive(Clone)]
pub struct FilesState {
    pub shared: Shared,
    pub log: Sender<String>,
}

/// 可以在網頁上編輯的文字檔大小上限
const MAX_TEXT: u64 = 2 * 1024 * 1024;

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, msg.into()).into_response()
}

fn bad(msg: impl Into<String>) -> Response {
    err(StatusCode::BAD_REQUEST, msg)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Box<Response>> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Box::new(err(StatusCode::INTERNAL_SERVER_ERROR, "內部錯誤")))
}

#[derive(Deserialize)]
pub struct Loc {
    #[serde(default)]
    root: usize,
    #[serde(default)]
    path: String,
}

impl FilesState {
    /// 根目錄 + 相對路徑 → 實際路徑(確認在根目錄裡)
    fn resolve(&self, root: usize, path: &str) -> Result<PathBuf, Box<Response>> {
        let r = self.shared.root(root).ok_or_else(|| Box::new(bad("沒有這個根目錄")))?;
        fsops::resolve(&r.path, path).map_err(|e| Box::new(err(StatusCode::NOT_FOUND, e)))
    }

    fn log(&self, peer: &Peer, msg: String) {
        let _ = self.log.send(format!("web {}: {msg}", peer.0.ip()));
    }
}

fn mtime(p: &Path) -> i64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).map(|m| m.mtime()).unwrap_or(0)
}

// ---------------------------------------------------------------- 瀏覽

#[derive(Serialize)]
struct RootInfo {
    name: String,
    path: String,
    ok: bool,
}

pub async fn roots(State(st): State<FilesState>) -> Response {
    let roots: Vec<_> = st
        .shared
        .roots()
        .into_iter()
        .map(|r| RootInfo {
            ok: r.path.is_dir(),
            name: r.name,
            path: r.path.display().to_string(),
        })
        .collect();
    Json(roots).into_response()
}

#[derive(Serialize)]
struct Listing {
    entries: Vec<fsops::Entry>,
}

pub async fn list(State(st): State<FilesState>, Query(q): Query<Loc>) -> Response {
    let dir = match st.resolve(q.root, &q.path) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match blocking(move || fsops::list(&dir)).await {
        Ok(Ok(entries)) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(Listing { entries }),
        )
            .into_response(),
        Ok(Err(e)) => bad(e),
        Err(r) => *r,
    }
}

#[derive(Serialize)]
struct Usage {
    files: u64,
    bytes: u64,
    /// 項目太多, 只算了一部分
    partial: bool,
}

pub async fn usage(State(st): State<FilesState>, Query(q): Query<Loc>) -> Response {
    let dir = match st.resolve(q.root, &q.path) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match blocking(move || fsops::dir_usage(&dir, 200_000)).await {
        Ok((files, bytes, partial)) => Json(Usage {
            files,
            bytes,
            partial,
        })
        .into_response(),
        Err(r) => *r,
    }
}

// ---------------------------------------------------------------- 檔案內容

#[derive(Deserialize)]
pub struct RawQuery {
    #[serde(default)]
    root: usize,
    #[serde(default)]
    path: String,
    /// 1 = 下載(不預覽)
    #[serde(default)]
    dl: Option<String>,
}

/// Content-Disposition 的檔名(RFC 5987)
fn disposition(kind: &str, name: &str) -> HeaderValue {
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' })
        .collect();
    let mut enc = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    HeaderValue::from_str(&format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{enc}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

pub async fn raw(State(st): State<FilesState>, Query(q): Query<RawQuery>, req: Request) -> Response {
    let path = match st.resolve(q.root, &q.path) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !path.is_file() {
        return err(StatusCode::NOT_FOUND, "不是檔案");
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let download = q.dl.is_some();
    let mut res = match ServeFile::new(&path).oneshot(req).await {
        Ok(r) => r.map(Body::new).into_response(),
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "無法讀取"),
    };
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if download {
        h.insert(header::CONTENT_DISPOSITION, disposition("attachment", &name));
    } else {
        // 使用者的 HTML / SVG 不能在這個網站的權限下執行 script
        if matches!(ext.as_str(), "html" | "htm" | "xhtml" | "xht" | "svg" | "xml" | "js" | "mjs") {
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
        }
        // PDF 要給瀏覽器內建的閱讀器顯示, 其他一律放進 sandbox
        if ext != "pdf" {
            h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
        }
        h.insert(header::CONTENT_DISPOSITION, disposition("inline", &name));
    }
    res
}

#[derive(Serialize)]
struct TextFile {
    text: String,
    encoding: &'static str,
    mtime: i64,
}

pub async fn read_text(State(st): State<FilesState>, Query(q): Query<Loc>) -> Response {
    let path = match st.resolve(q.root, &q.path) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let res = blocking(move || -> Result<TextFile, String> {
        let md = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if !md.is_file() {
            return Err("不是檔案".into());
        }
        if md.len() > MAX_TEXT {
            return Err(format!("檔案太大(超過 {} MB), 不能在網頁上編輯", MAX_TEXT / 1024 / 1024));
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return Err("這不是文字檔".into());
        }
        let (text, encoding) = txt::decode(&bytes);
        Ok(TextFile {
            text,
            encoding,
            mtime: mtime(&path),
        })
    })
    .await;
    match res {
        Ok(Ok(t)) => ([(header::CACHE_CONTROL, "no-store")], Json(t)).into_response(),
        Ok(Err(e)) => bad(e),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
pub struct SaveText {
    text: String,
    /// 讀取時的修改時間; 不一樣代表檔案在別處被改過
    mtime: Option<i64>,
    /// 不管有沒有被改過都存
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
struct Saved {
    mtime: i64,
}

pub async fn save_text(
    State(st): State<FilesState>,
    Extension(peer): Extension<Peer>,
    Query(q): Query<Loc>,
    Json(body): Json<SaveText>,
) -> Response {
    let path = match st.resolve(q.root, &q.path) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let shown = q.path.clone();
    let res = blocking(move || -> Result<i64, (StatusCode, String)> {
        let md = std::fs::metadata(&path).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        if !md.is_file() {
            return Err((StatusCode::BAD_REQUEST, "不是檔案".into()));
        }
        if !body.force && body.mtime.is_some_and(|m| m != mtime(&path)) {
            return Err((StatusCode::CONFLICT, "檔案在別的地方被修改過".into()));
        }
        // 先寫暫存檔再改名, 存到一半斷掉也不會毀掉原檔; 保留原本的權限
        let dir = path.parent().ok_or((StatusCode::BAD_REQUEST, "路徑不合法".to_string()))?;
        let name = path.file_name().unwrap().to_string_lossy();
        let tmp = dir.join(format!(".{name}.cng6-saving"));
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(body.text.as_bytes())?;
            f.sync_all()?;
            std::fs::set_permissions(&tmp, md.permissions())?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_file(&tmp);
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("無法存檔: {e}")));
        }
        Ok(mtime(&path))
    })
    .await;
    match res {
        Ok(Ok(m)) => {
            st.log(&peer, format!("檔案 存檔 {shown}"));
            Json(Saved { mtime: m }).into_response()
        }
        Ok(Err((code, e))) => err(code, e),
        Err(r) => *r,
    }
}

// ---------------------------------------------------------------- 上傳

#[derive(Deserialize)]
pub struct UploadQuery {
    #[serde(default)]
    root: usize,
    #[serde(default)]
    path: String,
    name: String,
}

#[derive(Serialize)]
struct Uploaded {
    name: String,
}

pub async fn upload(
    State(st): State<FilesState>,
    Extension(peer): Extension<Peer>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Response {
    let dir = match st.resolve(q.root, &q.path) {
        Ok(d) if d.is_dir() => d,
        Ok(_) => return bad("不是資料夾"),
        Err(r) => return *r,
    };
    if !fsops::valid_name(&q.name) {
        return bad(format!("檔名 '{}' 不合法", q.name));
    }
    let tmp = dir.join(format!(".{}.cng6-uploading", q.name));
    let mut file = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => return bad(format!("無法寫入: {e}")),
    };
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let res = match chunk {
            Ok(b) => {
                total += b.len() as u64;
                file.write_all(&b).await.map_err(|e| e.to_string())
            }
            Err(e) => Err(format!("上傳中斷: {e}")),
        };
        if let Err(e) = res {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
            return bad(e);
        }
    }
    if let Err(e) = file.flush().await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return bad(e.to_string());
    }
    drop(file);
    // 同名時自動改名
    let name = fsops::unique_name(&dir, &q.name);
    if let Err(e) = tokio::fs::rename(&tmp, dir.join(&name)).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return bad(e.to_string());
    }
    st.log(
        &peer,
        format!(
            "檔案 上傳 {}/{name} ({} KB)",
            q.path.trim_end_matches('/'),
            total / 1024
        ),
    );
    Json(Uploaded { name }).into_response()
}

// ---------------------------------------------------------------- 操作

#[derive(Deserialize)]
pub struct TrashRef {
    trash: usize,
    id: String,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Mkdir {
        root: usize,
        path: String,
        name: String,
    },
    Newfile {
        root: usize,
        path: String,
        name: String,
    },
    Rename {
        root: usize,
        path: String,
        name: String,
    },
    Trash {
        root: usize,
        paths: Vec<String>,
    },
    Copy {
        root: usize,
        paths: Vec<String>,
        to_root: usize,
        to: String,
    },
    Move {
        root: usize,
        paths: Vec<String>,
        to_root: usize,
        to: String,
    },
    Unzip {
        root: usize,
        path: String,
    },
    Restore {
        items: Vec<TrashRef>,
    },
    Purge {
        items: Vec<TrashRef>,
    },
    Empty,
}

#[derive(Serialize, Default)]
struct OpResult {
    /// 給使用者看的結果
    message: String,
    /// 新建立的名稱(新增、改名、解壓縮)
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// 部分失敗的項目
    errors: Vec<String>,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 執行操作; 回傳 (結果, 要記錄的訊息)
fn run_op(st: &FilesState, op: Op) -> Result<(OpResult, String), String> {
    let resolve = |root: usize, path: &str| -> Result<PathBuf, String> {
        let r = st.shared.root(root).ok_or("沒有這個根目錄")?;
        fsops::resolve(&r.path, path)
    };
    let dir_of = |root: usize, path: &str| -> Result<PathBuf, String> {
        let d = resolve(root, path)?;
        if d.is_dir() { Ok(d) } else { Err("不是資料夾".into()) }
    };
    let mut res = OpResult::default();
    let log = match op {
        Op::Mkdir { root, path, name } => {
            let p = fsops::child(&dir_of(root, &path)?, &name)?;
            if std::fs::symlink_metadata(&p).is_ok() {
                return Err(format!("'{name}' 已經存在"));
            }
            std::fs::create_dir(&p).map_err(|e| format!("無法建立資料夾: {e}"))?;
            res.message = format!("已建立資料夾 {name}");
            res.name = Some(name.clone());
            format!("檔案 新增資料夾 {}/{name}", path.trim_end_matches('/'))
        }
        Op::Newfile { root, path, name } => {
            let p = fsops::child(&dir_of(root, &path)?, &name)?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => format!("'{name}' 已經存在"),
                    _ => format!("無法建立檔案: {e}"),
                })?;
            res.message = format!("已建立 {name}");
            res.name = Some(name.clone());
            format!("檔案 新增檔案 {}/{name}", path.trim_end_matches('/'))
        }
        Op::Rename { root, path, name } => {
            let p = resolve(root, &path)?;
            if path.trim_matches('/').is_empty() {
                return Err("不能改根目錄的名稱".into());
            }
            let dst = fsops::child(p.parent().ok_or("路徑不合法")?, &name)?;
            if dst == p {
                return Ok((
                    OpResult {
                        message: "名稱沒有改變".into(),
                        ..Default::default()
                    },
                    String::new(),
                ));
            }
            if std::fs::symlink_metadata(&dst).is_ok() {
                return Err(format!("'{name}' 已經存在"));
            }
            std::fs::rename(&p, &dst).map_err(|e| format!("無法改名: {e}"))?;
            res.message = format!("已改名為 {name}");
            res.name = Some(name.clone());
            format!("檔案 改名 {path} → {name}")
        }
        Op::Trash { root, paths } => {
            let mut done = Vec::new();
            for path in &paths {
                if path.trim_matches('/').is_empty() {
                    res.errors.push("不能刪除根目錄".into());
                    continue;
                }
                match resolve(root, path).and_then(|p| trash::trash(&p).map(|_| p)) {
                    Ok(p) => done.push(file_name(&p)),
                    Err(e) => res.errors.push(format!("{path}: {e}")),
                }
            }
            res.message = format!("已移到回收桶 {} 項", done.len());
            format!("檔案 刪除(移到回收桶) {}", done.join(", "))
        }
        Op::Copy {
            root,
            paths,
            to_root,
            to,
        } => {
            let dst_dir = dir_of(to_root, &to)?;
            let mut done = Vec::new();
            for path in &paths {
                let r = resolve(root, path).and_then(|src| {
                    if src.is_dir() && fsops::is_inside(&src, &dst_dir) {
                        return Err("不能複製到自己裡面".into());
                    }
                    let name = fsops::unique_name(&dst_dir, &file_name(&src));
                    fsops::copy_all(&src, &dst_dir.join(&name)).map_err(|e| e.to_string())?;
                    Ok(name)
                });
                match r {
                    Ok(n) => done.push(n),
                    Err(e) => res.errors.push(format!("{path}: {e}")),
                }
            }
            res.message = format!("已複製 {} 項", done.len());
            format!("檔案 複製 {} → {}", done.join(", "), display(st, to_root, &to))
        }
        Op::Move {
            root,
            paths,
            to_root,
            to,
        } => {
            let dst_dir = dir_of(to_root, &to)?;
            let mut done = Vec::new();
            for path in &paths {
                let r = resolve(root, path).and_then(|src| {
                    if path.trim_matches('/').is_empty() {
                        return Err("不能搬移根目錄".into());
                    }
                    if src.parent() == Some(dst_dir.as_path()) {
                        return Ok(None); // 已經在這裡
                    }
                    if src.is_dir() && fsops::is_inside(&src, &dst_dir) {
                        return Err("不能搬到自己裡面".into());
                    }
                    let name = fsops::unique_name(&dst_dir, &file_name(&src));
                    fsops::move_path(&src, &dst_dir.join(&name)).map_err(|e| e.to_string())?;
                    Ok(Some(name))
                });
                match r {
                    Ok(Some(n)) => done.push(n),
                    Ok(None) => {}
                    Err(e) => res.errors.push(format!("{path}: {e}")),
                }
            }
            res.message = format!("已搬移 {} 項", done.len());
            format!("檔案 搬移 {} → {}", done.join(", "), display(st, to_root, &to))
        }
        Op::Unzip { root, path } => {
            let zip = resolve(root, &path)?;
            let dir = zip.parent().ok_or("路徑不合法")?;
            let fname = file_name(&zip);
            let stem = fname
                .rsplit_once('.')
                .map(|(s, _)| s)
                .filter(|s| !s.is_empty())
                .unwrap_or(&fname);
            let name = fsops::unique_name(dir, stem);
            let dest = dir.join(&name);
            match fsops::extract_zip(&zip, &dest) {
                Ok(n) => {
                    res.message = format!("已解壓縮 {n} 個檔案到 {name}");
                    res.name = Some(name);
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dest);
                    return Err(e);
                }
            }
            format!("檔案 解壓縮 {path}")
        }
        Op::Restore { items } => {
            let roots = st.shared.roots();
            let mut done = Vec::new();
            for it in &items {
                match trash::restore(&roots, it.trash, &it.id) {
                    Ok(p) => done.push(p.display().to_string()),
                    Err(e) => res.errors.push(format!("{}: {e}", it.id)),
                }
            }
            res.message = format!("已還原 {} 項", done.len());
            format!("檔案 從回收桶還原 {}", done.join(", "))
        }
        Op::Purge { items } => {
            let roots = st.shared.roots();
            let mut n = 0;
            for it in &items {
                match trash::purge(&roots, it.trash, &it.id) {
                    Ok(()) => n += 1,
                    Err(e) => res.errors.push(format!("{}: {e}", it.id)),
                }
            }
            res.message = format!("已永久刪除 {n} 項");
            format!("檔案 永久刪除 {n} 項(回收桶)")
        }
        Op::Empty => {
            let roots = st.shared.roots();
            let items = trash::list(&roots);
            let mut n = 0;
            for it in &items {
                match trash::purge(&roots, it.trash, &it.id) {
                    Ok(()) => n += 1,
                    Err(e) => res.errors.push(format!("{}: {e}", it.name)),
                }
            }
            res.message = format!("已清空回收桶({n} 項)");
            format!("檔案 清空回收桶({n} 項)")
        }
    };
    Ok((res, log))
}

/// 記錄用的位置: `根目錄名稱:/相對路徑`
fn display(st: &FilesState, root: usize, path: &str) -> String {
    let name = st.shared.root(root).map(|r| r.name).unwrap_or_default();
    format!("{name}:/{}", path.trim_matches('/'))
}

pub async fn op(
    State(st): State<FilesState>,
    Extension(peer): Extension<Peer>,
    Json(op): Json<Op>,
) -> Response {
    let st2 = st.clone();
    match blocking(move || run_op(&st2, op)).await {
        Ok(Ok((res, log))) => {
            if !log.is_empty() {
                st.log(&peer, log);
            }
            Json(res).into_response()
        }
        Ok(Err(e)) => bad(e),
        Err(r) => *r,
    }
}

pub async fn trash_list(State(st): State<FilesState>) -> Response {
    let roots = st.shared.roots();
    match blocking(move || trash::list(&roots)).await {
        Ok(items) => ([(header::CACHE_CONTROL, "no-store")], Json(items)).into_response(),
        Err(r) => *r,
    }
}

// ---------------------------------------------------------------- 壓縮下載

#[derive(Deserialize)]
pub struct ZipQuery {
    #[serde(default)]
    root: usize,
    /// 所在的資料夾
    #[serde(default)]
    path: String,
    /// 要壓縮的項目(JSON 陣列)
    names: String,
}

/// 把 zip 的輸出一塊一塊送進 channel(在 blocking thread 裡寫)
struct ChanWriter(tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>);

impl Write for ChanWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(buf)))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "下載已取消"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub async fn zip(
    State(st): State<FilesState>,
    Extension(peer): Extension<Peer>,
    Query(q): Query<ZipQuery>,
) -> Response {
    let dir = match st.resolve(q.root, &q.path) {
        Ok(d) if d.is_dir() => d,
        Ok(_) => return bad("不是資料夾"),
        Err(r) => return *r,
    };
    let Ok(names) = serde_json::from_str::<Vec<String>>(&q.names) else {
        return bad("names 格式錯誤");
    };
    if names.is_empty() {
        return bad("沒有選擇檔案");
    }
    let mut items = Vec::new();
    for n in &names {
        let rel = format!("{}/{n}", q.path.trim_end_matches('/'));
        match st.resolve(q.root, &rel) {
            Ok(p) if fsops::valid_name(n) => items.push(p),
            Ok(_) => return bad(format!("檔名 '{n}' 不合法")),
            Err(r) => return *r,
        }
    }
    let zip_name = if names.len() == 1 {
        format!("{}.zip", names[0])
    } else {
        // 在根目錄時用根目錄的名稱
        let base = if q.path.trim_matches('/').is_empty() {
            st.shared.root(q.root).map(|r| r.name).unwrap_or_default()
        } else {
            file_name(&dir)
        };
        format!("{}.zip", if base.is_empty() { "files" } else { &base })
    };
    st.log(&peer, format!("檔案 壓縮下載 {}", names.join(", ")));
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    tokio::task::spawn_blocking(move || {
        let w = std::io::BufWriter::with_capacity(256 * 1024, ChanWriter(tx.clone()));
        if let Err(e) = fsops::write_zip(&items, w) {
            let _ = tx.blocking_send(Err(std::io::Error::other(e.to_string())));
        }
    });
    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, disposition("attachment", &zip_name)),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition() {
        let v = disposition("attachment", "報告 1.pdf");
        assert_eq!(
            v.to_str().unwrap(),
            "attachment; filename=\"__ 1.pdf\"; filename*=UTF-8''%E5%A0%B1%E5%91%8A%201.pdf"
        );
    }
}
