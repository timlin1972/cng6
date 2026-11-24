//! Web 伺服器本體: 在獨立的 thread 跑 tokio runtime。
//!
//! https 模式下同一個 port 同時接受 https 與 http: 每個連線先偷看第一個 byte,
//! TLS 握手(0x16)就走 https, 否則當成 http 並轉址到 `https://` 同一個位址。

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post, put};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::crypto::ring;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

use super::books_api::{self, BooksState};
use super::cli_api;
use super::cluster_api;
use super::files_api::{self, FilesState};
use super::mail_api;
use super::music_api;
use super::page::{detect_device, render_page};
use super::sysmon::{self, SysMon};
use super::wallpaper_api::{self, WpState};
use super::weather_api;
use super::worldclock_api;
use crate::plugins::{books, cluster, files, mail, music, update, wallpaper, weather, worldclock};
use crate::remote;

/// 啟動伺服器需要的設定
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub port: u16,
    pub https: bool,
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// 執行中的伺服器, `stop()` 或 drop 時關閉
pub struct Handle {
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 連線來源(每個連線放進 request extension)
#[derive(Clone)]
pub struct Peer(pub PeerAddr);

#[derive(Clone, Debug)]
pub enum PeerAddr {
    /// 一般的網路連線
    Tcp(SocketAddr),
    /// 透過 cluster 從另一台機器轉過來的請求(機器名稱)
    Cluster(String),
}

impl PeerAddr {
    /// 記錄用的來源: IP 或 `cluster:<名稱>`
    pub fn ip(&self) -> String {
        match self {
            PeerAddr::Tcp(a) => a.ip().to_string(),
            PeerAddr::Cluster(name) => format!("cluster:{name}"),
        }
    }
}

#[derive(Clone)]
struct AppState {
    log: Sender<String>,
    port: u16,
    /// 網頁 CLI 的請求交給主迴圈
    remote: remote::Sender,
    services: Services,
}

/// 其他 plugin 和網頁伺服器共用的狀態; 新的 plugin 需要網頁 API 時加在這裡
#[derive(Clone)]
pub struct Services {
    pub wallpaper: wallpaper::Shared,
    pub music: music::Shared,
    pub weather: weather::Shared,
    pub worldclock: worldclock::Shared,
    pub books: books::Shared,
    pub files: files::Shared,
    pub cluster: cluster::Shared,
    pub update: update::Shared,
    pub mail: mail::Shared,
}

/// 啟動伺服器。成功時回傳 handle 與要顯示的提示訊息。
///
/// 憑證與 port 在這裡同步檢查, 錯誤可以直接回報給 CLI。
pub fn start(
    cfg: &Config,
    log: Sender<String>,
    remote: remote::Sender,
    services: Services,
) -> Result<(Handle, Vec<String>), String> {
    let mut notes = Vec::new();
    let tls = if cfg.https {
        if !cfg.cert.exists() && !cfg.key.exists() {
            generate_cert(&cfg.cert, &cfg.key)?;
            notes.push(format!(
                "已產生自簽憑證 {} / {} (瀏覽器第一次連線會顯示不安全警告)",
                cfg.cert.display(),
                cfg.key.display()
            ));
        }
        Some(TlsAcceptor::from(load_tls(&cfg.cert, &cfg.key)?))
    } else {
        None
    };

    let listener = std::net::TcpListener::bind(("0.0.0.0", cfg.port))
        .map_err(|e| format!("無法使用 port {}: {e}", cfg.port))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("無法設定 socket: {e}"))?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("cng6-web")
        .enable_all()
        .build()
        .map_err(|e| format!("無法建立 runtime: {e}"))?;

