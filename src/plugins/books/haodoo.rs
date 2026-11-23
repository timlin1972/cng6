//! 從好讀(haodoo.net)下載 EPUB
//!
//! 好讀沒有站內搜尋(用 Google), 所以把各分類的書單抓下來做成索引,
//! 快取在 `books/.cache/haodoo.json`, 在 Pi 上搜尋。
//!
//! - 分類頁 `?M=hd&P={分類}`、`?M=hd&P={分類}-{n}`: `<font color="CC0000">作者</font>` 後面接著
//!   `<a href="?M=book&P={代號}">【書名】</a>`(新上架的是 `?M=Share&P=…`)
//! - 書頁 `?M={book|Share}&P={代號}`: 一冊一段, `<font color="CC0000">作者</font>《書名》`
//!   後面有 `DownloadEpub('{檔名代號}')`、`DownloadVEpub('{直排檔名代號}')`
//! - 檔案 `/PDB/{代號第一個字}/{其餘}.epub`

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

const BASE: &str = "https://www.haodoo.net/";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";
/// 索引多久重抓一次
const INDEX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// EPUB 大小上限
const MAX_EPUB: u64 = 200 * 1024 * 1024;

pub const CATEGORIES: &[(&str, &str)] = &[
    ("100", "世紀百強"),
    ("wisdom", "隨身智囊"),
    ("history", "歷史煙雲"),
    ("martial", "武俠小說"),
    ("mystery", "懸疑小說"),
    ("romance", "言情小說"),
    ("scifi", "奇幻小說"),
    ("fiction", "小說園地"),
];

/// 書單上的一部書(可能有好幾冊)
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    /// "book" 或 "Share"
    pub m: String,
    pub id: String,
    pub title: String,
    pub author: String,
    pub category: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Index {
    pub updated: u64,
    pub categories: Vec<(String, String)>,
    pub books: Vec<Entry>,
}

/// 書頁裡的一冊
#[derive(Serialize, Debug)]
pub struct Volume {
    pub title: String,
    pub author: String,
    /// 橫排 EPUB 的檔名代號
    pub epub: Option<String>,
    /// 直排 EPUB 的檔名代號
    pub vepub: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(UA)
        .build()
        .new_agent()
}

fn get_page(agent: &ureq::Agent, m: &str, p: &str) -> Result<String, String> {
    agent
        .get(BASE)
        .query("M", m)
        .query("P", p)
        .call()
        .map_err(|e| format!("無法連到好讀: {e}"))?
        .body_mut()
        .with_config()
        .limit(10 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("無法讀取好讀網頁: {e}"))
}

