//! TXT 小說: 自動判斷編碼(UTF-8 / Big5 / GBK…)、依「第…章」自動分章, 轉成 XHTML 給閱讀器顯示。

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Chapter {
    pub title: String,
    /// 這一章的段落
    #[serde(skip)]
    pub paragraphs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TxtBook {
    pub encoding: &'static str,
    pub chapters: Vec<Chapter>,
}

/// 自動判斷編碼並解碼
pub fn decode(bytes: &[u8]) -> (String, &'static str) {
    // 有 BOM 就照 BOM
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (s, _) = enc.decode_without_bom_handling(&bytes[bom_len..]);
        return (s.into_owned(), enc.name());
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), "UTF-8");
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    // 繁中小說以 Big5 居多, 提示偵測器優先考慮
    let enc = det.guess(Some(b"tw"), chardetng::Utf8Detection::Allow);
    let (s, _, _) = enc.decode(bytes);
    (s.into_owned(), enc.name())
}

/// 是不是章節標題: 「第十二章 xxx」「第3回」「序章」「楔子」「後記」…
fn is_heading(line: &str) -> bool {
    let l = line.trim();
    let n = l.chars().count();
    if n == 0 || n > 40 {
        return false;
    }
    if [
        "序章", "序言", "前言", "楔子", "引子", "尾聲", "後記", "后记", "番外", "終章", "终章",
    ]
    .iter()
    .any(|p| l.starts_with(p))
    {
        return true;
    }
    // 第 + 數字 + 章/節/回/卷/集/部/篇
    let Some(rest) = l.strip_prefix('第') else {
        return false;
    };
    let num: String = rest
        .chars()
        .take_while(|c| {
            c.is_ascii_digit()
                || c.is_whitespace()
                || "零〇一二三四五六七八九十百千萬万兩两０１２３４５６７８９".contains(*c)
        })
        .collect();
    if num.trim().is_empty() {
        return false;
    }
    rest[num.len()..]
        .chars()
        .next()
        .is_some_and(|c| "章節节回卷集部篇話话".contains(c))
}

pub fn parse(path: &Path) -> Result<TxtBook, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("無法讀取: {e}"))?;
    let (text, encoding) = decode(&bytes);
    Ok(TxtBook {
        encoding,
        chapters: split(&text),
    })
}

/// 分章; 沒有章節標題時, 每 300 段一章
pub fn split(text: &str) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut current = Chapter {
        title: String::new(),
        paragraphs: Vec::new(),
    };
    for line in text.lines() {
        let l = line.trim().trim_start_matches('\u{3000}').trim();
        if l.is_empty() {
            continue;
        }
        if is_heading(l) {
            if !current.paragraphs.is_empty() || !current.title.is_empty() {
                chapters.push(std::mem::replace(
                    &mut current,
                    Chapter {
                        title: String::new(),
                        paragraphs: Vec::new(),
                    },
                ));
            }
            current.title = l.to_string();
            continue;
        }
        current.paragraphs.push(l.to_string());
    }
    if !current.paragraphs.is_empty() || !current.title.is_empty() {
        chapters.push(current);
    }
    // 沒有偵測到章節: 每 300 段切一次
    if chapters.len() <= 1 {
        let paras = chapters.pop().map(|c| c.paragraphs).unwrap_or_default();
        chapters = paras
            .chunks(300)
            .enumerate()
            .map(|(i, c)| Chapter {
                title: format!("第 {} 部分", i + 1),
                paragraphs: c.to_vec(),
            })
            .collect();
    }
    // 第一段沒有標題的內容(書名、簡介)
    for (i, c) in chapters.iter_mut().enumerate() {
        if c.title.is_empty() {
            c.title = if i == 0 {
                "開頭".into()
            } else {
                format!("第 {} 部分", i + 1)
            };
        }
    }
    chapters
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 一章轉成 XHTML 頁面
pub fn chapter_html(book_title: &str, ch: &Chapter) -> String {
    let mut body = format!("<h2>{}</h2>\n", escape(&ch.title));
    for p in &ch.paragraphs {
        body.push_str(&format!("<p>{}</p>\n", escape(p)));
    }
    format!(
        "<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"zh-Hant\"><head><meta charset=\"utf-8\"/><title>{}</title>\
         <style>p{{text-indent:2em;margin:0 0 .6em}}h2{{text-align:center;margin:1em 0 1.2em}}</style></head><body>\n{body}</body></html>",
        escape(book_title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings() {
        assert!(is_heading("第一章 初入江湖"));
        assert!(is_heading("第123章"));
        assert!(is_heading("第十二回　風雲"));
        assert!(is_heading("楔子"));
        assert!(!is_heading("第一次見到他的時候, 他正坐在窗邊"));
        assert!(!is_heading("他說第一"));
    }

    #[test]
    fn split_and_decode() {
        let text = "書名\n作者\n\n第一章 開始\n　　第一段。\n第二段。\n第二章 結束\n最後一段。\n";
        let ch = split(text);
        let titles: Vec<_> = ch.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["開頭", "第一章 開始", "第二章 結束"]);
        assert_eq!(ch[1].paragraphs, ["第一段。", "第二段。"]);
        // Big5 編碼的中文
        let (big5, _, _) = encoding_rs::BIG5.encode("第一章 天下大勢，分久必合，合久必分。");
        let (s, enc) = decode(&big5);
        assert_eq!(enc, "Big5");
        assert!(s.starts_with("第一章"));
        assert!(chapter_html("書", &ch[1]).contains("<p>第一段。</p>"));
    }
}