    let state = AppState {
        log,
        port: cfg.port,
        remote,
        services,
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let thread = std::thread::spawn(move || {
        rt.block_on(async move {
            let Ok(listener) = TcpListener::from_std(listener) else {
                return;
            };
            tokio::select! {
                _ = accept_loop(listener, tls, state) => {}
                _ = shutdown_rx => {}
            }
        });
        // 結束還在處理中的連線
        rt.shutdown_timeout(Duration::from_secs(1));
    });

    Ok((
        Handle {
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        },
        notes,
    ))
}

/// 各 plugin 的網頁 API(不含首頁與系統監控)。
///
/// 也給 cluster 用: 其他機器透過 MQTT 轉過來的請求, 直接在本機交給這個 Router 處理。
pub fn local_api(log: Sender<String>, remote: remote::Sender, services: Services) -> Router {
    let state = AppState {
        log,
        port: 0,
        remote,
        services,
    };
    let wp = Router::new()
        .route("/api/wallpaper", get(wallpaper_api::info))
        .route(
            "/api/wallpaper/{name}",
            put(wallpaper_api::upload).delete(wallpaper_api::delete),
        )
        .route("/wallpapers/{name}", get(wallpaper_api::image))
        .layer(DefaultBodyLimit::max(wallpaper_api::MAX_UPLOAD))
        .with_state(WpState {
            shared: state.services.wallpaper.clone(),
            log: state.log.clone(),
        });
    let books = Router::new()
        .route("/api/books", get(books_api::list))
        .route("/api/books/{file}", get(books_api::detail))
        .route("/api/books/{file}/progress", put(books_api::save_progress))
        .route("/api/books/upload/{name}", put(books_api::upload))
        .route("/api/books/haodoo", get(books_api::haodoo_index))
        .route("/api/books/haodoo/download", post(books_api::haodoo_download))
        .route("/api/books/haodoo/{m}/{id}", get(books_api::haodoo_volumes))
        .route("/books/res/{file}/{*path}", get(books_api::resource))
        .route("/books/txt/{file}/{n}", get(books_api::txt_chapter))
        .route("/books/page/{file}/{n}", get(books_api::comic_page))
        .route("/books/file/{file}", get(books_api::raw_file))
        .route("/books/cover/{file}", get(books_api::cover))
        .route("/books/fonts/f/{*path}", get(books_api::font_file))
        .route("/books/fonts/{name}", get(books_api::font_css))
        .route("/pdfjs/{*path}", get(super::pdfjs::file))
        .layer(DefaultBodyLimit::max(books_api::MAX_UPLOAD))
        .with_state(BooksState {
            shared: state.services.books.clone(),
            log: state.log.clone(),
        });
    let files = Router::new()
        .route("/api/files/roots", get(files_api::roots))
        .route("/api/files/list", get(files_api::list))
        .route("/api/files/usage", get(files_api::usage))
        .route(
            "/api/files/text",
            get(files_api::read_text).put(files_api::save_text),
        )
        .route("/api/files/op", post(files_api::op))
        .route("/api/files/trash", get(files_api::trash_list))
        .route("/files/raw", get(files_api::raw))
        .route("/files/zip", get(files_api::zip))
        // 上傳串流寫入檔案, 不限大小
        .route(
            "/api/files/upload",
            put(files_api::upload).layer(DefaultBodyLimit::disable()),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(FilesState {
            shared: state.services.files.clone(),
            log: state.log.clone(),
        });
    let mail = Router::new()
        .route("/api/mail", get(mail_api::overview))
        .route("/api/mail/notices", get(mail_api::notices))
        .route("/api/mail/avatar/{domain}", get(mail_api::avatar))
        .route("/api/mail/accounts", post(mail_api::add_account))
        .route("/api/mail/accounts/{name}", axum::routing::delete(mail_api::remove_account))
        .route("/api/mail/a/{acct}/labels", get(mail_api::labels))
        .route("/api/mail/a/{acct}/list", get(mail_api::list))
        .route("/api/mail/a/{acct}/op", post(mail_api::op))
        .route("/api/mail/a/{acct}/msg/{id}", get(mail_api::message))
        .route("/api/mail/a/{acct}/msg/{id}/body", get(mail_api::body))
        .route("/api/mail/a/{acct}/msg/{id}/part/{n}", get(mail_api::part))
        .with_state(state.services.mail.clone());
    let update = Router::new()
        .route("/api/update", get(update_info))
        .with_state(state.services.update.clone());
    let worldclock = Router::new()
        .route("/api/worldclock", get(worldclock_api::info))
        .with_state(state.services.worldclock.clone());
    let weather = Router::new()
        .route("/api/weather", get(weather_api::info))
        .with_state(state.services.weather.clone());
    let music = Router::new()
        .route("/api/music", get(music_api::info))
        .route("/music/file/{name}", get(music_api::file))
        .route("/music/cover/{name}", get(music_api::cover))
        .route("/music/lyrics/{name}", get(music_api::lyrics))
        .with_state(state.services.music.clone());
    let cli = Router::new()
        .route("/api/cli", post(cli_api::cli))
        .with_state(state.remote.clone());
    // 把本機的檔案傳到另一台; 放在這裡, 別台看這台的檔案時也能叫這台傳送
    let cluster_send = Router::new()
        .route("/api/cluster/send", post(cluster_api::send))
        .with_state(cluster_api::ClusterState {
            shared: state.services.cluster.clone(),
            files: state.services.files.clone(),
        });
    Router::new()
        .merge(cli)
        .merge(cluster_send)
        .merge(wp)
        .merge(music)
        .merge(weather)
        .merge(worldclock)
        .merge(update)
        .merge(mail)
        .merge(books)
        .merge(files)
}

async fn accept_loop(listener: TcpListener, tls: Option<TlsAcceptor>, state: AppState) {
    let api = Router::new()
        .route("/api/system", get(sysmon::snapshot))
        .route("/api/system/stream", get(sysmon::stream))
        .with_state(SysMon::spawn());
    let cluster = Router::new()
        .route("/api/cluster", get(cluster_api::info))
        .route("/api/cluster/clip", post(cluster_api::clip))
        // 透過 MQTT 轉給另一台機器(檔案上傳不限大小, 由 cluster 限制)
        .route(
            "/api/cluster/node/{name}/{*rest}",
            any(cluster_api::proxy).layer(DefaultBodyLimit::disable()),
        )
        .with_state(cluster_api::ClusterState {
            shared: state.services.cluster.clone(),
            files: state.services.files.clone(),
        });
    let app = Router::new()
        .route("/", get(index))
        .route("/favicon.svg", get(favicon))
        .route("/favicon.ico", get(favicon))
        .route("/apple-touch-icon.png", get(touch_icon))
        .fallback(not_found)
        .with_state(state.clone())
        .merge(api)
        .merge(local_api(
            state.log.clone(),
            state.remote.clone(),
            state.services.clone(),
        ))
        .merge(cluster);
    let redirect = Router::new().fallback(redirect).with_state(state);
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        let app = app.clone().layer(Extension(Peer(PeerAddr::Tcp(peer))));
        let redirect = redirect.clone().layer(Extension(Peer(PeerAddr::Tcp(peer))));
        let tls = tls.clone();
        tokio::spawn(async move {
            match tls {
                None => serve(stream, app).await,
                Some(acceptor) => {
                    if is_tls(&stream).await {
                        if let Ok(s) = acceptor.accept(stream).await {
                            serve(s, app).await;
                        }
                    } else {
                        serve(stream, redirect).await;
                    }
                }
            }
        });
    }
}

/// 偷看第一個 byte 判斷是不是 TLS 握手(不會消耗資料)
async fn is_tls(stream: &TcpStream) -> bool {
    let mut b = [0u8; 1];
    matches!(
        tokio::time::timeout(Duration::from_secs(10), stream.peek(&mut b)).await,
        Ok(Ok(1))
    ) && b[0] == 0x16
}

async fn serve<S>(stream: S, app: Router)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let _ = auto::Builder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(app))
        .await;
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn index(
    State(st): State<AppState>,
    Extension(Peer(peer)): Extension<Peer>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let device = detect_device(
        uri.query(),
        header_str(&headers, "sec-ch-ua-mobile"),
        header_str(&headers, "user-agent").unwrap_or(""),
    );
    // HTTP/2 的 uri 含有 scheme 與 host, 只記錄路徑讓紀錄一致
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let _ = st
        .log
        .send(format!("web: {} {method} {path} → {device}", peer.ip()));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // 請瀏覽器之後的請求附上 Sec-CH-UA-Mobile, 並告訴快取頁面會依裝置不同
            (
                header::HeaderName::from_static("accept-ch"),
                "Sec-CH-UA-Mobile",
            ),
            (header::VARY, "User-Agent, Sec-CH-UA-Mobile"),
        ],
        render_page(device, &st.services.cluster.name()),
    )
        .into_response()
}