/// 網頁的主要內容(去掉選單、側欄)
fn main_content(html: &str) -> &str {
    let start = html.find("Main content").unwrap_or(0);
    let end = html[start..]
        .find("bottom link")
        .map_or(html.len(), |e| start + e);
    &html[start..end]
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

fn trim_brackets(s: &str) -> String {
    s.trim()
        .trim_start_matches(['【', '《'])
        .trim_end_matches(['】', '》'])
        .trim()
        .to_string()
}

/// 解析分類頁的書單
fn parse_list(html: &str, category: &str, out: &mut Vec<Entry>) {
    let html = main_content(html);
    let mut author = String::new();
    let mut rest = html;
    loop {
        let a = rest.find(r#"<font color="CC0000">"#);
        let l = rest.find(r#"<a href="?M="#);
        match (a, l) {
            (Some(a), l) if l.is_none_or(|l| a < l) => {
                rest = &rest[a + r#"<font color="CC0000">"#.len()..];
                let end = rest.find("</font>").unwrap_or(0);
                author = strip_tags(&rest[..end]);
                rest = &rest[end..];
            }
            (_, Some(l)) => {
                rest = &rest[l + r#"<a href="?M="#.len()..];
                let Some(q) = rest.find('"') else { break };
                let href = &rest[..q];
                let Some(close) = rest.find("</a>") else { break };
                let text = strip_tags(&format!("<{}", &rest[q..close]));
                rest = &rest[close..];
                let Some((m, id)) = href.split_once("&P=") else {
                    continue;
                };
                let title = trim_brackets(&text);
                if !(m == "book" || m == "Share") || title.is_empty() || id.is_empty() {
                    continue;
                }
                if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                    continue;
                }
                out.push(Entry {
                    m: m.into(),
                    id: id.into(),
                    title,
                    author: author.clone(),
                    category: category.into(),
                });
            }
            _ => break,
        }
    }
}

/// 分類頁有幾頁(`?M=hd&P={分類}-{n}`)
fn page_count(html: &str, category: &str) -> usize {
    let pat = format!(r#"href="?M=hd&P={category}-"#);
    html.match_indices(&pat)
        .filter_map(|(i, _)| {
            let s = &html[i + pat.len()..];
            s[..s.find('"')?].parse::<usize>().ok()
        })
        .max()
        .unwrap_or(1)
}

fn crawl_category(agent: &ureq::Agent, category: &str) -> Result<Vec<Entry>, String> {
    let first = get_page(agent, "hd", category)?;
    let mut out = Vec::new();
    parse_list(&first, category, &mut out);
    // 第 1 頁就是分類首頁
    for n in 2..=page_count(&first, category).min(50) {
        std::thread::sleep(Duration::from_millis(200));
        let html = get_page(agent, "hd", &format!("{category}-{n}"))?;
        parse_list(&html, category, &mut out);
    }
    Ok(out)
}

fn crawl() -> Result<Index, String> {
    let agent = agent();
    let results: Vec<Result<Vec<Entry>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = CATEGORIES
            .iter()
            .map(|(c, _)| {
                let agent = agent.clone();
                s.spawn(move || crawl_category(&agent, c))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("內部錯誤".into())))
            .collect()
    });
    let mut books: Vec<Entry> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in results {
        for e in r? {
            // 同一部書可能出現在「最新上架」和作者書單, 留第一個
            if seen.insert((e.m.clone(), e.id.clone())) {
                books.push(e);
            }
        }
    }
    if books.is_empty() {
        return Err("好讀的網頁格式可能改了, 讀不到書單".into());
    }
    Ok(Index {
        updated: chrono::Utc::now().timestamp().max(0) as u64,
        categories: CATEGORIES
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        books,
    })
}

/// 書單索引: 有快取且沒過期就用快取
pub fn index(books_dir: &Path, refresh: bool) -> Result<Index, String> {
    let path = books_dir.join(".cache").join("haodoo.json");
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < INDEX_TTL);
    if !refresh
        && fresh
        && let Some(idx) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
    {
        return Ok(idx);
    }
    match crawl() {
        Ok(idx) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if let Ok(json) = serde_json::to_vec(&idx)
                && std::fs::write(&tmp, json).is_ok()
            {
                let _ = std::fs::rename(&tmp, &path);
            }
            Ok(idx)
        }
        // 抓不到就用舊的快取
        Err(e) => std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
            .ok_or(e),
    }
}

fn js_arg(seg: &str, func: &str) -> Option<String> {
    let pat = format!("{func}('");
    let i = seg.find(&pat)? + pat.len();
    let s = &seg[i..];
    let code = &s[..s.find('\'')?];
    valid_code(code).then(|| code.to_string())
}

fn valid_code(code: &str) -> bool {
    code.len() >= 2 && code.len() <= 40 && code.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 書頁上的各冊
pub fn volumes(m: &str, id: &str) -> Result<Vec<Volume>, String> {
    if !(m == "book" || m == "Share") || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("書的代號不對".into());
    }
    let html = get_page(&agent(), m, id)?;
    let html = main_content(&html);
    let marker = r#"<font color="CC0000">"#;
    let mut out = Vec::new();
    for seg in html.split(marker).skip(1) {
        let Some(end) = seg.find("</font>") else {
            continue;
        };
        let author = strip_tags(&seg[..end]);
        let after = &seg[end..];
        let title = after
            .find('《')
            .and_then(|i| {
                let s = &after[i..];
                s.find('》').map(|j| trim_brackets(&s[..j + '》'.len_utf8()]))
            })
            .unwrap_or_default();
        // 新上架的書頁只有「從好讀二代下載 EPUB」, 但檔案一樣放在 /PDB/, 代號和 updb 相同
        let epub = js_arg(seg, "DownloadEpub").or_else(|| {
            seg.contains("haodoo.org/?product=")
                .then(|| js_arg(seg, "DownloadUpdb"))
                .flatten()
        });
        let vepub = js_arg(seg, "DownloadVEpub");
        if epub.is_none() && vepub.is_none() {
            continue;
        }
        out.push(Volume {
            title,
            author,
            epub,
            vepub,
        });
    }
    Ok(out)
}

/// 檔名裡不能用的字換掉
fn safe_file_name(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "好讀".into() } else { s }
}

