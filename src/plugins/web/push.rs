//! Web Push: 網頁(PWA)關掉時也能收到通知(新郵件、發票中獎)。
//!
//! - 瀏覽器訂閱後把 endpoint 和金鑰交給我們, 存在 web-push.json(權限 600)
//! - 有通知時, 用 VAPID(RFC 8292)簽名, 內容用 RFC 8291(aes128gcm)加密, POST 到瀏覽器廠商的推播服務
//! - 推播服務回 404/410 代表訂閱已失效, 直接刪掉
//!
//! 需要瀏覽器信任的 HTTPS(例如 Tailscale 的憑證), 自簽憑證不能註冊 service worker。

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use ring::{aead, agreement, hkdf};
use serde::{Deserialize, Serialize};

const FILE: &str = "web-push.json";
/// 通知在推播服務最多等多久(裝置離線時)
const TTL: u32 = 24 * 3600;
/// VAPID 的聯絡方式: 推播服務出問題時用, 這裡不放個人資料
const SUBJECT: &str = "mailto:cng6@example.com";

#[derive(Serialize, Deserialize, Clone)]
pub struct Sub {
    pub endpoint: String,
    /// 瀏覽器的 ECDH 公鑰 (base64url, 65 bytes)
    pub p256dh: String,
    /// 瀏覽器的 auth secret (base64url, 16 bytes)
    pub auth: String,
    /// 哪台裝置 (瀏覽器提供的簡短說明)
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created: i64,
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    /// VAPID 私鑰 (PKCS#8, base64url); 換掉的話所有訂閱都要重新訂
    #[serde(default)]
    vapid: String,
    #[serde(default)]
    subs: Vec<Sub>,
}

/// 一則通知: 給 service worker 顯示
#[derive(Serialize, Clone)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// 同一個 tag 的通知會互相取代
    pub tag: String,
    /// 點通知要開的信 (帳號, 信件 id), 沒有就只打開網頁
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mail: Option<(String, String)>,
}

struct Inner {
    file: PathBuf,
    store: Mutex<Store>,
    key: EcdsaKeyPair,
    rng: SystemRandom,
    log: OnceLock<Sender<String>>,
}

#[derive(Clone)]
pub struct Shared(Arc<Inner>);

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self::with_file(PathBuf::from(FILE))
    }

    fn with_file(file: PathBuf) -> Self {
        let rng = SystemRandom::new();
        let mut store: Store = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let parsed = b64url_decode(&store.vapid)
            .and_then(|p| EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &p, &rng).ok());
        let key = match parsed {
            Some(k) => k,
            None => {
                // 第一次(或檔案壞了): 產生新的 VAPID 金鑰, 舊的訂閱不能用了
                let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .expect("無法產生 VAPID 金鑰");
                store = Store { vapid: b64url(doc.as_ref()), subs: Vec::new() };
                let _ = save(&file, &store);
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, doc.as_ref(), &rng)
                    .expect("VAPID 金鑰格式錯誤")
            }
        };
        Self(Arc::new(Inner { file, store: Mutex::new(store), key, rng, log: OnceLock::new() }))
    }

    /// 錯誤訊息送到 web 的訊息區 (web 啟動時設定)
    pub fn set_log(&self, log: Sender<String>) {
        let _ = self.0.log.set(log);
    }

    fn log(&self, text: String) {
        if let Some(l) = self.0.log.get() {
            let _ = l.send(format!("web: 推播 {text}"));
        }
    }

    /// 瀏覽器訂閱時要用的公鑰 (base64url)
    pub fn public_key(&self) -> String {
        b64url(self.0.key.public_key().as_ref())
    }

    pub fn count(&self) -> usize {
        self.0.store.lock().unwrap().subs.len()
    }

    fn has(&self, endpoint: &str) -> bool {
        self.0.store.lock().unwrap().subs.iter().any(|s| s.endpoint == endpoint)
    }

    fn subscribe(&self, sub: Sub) -> Result<(), String> {
        if !sub.endpoint.starts_with("https://") {
            return Err("endpoint 必須是 https".into());
        }
        if b64url_decode(&sub.p256dh).map(|k| k.len()) != Some(65)
            || b64url_decode(&sub.auth).map(|k| k.len()) != Some(16)
        {
            return Err("訂閱的金鑰格式錯誤".into());
        }
        let mut st = self.0.store.lock().unwrap();
        st.subs.retain(|s| s.endpoint != sub.endpoint);
        st.subs.push(sub);
        save(&self.0.file, &st)
    }

    fn unsubscribe(&self, endpoint: &str) -> Result<(), String> {
        let mut st = self.0.store.lock().unwrap();
        st.subs.retain(|s| s.endpoint != endpoint);
        save(&self.0.file, &st)
    }

    /// 送給所有訂閱的裝置 (背景執行, 不會卡住呼叫的人)
    pub fn notify(&self, msg: Message) {
        let subs = self.0.store.lock().unwrap().subs.clone();
        if subs.is_empty() {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let payload = serde_json::to_vec(&msg).unwrap_or_default();
            for s in subs {
                let _ = me.deliver(&s, &payload);
            }
        });
    }

    /// 送一則並處理結果: 失效的訂閱刪掉, 其他錯誤記到訊息區。回傳是否成功
    fn deliver(&self, sub: &Sub, payload: &[u8]) -> Result<(), String> {
        let who = if sub.label.is_empty() { host_of(&sub.endpoint) } else { sub.label.clone() };
        match self.send(sub, payload) {
            Ok(code) if (200..300).contains(&code) => Ok(()),
            Ok(404 | 410) => {
                let _ = self.unsubscribe(&sub.endpoint);
                self.log(format!("{who}: 訂閱已失效, 已移除"));
                Err("訂閱已失效, 請重新開啟推播".into())
            }
            Ok(code) => {
                self.log(format!("{who}: 推播服務回應 {code}"));
                Err(format!("推播服務回應 {code}"))
            }
            Err(e) => {
                self.log(format!("{who}: {e}"));
                Err(e)
            }
        }
    }

    /// 加密並送出, 回傳推播服務的 HTTP 狀態碼
    fn send(&self, sub: &Sub, payload: &[u8]) -> Result<u16, String> {
        let ua_public = b64url_decode(&sub.p256dh).ok_or("p256dh 格式錯誤")?;
        let auth = b64url_decode(&sub.auth).ok_or("auth 格式錯誤")?;
        let body = encrypt(&self.0.rng, &ua_public, &auth, payload)?;
        let jwt = self.vapid_jwt(&sub.endpoint)?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        let res = agent
            .post(&sub.endpoint)
            .header("TTL", TTL.to_string())
            .header("Urgency", "high")
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", format!("vapid t={jwt}, k={}", self.public_key()))
            .send(&body[..])
            .map_err(|e| format!("無法連到推播服務: {e}"))?;
        Ok(res.status().as_u16())
    }

    /// VAPID (RFC 8292): 用我們的私鑰簽一個 JWT, 推播服務用公鑰確認是我們送的
    fn vapid_jwt(&self, endpoint: &str) -> Result<String, String> {
        let header = b64url(br#"{"typ":"JWT","alg":"ES256"}"#);
        let exp = chrono::Utc::now().timestamp() + 12 * 3600;
        let claims = serde_json::json!({ "aud": origin_of(endpoint), "exp": exp, "sub": SUBJECT });
        let input = format!("{header}.{}", b64url(claims.to_string().as_bytes()));
        let sig = self.0.key.sign(&self.0.rng, input.as_bytes()).map_err(|_| "VAPID 簽名失敗")?;
        Ok(format!("{input}.{}", b64url(sig.as_ref())))
    }
}

