//! 郵件的網頁 API。
//!
//! - GET    /api/mail                         帳號、未讀數、最新幾封
//! - GET    /api/mail/notices?after=N          新信通知
//! - POST   /api/mail/accounts                 加入帳號 {name, email, password}(先試登入)
//! - DELETE /api/mail/accounts/{name}
//! - GET    /api/mail/a/{acct}/labels
//! - GET    /api/mail/a/{acct}/list?q=&mailbox=&offset=
//! - GET    /api/mail/a/{acct}/msg/{id}        信件資訊(寄件者、附件…)
//! - GET    /api/mail/a/{acct}/msg/{id}/body   內文(HTML, 放在 iframe 裡; 不執行程式, 預設不載入外部圖片)
//! - GET    /api/mail/a/{acct}/msg/{id}/part/{n}  附件或內嵌圖片
//! - POST   /api/mail/a/{acct}/op              {ids, op}: read / unread / star / unstar / archive / inbox / trash
//! - POST   /api/mail/trust                    {addr, trust}: 這個寄件者的外部圖片一律直接載入
//! - GET    /api/mail/avatar/{domain}          寄件者公司的圖示(Pi 代抓並快取; 沒有時 404, 前端改用文字頭像)

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use mail_parser::MessageParser;
use serde::{Deserialize, Serialize};

use super::files_api::disposition;
use crate::plugins::mail::Shared;
use crate::plugins::mail::imap::{Job, Op, Out};

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// IMAP 失敗: 502(不是網頁伺服器的錯)
fn bad_gateway(e: String) -> Response {
    err(StatusCode::BAD_GATEWAY, e)
}

pub async fn overview(State(m): State<Shared>) -> Response {
    Json(m.overview()).into_response()
}

#[derive(Deserialize)]
pub struct AfterQuery {
    #[serde(default)]
    after: u64,
}

pub async fn notices(State(m): State<Shared>, Query(q): Query<AfterQuery>) -> Response {
    let (list, last) = m.notices_after(q.after);
    Json(serde_json::json!({ "notices": list, "last": last })).into_response()
}

#[derive(Deserialize)]
pub struct AddAccount {
    name: String,
    email: String,
    password: String,
}

