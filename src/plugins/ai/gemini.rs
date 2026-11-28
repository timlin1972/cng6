//! Google Gemini API (generativelanguage.googleapis.com) 的最小用戶端。
//!
//! - 串流: `streamGenerateContent?alt=sse`, 每收到一段就回呼, 網頁可以一邊收一邊顯示
//! - 回傳的 parts 原樣保留(function call 的 thoughtSignature 下一輪要原封不動送回去)

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// API 位址; 測試時可以用 CNG6_TEST_GEMINI_BASE 指到假的伺服器
fn base() -> String {
    std::env::var("CNG6_TEST_GEMINI_BASE").unwrap_or_else(|_| BASE.to_string())
}

/// 送出後等多久還沒開始回應就放棄(模型太忙時 Google 有時完全不回, 不會回錯誤)
const RESPONSE_WAIT: Duration = Duration::from_secs(60);
/// 串流中途停住多久就放棄
const STALL_WAIT: Duration = Duration::from_secs(120);
const BUSY: &str = "Gemini 目前沒有回應(這個模型可能太忙), 請稍後再試, 或在設定換一個模型";

/// 共用的連線(建立 TLS 設定要一點時間, 重複使用比較快)
fn agent() -> &'static ureq::Agent {
    static A: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    A.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(RESPONSE_WAIT))
            .timeout_per_read(Some(STALL_WAIT))
            .http_status_as_error(false)
            .build()
            .new_agent()
    })
}

/// API 回的錯誤訊息({"error":{"message":…}})
fn api_error(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        // AIza… 錯了回 400, AQ.… 錯了回 401
        400 | 401 if msg.contains("API key") => format!("API key 無效: {msg}"),
        401 | 403 => format!("API key 沒有權限: {msg}"),
        404 => format!("找不到這個模型: {msg}"),
        429 => format!("用量超過限制(請稍後再試): {msg}"),
        503 => format!("Gemini 這個模型目前太忙, 請稍後再試, 或在設定換一個模型({msg})"),
        _ => format!("Gemini 錯誤 {status}: {msg}"),
    }
}

/// 連線錯誤的訊息(逾時說明可能是模型太忙)
fn conn_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => BUSY.to_string(),
        e => format!("無法連到 Gemini: {e}"),
    }
}

/// 可以用來聊天的模型(名稱不含 models/ 前綴), 也用來確認 API key 能不能用
pub fn models(key: &str) -> Result<Vec<String>, String> {
    let mut res = agent()
        .get(format!("{}/models?pageSize=200", base()))
        .header("x-goog-api-key", key)
        .call()
        .map_err(conn_error)?;
    let status = res.status().as_u16();
    let body = res.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        return Err(api_error(status, &body));
    }
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut out: Vec<String> = v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "generateContent"))
        })
        .filter_map(|m| m["name"].as_str())
        .map(|n| n.trim_start_matches("models/").to_string())
        .filter(|n| n.starts_with("gemini") && !n.contains("embedding") && !n.contains("tts") && !n.contains("image"))
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// 一次串流的結果
pub struct Reply {
    /// 模型這一輪的 parts(連續的文字合併成一段)
    pub parts: Vec<Value>,
    /// 停止的原因(STOP、MAX_TOKENS、SAFETY…)
    pub finish: String,
}