/// 自動更新的狀態, 加上前端程式的版本(網頁發現版本變了會重新載入)
async fn update_info(State(u): State<update::Shared>) -> Response {
    let mut v = serde_json::to_value(u.snapshot()).unwrap_or_default();
    v["assets"] = serde_json::Value::String(super::page::assets_version().to_string());
    axum::Json(v).into_response()
}

/// 網站圖示: CNG6 (Controller Next Generation Six) 的「6」
async fn favicon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("assets/icon.svg"),
    )
        .into_response()
}

/// iPhone / iPad 加到主畫面時的圖示(要 PNG)
async fn touch_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        &include_bytes!("assets/apple-touch-icon.png")[..],
    )
        .into_response()
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "404 Not Found").into_response()
}

/// http 連線: 轉址到 https 的同一個位址
async fn redirect(
    State(st): State<AppState>,
    Extension(Peer(peer)): Extension<Peer>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let host = header_str(&headers, "host")
        .map(str::to_string)
        .unwrap_or_else(|| format!("localhost:{}", st.port));
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let target = format!("https://{host}{path}");
    let _ = st.log.send(format!(
        "web: {} {method} {path} (http) → {target}",
        peer.ip()
    ));
    // 用暫時轉址: 之後關掉 https 時, 瀏覽器不會記住舊的轉址
    Redirect::temporary(&target).into_response()
}