pub async fn add_account(State(m): State<Shared>, Json(a): Json<AddAccount>) -> Response {
    match m.add_account(&a.name, &a.email, &a.password).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn remove_account(State(m): State<Shared>, Path(name): Path<String>) -> Response {
    match m.remove_account(&name) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

pub async fn labels(State(m): State<Shared>, Path(acct): Path<String>) -> Response {
    match m.request(&acct, Job::Labels).await {
        Ok(Out::Labels(l)) => Json(l).into_response(),
        Ok(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "回應錯誤"),
        Err(e) => bad_gateway(e),
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    mailbox: Option<String>,
    #[serde(default)]
    offset: usize,
}

pub async fn list(State(m): State<Shared>, Path(acct): Path<String>, Query(q): Query<ListQuery>) -> Response {
    let job = Job::List {
        mailbox: q.mailbox,
        q: q.q,
        offset: q.offset,
    };
    match m.request(&acct, job).await {
        Ok(Out::List(p)) => Json(p).into_response(),
        Ok(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "回應錯誤"),
        Err(e) => bad_gateway(e),
    }
}

async fn raw(m: &Shared, acct: &str, id: &str) -> Result<std::sync::Arc<Vec<u8>>, Box<Response>> {
    let id: u64 = id.parse().map_err(|_| Box::new(err(StatusCode::BAD_REQUEST, "信件 id 錯誤")))?;
    match m.request(acct, Job::Raw { id }).await {
        Ok(Out::Raw(r)) => Ok(r),
        Ok(_) => Err(Box::new(err(StatusCode::INTERNAL_SERVER_ERROR, "回應錯誤"))),
        Err(e) => Err(Box::new(bad_gateway(e))),
    }
}

#[derive(Serialize)]
struct Addr {
    name: String,
    addr: String,
}

#[derive(Serialize)]
struct Attachment {
    part: u32,
    name: String,
    size: usize,
    #[serde(rename = "type")]
    ctype: String,
}

#[derive(Serialize)]
struct MessageInfo {
    id: String,
    subject: String,
    from: Option<Addr>,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    date: i64,
    /// 內文裡有外部圖片(預設不載入)
    remote_images: bool,
    /// 這個寄件者的外部圖片一律直接載入
    trusted: bool,
    attachments: Vec<Attachment>,
}

fn addrs(list: Option<mail_parser::AddressList<'_>>) -> Vec<Addr> {
    list.map(|l| {
        l.mailboxes()
            .map(|a| Addr {
                name: a.name().unwrap_or_default().to_string(),
                addr: a.address().unwrap_or_default().to_string(),
            })
            .collect()
    })
    .unwrap_or_default()
}

fn content_type(p: &mail_parser::MessagePart<'_>) -> String {
    p.content_type()
        .map(|c| match c.subtype() {
            Some(s) => format!("{}/{}", c.ctype(), s),
            None => c.ctype().to_string(),
        })
        .unwrap_or_else(|| "application/octet-stream".into())
        .to_ascii_lowercase()
}

pub async fn message(State(m): State<Shared>, Path((acct, id)): Path<(String, String)>) -> Response {
    let raw = match raw(&m, &acct, &id).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let Some(msg) = MessageParser::new().parse(raw.as_slice()) else {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "無法解析這封信");
    };
    let attachments = msg
        .attachments()
        .map(|p| Attachment {
            part: p.id(),
            name: p.attachment_name().unwrap_or("未命名").to_string(),
            size: p.decoded().len(),
            ctype: content_type(&p),
        })
        .collect();
    let html = msg.body_html(0).unwrap_or_default();
    let from = addrs(msg.from()).into_iter().next();
    let trusted = from.as_ref().is_some_and(|f| m.is_trusted(&f.addr));
    let info = MessageInfo {
        id,
        subject: msg.subject().unwrap_or("").to_string(),
        from,
        trusted,
        to: addrs(msg.to()),
        cc: addrs(msg.cc()),
        date: msg.date().map(|d| d.to_timestamp()).unwrap_or(0),
        remote_images: has_remote_images(&html),
        attachments,
    };
    Json(info).into_response()
}

fn has_remote_images(html: &str) -> bool {
    let l = html.to_ascii_lowercase();
    ["src=\"http", "src='http", "src=http", "url(http", "url('http", "url(\"http", "background=\"http"]
        .iter()
        .any(|p| l.contains(p))
}

#[derive(Deserialize)]
pub struct BodyQuery {
    #[serde(default)]
    images: u8,
}

/// 信件內文: 放在 sandbox 的 iframe 裡(不執行程式), CSP 決定能不能載入外部圖片
pub async fn body(
    State(m): State<Shared>,
    Path((acct, id)): Path<(String, String)>,
    Query(q): Query<BodyQuery>,
) -> Response {
    let raw = match raw(&m, &acct, &id).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let Some(msg) = MessageParser::new().parse(raw.as_slice()) else {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "無法解析這封信");
    };
    let mut html = match msg.body_html(0) {
        Some(h) => h.into_owned(),
        None => mail_parser::text_to_html(&msg.body_text(0).unwrap_or_default()),
    };
    // 內嵌圖片 cid:xxx → 這封信的 part
    for p in msg.parts() {
        if let Some(cid) = p.content_id() {
            let cid = cid.trim_matches(['<', '>']);
            if !cid.is_empty() {
                html = html.replace(&format!("cid:{cid}"), &format!("part/{}", p.id()));
            }
        }
    }
    let page = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><base target=\"_blank\">\
         <style>html{{background:#fff;color:#222}}html,body{{margin:0;overflow-x:hidden}}\
         body{{font:14px/1.55 -apple-system,system-ui,sans-serif;overflow-wrap:anywhere}}\
         #cng6-mail{{padding:14px;transform-origin:0 0}}\
         img{{max-width:100%;height:auto}}pre{{white-space:pre-wrap}}</style></head>\
         <body><div id=\"cng6-mail\">{html}</div></body></html>"
    );
    let trusted = addrs(msg.from()).first().is_some_and(|f| m.is_trusted(&f.addr));
    let remote = if q.images == 1 || trusted { " https: http:" } else { "" };
    let csp = format!(
        "default-src 'none'; img-src 'self' data:{remote}; style-src 'unsafe-inline'{remote}; font-src data:{remote}; \
         sandbox allow-same-origin allow-popups allow-popups-to-escape-sandbox"
    );
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
            (header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&csp).unwrap_or(HeaderValue::from_static("sandbox"))),
            (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300")),
            (header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
        ],
        page,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct PartQuery {
    #[serde(default)]
    dl: u8,
}

/// 附件或內嵌圖片
pub async fn part(
    State(m): State<Shared>,
    Path((acct, id, n)): Path<(String, String, u32)>,
    Query(q): Query<PartQuery>,
) -> Response {
    let raw = match raw(&m, &acct, &id).await {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let Some(msg) = MessageParser::new().parse(raw.as_slice()) else {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "無法解析這封信");
    };
    let Some(p) = msg.part(n) else {
        return err(StatusCode::NOT_FOUND, "沒有這個附件");
    };
    let name = p.attachment_name().unwrap_or("attachment").to_string();
    let ctype = content_type(&p);
    let data = p.decoded().into_owned();
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&ctype).unwrap_or(HeaderValue::from_static("application/octet-stream")),
            ),
            (
                header::CONTENT_DISPOSITION,
                disposition(if q.dl == 1 { "attachment" } else { "inline" }, &name),
            ),
            // 附件可能是 HTML: 不能在這個網站執行程式
            (header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'")),
            (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600")),
        ],
        data,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct TrustBody {
    addr: String,
    trust: bool,
}

pub async fn trust(State(m): State<Shared>, Json(b): Json<TrustBody>) -> Response {
    match m.set_trusted(&b.addr, b.trust) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
pub struct OpBody {
    ids: Vec<String>,
    op: String,
}

pub async fn op(State(m): State<Shared>, Path(acct): Path<String>, Json(b): Json<OpBody>) -> Response {
    let Some(op) = Op::parse(&b.op) else {
        return err(StatusCode::BAD_REQUEST, "不支援的動作");
    };
    let ids: Result<Vec<u64>, _> = b.ids.iter().map(|s| s.parse::<u64>()).collect();
    let Ok(ids) = ids else {
        return err(StatusCode::BAD_REQUEST, "信件 id 錯誤");
    };
    if ids.is_empty() {
        return err(StatusCode::BAD_REQUEST, "沒有選擇信件");
    }
    match m.request(&acct, Job::Op { ids, op }).await {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => bad_gateway(e),
    }
}

// ------------------------------------------------------------------ 頭像

/// 個人信箱的網域: 顯示這些網域的圖示沒有意義(每個人都一樣), 前端用名字的第一個字
const PERSONAL: &[&str] = &[
    "gmail.com", "googlemail.com", "yahoo.com", "yahoo.com.tw", "ymail.com", "hotmail.com", "outlook.com",
    "live.com", "msn.com", "icloud.com", "me.com", "mac.com", "aol.com", "protonmail.com", "proton.me",
    "qq.com", "163.com", "126.com", "hinet.net", "msa.hinet.net", "gmx.com", "mail.com", "yandex.com",
];

/// 寄件者地址 → 公司的主網域(例如 email.apple.com → apple.com, mail.pchome.com.tw → pchome.com.tw)
pub fn org_domain(addr: &str) -> Option<String> {
    let host = addr.rsplit_once('@')?.1.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() < 2 {
        return None;
    }
    // 例如 com.tw、co.uk: 主網域要取三段
    let second = labels[labels.len() - 2];
    let tld = labels[labels.len() - 1];
    let keep = if tld.len() == 2 && ["com", "co", "org", "net", "edu", "gov", "ac", "or", "ne", "idv"].contains(&second) {
        3
    } else {
        2
    };
    let d = labels[labels.len().saturating_sub(keep)..].join(".");
    if PERSONAL.contains(&d.as_str()) || PERSONAL.contains(&host.as_str()) {
        return None;
    }
    Some(d)
}

fn avatar_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"));
    base.join("cng6").join("mail-avatars")
}

/// PNG 的寬度(圖示太小代表沒有找到, Google 會回一個 16px 的地球)
fn png_width(b: &[u8]) -> Option<u32> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some(u32::from_be_bytes([b[16], b[17], b[18], b[19]]))
}