/// 下載一冊 EPUB 到書庫, 回傳存檔的檔名
pub fn download(books_dir: &Path, code: &str, title: &str, vertical: bool) -> Result<String, String> {
    if !valid_code(code) {
        return Err("書的代號不對".into());
    }
    let url = format!("{BASE}PDB/{}/{}.epub", &code[..1], &code[1..]);
    let data = agent()
        .get(&url)
        .call()
        .map_err(|e| format!("無法下載: {e}"))?
        .body_mut()
        .with_config()
        .limit(MAX_EPUB)
        .read_to_vec()
        .map_err(|e| format!("無法下載: {e}"))?;
    // EPUB 是 zip
    if !data.starts_with(b"PK") {
        return Err("好讀沒有這本書的 EPUB 檔".into());
    }
    std::fs::create_dir_all(books_dir).map_err(|e| format!("無法建立資料夾: {e}"))?;
    let stem = safe_file_name(&format!(
        "{}{}",
        if title.trim().is_empty() { code } else { title.trim() },
        if vertical { "(直排)" } else { "" }
    ));
    let name = std::iter::once(format!("{stem}.epub"))
        .chain((1..).map(|i| format!("{stem}-{i}.epub")))
        .find(|n| !books_dir.join(n).exists())
        .unwrap();
    let tmp = books_dir.join(format!(".{name}.downloading"));
    std::fs::write(&tmp, &data)
        .and_then(|()| std::fs::rename(&tmp, books_dir.join(&name)))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("無法寫入: {e}")
        })?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_volumes() {
        let list = r#"menu <a href="?M=book&P=1">【x】</a> Main content
<font color="CC0000">金庸</font><br>
<a href="?M=book&P=55">【射鵰英雄傳】</a><a href="?M=book&P=56">《神鵰俠侶》</a>
<font color="CC0000">莫琳</font><a href="?M=Share&P=1010490737">【掠殺】</a><br>
<a href="?M=hd&P=mystery-2">2</a>
bottom link <a href="?M=book&P=9">【y】</a>"#;
        let mut out = Vec::new();
        parse_list(list, "martial", &mut out);
        let got: Vec<_> = out
            .iter()
            .map(|e| (e.m.as_str(), e.id.as_str(), e.title.as_str(), e.author.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("book", "55", "射鵰英雄傳", "金庸"),
                ("book", "56", "神鵰俠侶", "金庸"),
                ("Share", "1010490737", "掠殺", "莫琳"),
            ]
        );
        assert_eq!(page_count(r#"<a href="?M=hd&P=scifi-1">1<a href="?M=hd&P=scifi-10">"#, "scifi"), 10);
        assert_eq!(js_arg("x DownloadEpub('D10U8a') y", "DownloadEpub").as_deref(), Some("D10U8a"));
        assert_eq!(js_arg("DownloadEpub('a/../b')", "DownloadEpub"), None);
        assert_eq!(safe_file_name("a/b:c"), "a_b_c");
    }

    /// 連到好讀實際測試: cargo test haodoo::tests::live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live() {
        let dir = std::env::temp_dir().join("cng6-haodoo-test");
        let idx = index(&dir, true).unwrap();
        for (c, name) in CATEGORIES {
            let n = idx.books.iter().filter(|b| b.category == *c).count();
            println!("{name}: {n}");
            assert!(n > 10, "{name} 只有 {n} 本");
        }
        let v = volumes("book", "10U8").unwrap();
        println!("{v:?}");
        assert_eq!(v.len(), 4);
        let v = volumes("Share", "1010490737").unwrap();
        println!("{v:?}");
        let name = download(&dir, v[0].epub.as_deref().unwrap(), &v[0].title, false).unwrap();
        println!("{name}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