fn load_tls(cert: &Path, key: &Path) -> Result<Arc<ServerConfig>, String> {
    let certs = CertificateDer::pem_file_iter(cert)
        .and_then(|it| it.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("無法讀取憑證 {}: {e}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("無法讀取私鑰 {}: {e}", key.display()))?;
    let mut config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS 設定錯誤: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("憑證與私鑰不符或格式錯誤: {e}"))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// 產生自簽憑證, 涵蓋 localhost、主機名稱與本機所有 IP
fn generate_cert(cert: &Path, key: &Path) -> Result<(), String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(host) = sysinfo::System::host_name() {
        names.push(host.clone());
        names.push(format!("{host}.local"));
    }
    names.extend(local_ips().iter().map(IpAddr::to_string));
    names.dedup();
    let ck = rcgen::generate_simple_self_signed(names).map_err(|e| format!("無法產生憑證: {e}"))?;
    std::fs::write(cert, ck.cert.pem()).map_err(|e| format!("無法寫入 {}: {e}", cert.display()))?;
    std::fs::write(key, ck.signing_key.serialize_pem())
        .map_err(|e| format!("無法寫入 {}: {e}", key.display()))?;
    // 私鑰只給自己讀
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 本機的 IPv4 位址(不含 loopback)
pub fn local_ips() -> Vec<IpAddr> {
    let nets = sysinfo::Networks::new_with_refreshed_list();
    let mut ips: Vec<IpAddr> = nets
        .values()
        .flat_map(|n| n.ip_networks().iter().map(|ip| ip.addr))
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback())
        .collect();
    ips.sort();
    ips.dedup();
    ips
}