/// RFC 8291: 用瀏覽器的公鑰和 auth secret 加密, 輸出 aes128gcm 格式 (RFC 8188) 的內容
fn encrypt(rng: &SystemRandom, ua_public: &[u8], auth: &[u8], payload: &[u8]) -> Result<Vec<u8>, String> {
    let fail = |_| "加密失敗".to_string();
    // 這次專用的金鑰對 (application server key)
    let as_private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, rng).map_err(fail)?;
    let as_public = as_private.compute_public_key().map_err(fail)?;
    let as_public = as_public.as_ref().to_vec();
    let ecdh = agreement::agree_ephemeral(
        as_private,
        &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public),
        |s| s.to_vec(),
    )
    .map_err(|_| "瀏覽器的公鑰無效".to_string())?;
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(fail)?;
    let (cek, nonce) = derive(&ecdh, auth, ua_public, &as_public, &salt);

    // 內容 + 0x02 (最後一個 record, 不補 padding)
    let mut data = Vec::with_capacity(payload.len() + 17);
    data.extend_from_slice(payload);
    data.push(2);
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(fail)?);
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut data)
        .map_err(fail)?;

    // header: salt(16) | record size(4) | key id 長度(1) | key id = 我們的公鑰(65)
    let mut out = Vec::with_capacity(86 + data.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&data);
    Ok(out)
}

/// RFC 8291 §3.4 的金鑰推導: 回傳 (content encryption key, nonce)
fn derive(ecdh: &[u8], auth: &[u8], ua_public: &[u8], as_public: &[u8], salt: &[u8]) -> ([u8; 16], [u8; 12]) {
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = [0u8; 32];
    hkdf_fill(auth, ecdh, &key_info, &mut ikm);
    let mut cek = [0u8; 16];
    hkdf_fill(salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek);
    let mut nonce = [0u8; 12];
    hkdf_fill(salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce);
    (cek, nonce)
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf_fill(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let info = [info];
    // 長度固定且很短(≤ 32), 不會失敗
    prk.expand(&info, Len(out.len())).and_then(|okm| okm.fill(out)).expect("HKDF");
}

/// https://host[:port]
fn origin_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?']).next().unwrap_or("");
    format!("https://{host}")
}

fn host_of(url: &str) -> String {
    origin_of(url).trim_start_matches("https://").to_string()
}