/// 送出對話, 串流接收; `on_text` 每收到一段文字就呼叫一次。`stop` 設成 true 會提早結束
pub fn stream(
    key: &str,
    model: &str,
    system: &str,
    contents: &[Value],
    tools: &Value,
    stop: &AtomicBool,
    mut on_text: impl FnMut(&str),
) -> Result<Reply, String> {
    let body = json!({
        "systemInstruction": { "parts": [{ "text": system }] },
        "contents": contents,
        "tools": tools,
    });
    let mut res = agent()
        .post(format!("{}/models/{model}:streamGenerateContent?alt=sse", base()))
        .header("x-goog-api-key", key)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(conn_error)?;
    let status = res.status().as_u16();
    if status != 200 {
        let body = res.body_mut().read_to_string().unwrap_or_default();
        return Err(api_error(status, &body));
    }
    let reader = BufReader::new(res.body_mut().as_reader());
    let mut parts: Vec<Value> = Vec::new();
    let mut finish = String::new();
    for line in reader.lines() {
        if stop.load(Ordering::Relaxed) {
            finish = "STOPPED".into();
            break;
        }
        let line = line.map_err(|e| {
            // ureq 的逾時包在 io::Error 裡
            let timeout = e.kind() == std::io::ErrorKind::TimedOut
                || e.get_ref().and_then(|x| x.downcast_ref::<ureq::Error>()).is_some_and(|x| matches!(x, ureq::Error::Timeout(_)));
            if timeout { BUSY.to_string() } else { format!("連線中斷: {e}") }
        })?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(chunk) = serde_json::from_str::<Value>(data.trim()) else { continue };
        if let Some(e) = chunk.get("error") {
            return Err(format!("Gemini 錯誤: {}", e["message"].as_str().unwrap_or("未知")));
        }
        let cand = &chunk["candidates"][0];
        if let Some(f) = cand["finishReason"].as_str() {
            finish = f.to_string();
        }
        if let Some(r) = chunk["promptFeedback"]["blockReason"].as_str() {
            return Err(format!("內容被 Gemini 擋下來了({r})"));
        }
        for p in cand["content"]["parts"].as_array().into_iter().flatten() {
            merge_part(&mut parts, p.clone(), &mut on_text);
        }
    }
    Ok(Reply { parts, finish })
}

/// 把串流收到的一段 part 加進去: 一般文字接在前一段文字後面, 其他(function call、思考)原樣保留
fn merge_part(parts: &mut Vec<Value>, p: Value, on_text: &mut impl FnMut(&str)) {
    let thought = p["thought"].as_bool() == Some(true);
    if let (Some(t), false) = (p["text"].as_str(), thought) {
        on_text(t);
        if let Some(last) = parts.last_mut()
            && last.get("text").is_some()
            && last["thought"].as_bool() != Some(true)
        {
            let joined = format!("{}{t}", last["text"].as_str().unwrap_or(""));
            last["text"] = Value::String(joined);
            // 簽名通常在最後一段: 留著(文字的簽名不是必要的, 但送回去比較好)
            if let Some(sig) = p.get("thoughtSignature") {
                last["thoughtSignature"] = sig.clone();
            }
            return;
        }
    }
    parts.push(p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_streamed_text() {
        let mut parts = Vec::new();
        let mut seen = String::new();
        let mut cb = |t: &str| seen.push_str(t);
        merge_part(&mut parts, json!({"text": "你好"}), &mut cb);
        merge_part(&mut parts, json!({"text": ",世界"}), &mut cb);
        merge_part(&mut parts, json!({"functionCall": {"name": "x", "args": {}}, "thoughtSignature": "s"}), &mut cb);
        merge_part(&mut parts, json!({"text": "後面", "thoughtSignature": "t"}), &mut cb);
        assert_eq!(seen, "你好,世界後面");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "你好,世界");
        assert_eq!(parts[1]["thoughtSignature"], "s");
        assert_eq!(parts[2]["thoughtSignature"], "t");
    }

    #[test]
    fn error_messages() {
        let e = api_error(400, r#"{"error":{"message":"API key not valid. Please pass a valid API key."}}"#);
        assert!(e.starts_with("API key 無效"), "{e}");
        let e = api_error(401, r#"{"error":{"message":"API key not valid. Please pass a valid API key."}}"#);
        assert!(e.starts_with("API key 無效"), "{e}");
        assert!(api_error(429, "{}").starts_with("用量超過"));
        assert!(api_error(503, r#"{"error":{"message":"high demand"}}"#).contains("太忙"));
    }
}