fn fetch_avatar(domain: &str) -> Option<Vec<u8>> {
    let dir = avatar_dir();
    let png = dir.join(format!("{domain}.png"));
    let none = dir.join(format!("{domain}.none"));
    if let Ok(b) = std::fs::read(&png) {
        return Some(b);
    }
    // 找不到的也記住一週, 不要一直問
    if let Ok(m) = std::fs::metadata(&none)
        && m.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e.as_secs() < 7 * 86400)
    {
        return None;
    }
    let _ = std::fs::create_dir_all(&dir);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let got = agent
        .get(format!("https://www.google.com/s2/favicons?domain={domain}&sz=64"))
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().with_config().limit(512 * 1024).read_to_vec().ok())
        .filter(|b| png_width(b).is_some_and(|w| w >= 32));
    match &got {
        Some(b) => {
            let tmp = dir.join(format!("{domain}.png.tmp"));
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &png);
            }
        }
        None => {
            let _ = std::fs::write(&none, b"");
        }
    }
    got
}

pub async fn avatar(Path(domain): Path<String>) -> Response {
    let Some(d) = org_domain(&format!("x@{domain}")) else {
        return err(StatusCode::NOT_FOUND, "沒有圖示");
    };
    let got = tokio::task::spawn_blocking(move || fetch_avatar(&d)).await.ok().flatten();
    match got {
        Some(b) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
                (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=604800")),
            ],
            b,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"))],
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_domains() {
        assert_eq!(org_domain("no-reply@email.apple.com").as_deref(), Some("apple.com"));
        assert_eq!(org_domain("a@mail.pchome.com.tw").as_deref(), Some("pchome.com.tw"));
        assert_eq!(org_domain("a@github.com").as_deref(), Some("github.com"));
        assert_eq!(org_domain("a@gmail.com"), None);
        assert_eq!(org_domain("a@yahoo.com.tw"), None);
        assert_eq!(org_domain("a@msa.hinet.net"), None);
        assert_eq!(org_domain("a@x"), None);
        assert_eq!(org_domain("a@ev il.com"), None);
    }

    #[test]
    fn remote_images() {
        assert!(has_remote_images("<img SRC=\"https://t.example/p.gif\">"));
        assert!(has_remote_images("<td style=\"background:url(http://x/y.png)\">"));
        assert!(!has_remote_images("<img src=\"cid:abc\"><p>http://example.com</p>"));
    }

    #[test]
    fn parse_parts() {
        let raw = b"From: =?UTF-8?B?5p6X?= <a@example.com>\r\nTo: b@example.com\r\nSubject: =?UTF-8?B?5ris6Kmm?=\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=X\r\n\r\n--X\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>hi <img src=\"cid:img1\"></p>\r\n--X\r\nContent-Type: image/png\r\nContent-ID: <img1>\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--X\r\nContent-Type: application/pdf; name=\"r.pdf\"\r\nContent-Disposition: attachment; filename=\"r.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--X--\r\n";
        let msg = MessageParser::new().parse(&raw[..]).unwrap();
        assert_eq!(msg.subject(), Some("測試"));
        assert_eq!(addrs(msg.from())[0].name, "林");
        let names: Vec<_> = msg.attachments().filter_map(|p| p.attachment_name()).collect();
        assert!(names.contains(&"r.pdf"));
        let cid: Vec<_> = msg.parts().filter_map(|p| p.content_id()).collect();
        assert_eq!(cid, vec!["img1"]);
    }
}