fn save(path: &std::path::Path, st: &Store) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let data = serde_json::to_vec_pretty(st).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("無法寫入 {}: {e}", tmp.display()))?;
    f.write_all(&data).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(super) fn b64url(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            out.push(B64URL[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// base64url 解碼 (也接受一般 base64 與尾端的 =)
pub(super) fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.trim_end_matches('=').bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---- 網頁 API ----

#[derive(Deserialize)]
pub struct SubscribeReq {
    endpoint: String,
    keys: SubKeys,
    #[serde(default)]
    label: String,
}

#[derive(Deserialize)]
pub struct SubKeys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
pub struct EndpointReq {
    #[serde(default)]
    endpoint: String,
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// GET /api/push?endpoint=… → 公鑰、這個 endpoint 是否已訂閱、總共幾台
pub async fn info(
    State(p): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<EndpointReq>,
) -> Response {
    Json(serde_json::json!({
        "key": p.public_key(),
        "subscribed": !q.endpoint.is_empty() && p.has(&q.endpoint),
        "count": p.count(),
    }))
    .into_response()
}

pub async fn subscribe(State(p): State<Shared>, Json(r): Json<SubscribeReq>) -> Response {
    let label: String = r.label.chars().take(60).collect();
    let sub = Sub {
        endpoint: r.endpoint,
        p256dh: r.keys.p256dh,
        auth: r.keys.auth,
        label,
        created: chrono::Utc::now().timestamp(),
    };
    match p.subscribe(sub) {
        Ok(()) => Json(serde_json::json!({ "ok": true, "count": p.count() })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn unsubscribe(State(p): State<Shared>, Json(r): Json<EndpointReq>) -> Response {
    match p.unsubscribe(&r.endpoint) {
        Ok(()) => Json(serde_json::json!({ "ok": true, "count": p.count() })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// POST /api/push/test: 送一則測試通知給這個裝置, 等推播服務的結果
pub async fn test(State(p): State<Shared>, Json(r): Json<EndpointReq>) -> Response {
    let Some(sub) = p.0.store.lock().unwrap().subs.iter().find(|s| s.endpoint == r.endpoint).cloned() else {
        return err(StatusCode::NOT_FOUND, "這個裝置還沒有訂閱");
    };
    let msg = Message {
        title: "CNG6 測試通知".into(),
        body: "推播通知可以用了".into(),
        tag: "cng6-test".into(),
        mail: None,
    };
    let payload = serde_json::to_vec(&msg).unwrap_or_default();
    let res = tokio::task::spawn_blocking(move || p.deliver(&sub, &payload)).await;
    match res {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_GATEWAY, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Service worker: 要放在網站根目錄, 才能管整個網站
pub async fn service_worker() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("assets/sw.js"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64url_round_trip() {
        for n in 0..40 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64url_decode(&b64url(&data)).unwrap(), data);
        }
        assert_eq!(b64url(b"\xfb\xff"), "-_8");
    }

    /// RFC 8291 附錄 A 的範例: 金鑰推導和加密結果要一樣
    #[test]
    fn rfc8291_example() {
        let d = |s: &str| b64url_decode(s).unwrap();
        let ua_public = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let as_public = d("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        let auth = d("BTBZMqHH6r4Tts7J_aSIgg");
        let salt = d("DGv6ra1nlYgDCS1FRnbzlw");
        // ECDH 的結果 (RFC 的範例金鑰算出來的)
        let ecdh = d("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs");
        let (cek, nonce) = derive(&ecdh, &auth, &ua_public, &as_public, &salt);
        assert_eq!(b64url(&cek), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(b64url(&nonce), "4h_95klXJ5E_qnoN");
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut data = b"When I grow up, I want to be a watermelon\x02".to_vec();
        key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut data)
            .unwrap();
        assert_eq!(b64url(&data), "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ");
    }

    /// 給外部程式驗證(Python cryptography 解密、驗簽): 讀 PUSH_UA_PUBLIC / PUSH_UA_AUTH,
    /// 印出加密後的內容與 VAPID 標頭。cargo test push_vector -- --ignored --nocapture
    #[test]
    #[ignore]
    fn push_vector() {
        let dir = std::env::temp_dir().join(format!("cng6-push-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = Shared::with_file(dir.join("web-push.json"));
        let sub = Sub {
            endpoint: "https://push.example.com/x".into(),
            p256dh: std::env::var("PUSH_UA_PUBLIC").unwrap(),
            auth: std::env::var("PUSH_UA_AUTH").unwrap(),
            label: String::new(),
            created: 0,
        };
        let body = encrypt(
            &p.0.rng,
            &b64url_decode(&sub.p256dh).unwrap(),
            &b64url_decode(&sub.auth).unwrap(),
            br#"{"title":"hello"}"#,
        )
        .unwrap();
        println!("BODY={}", b64url(&body));
        println!("JWT={}", p.vapid_jwt(&sub.endpoint).unwrap());
        println!("KEY={}", p.public_key());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn origin() {
        assert_eq!(origin_of("https://fcm.googleapis.com/fcm/send/abc"), "https://fcm.googleapis.com");
        assert_eq!(origin_of("https://web.push.apple.com:443/x?y"), "https://web.push.apple.com:443");
    }
}
