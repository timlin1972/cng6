//! 書庫: 掃描 books/、各格式的書本資訊(依修改時間快取)、漫畫頁面、MOBI 轉檔、閱讀進度。
//!
//! - EPUB: 直接解析
//! - MOBI / AZW / AZW3: 用 Calibre 的 ebook-convert 在背景轉成 EPUB(存在 .cache/)
//! - TXT: 自動判斷編碼與分章
//! - PDF: 網頁用 pdf.js 顯示
//! - CBZ / ZIP: 壓縮檔裡的圖片, 一頁一張
//! - 閱讀進度與書籤存在 books/.progress.json(跟著書走)

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::epub::{self, EpubInfo, TocItem};
use super::txt;

pub const CACHE_DIR: &str = ".cache";
const PROGRESS_FILE: &str = ".progress.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Epub,
    Mobi,
    Txt,
    Pdf,
    Comic,
}

pub fn kind_of(name: &str) -> Option<Kind> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "epub" => Kind::Epub,
        "mobi" | "azw" | "azw3" | "kfx" => Kind::Mobi,
        "txt" => Kind::Txt,
        "pdf" => Kind::Pdf,
        "cbz" | "zip" => Kind::Comic,
        _ => return None,
    })
}

pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 400
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif", "bmp"];

fn is_image(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

/// 自然排序: "p2.jpg" < "p10.jpg"
fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut out = Vec::new();
    let mut num = String::new();
    let mut txt = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !txt.is_empty() {
                out.push((1, 0, std::mem::take(&mut txt).to_lowercase()));
            }
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((0, num.parse().unwrap_or(u64::MAX), String::new()));
                num.clear();
            }
            txt.push(c);
        }
    }
    if !num.is_empty() {
        out.push((0, num.parse().unwrap_or(u64::MAX), String::new()));
    }
    if !txt.is_empty() {
        out.push((1, 0, txt.to_lowercase()));
    }
    out
}

/// 一本書的內容結構(閱讀器用)
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    /// EPUB(含 MOBI 轉出的): 章節是 zip 裡的路徑
    Epub {
        direction: String,
        chapters: Vec<String>,
        toc: Vec<TocItem>,
    },
    /// TXT: 章節標題(內容另外向伺服器要)
    Txt {
        encoding: String,
        chapters: Vec<String>,
    },
    Pdf,
    /// 漫畫: 頁數
    Comic {
        pages: usize,
    },
    /// MOBI 轉檔中或失敗
    Converting {
        error: Option<String>,
    },
}

/// 書庫列表的一本書
#[derive(Debug, Clone, Serialize)]
pub struct Book {
    /// 檔名(在資料夾中唯一, 當作識別)
    pub file: String,
    pub kind: Kind,
    pub title: String,
    pub author: String,
    pub size: u64,
    pub has_cover: bool,
    /// 讀到哪裡(0~1)與最後閱讀時間(Unix 秒)
    pub percent: f64,
    pub last_read: u64,
}

#[derive(Clone)]
struct Parsed {
    mtime: SystemTime,
    title: String,
    author: String,
    cover: Option<String>,
    content: Content,
    /// TXT 的章節內容
    txt: Option<Arc<txt::TxtBook>>,
    /// 漫畫的圖片(zip 裡的路徑, 已排序)
    pages: Vec<String>,
}

#[derive(Default)]
pub struct Library {
    cache: Mutex<HashMap<PathBuf, Parsed>>,
    /// MOBI 轉檔中的書
    converting: Mutex<HashMap<String, Option<String>>>,
    progress: Mutex<()>,
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

impl Library {
    /// MOBI 轉出的 EPUB 位置
    pub fn converted_path(dir: &Path, file: &str) -> PathBuf {
        dir.join(CACHE_DIR).join(format!("{file}.epub"))
    }

    /// 實際要讀的 EPUB(MOBI 用轉出的檔案)
    pub fn epub_path(dir: &Path, file: &str) -> PathBuf {
        match kind_of(file) {
            Some(Kind::Mobi) => Self::converted_path(dir, file),
            _ => dir.join(file),
        }
    }

    fn parse(&self, dir: &Path, file: &str, kind: Kind, mtime: SystemTime) -> Parsed {
        let path = dir.join(file);
        let fallback = |content| Parsed {
            mtime,
            title: stem(file).to_string(),
            author: String::new(),
            cover: None,
            content,
            txt: None,
            pages: Vec::new(),
        };
        match kind {
            Kind::Epub | Kind::Mobi => {
                let epath = Self::epub_path(dir, file);
                if kind == Kind::Mobi && !epath.is_file() {
                    let err = self.converting.lock().unwrap().get(file).cloned().flatten();
                    return fallback(Content::Converting { error: err });
                }
                match epub::parse(&epath) {
                    Ok(EpubInfo {
                        title,
                        author,
                        direction,
                        chapters,
                        toc,
                        cover,
                        ..
                    }) => Parsed {
                        mtime,
                        title: if title.is_empty() {
                            stem(file).to_string()
                        } else {
                            title
                        },
                        author,
                        cover,
                        content: Content::Epub {
                            direction,
                            chapters,
                            toc,
                        },
                        txt: None,
                        pages: Vec::new(),
                    },
                    Err(e) => fallback(Content::Converting { error: Some(e) }),
                }
            }
            Kind::Txt => match txt::parse(&path) {
                Ok(book) => {
                    let mut p = fallback(Content::Txt {
                        encoding: book.encoding.to_string(),
                        chapters: book.chapters.iter().map(|c| c.title.clone()).collect(),
                    });
                    p.txt = Some(Arc::new(book));
                    p
                }
                Err(e) => fallback(Content::Converting { error: Some(e) }),
            },
            Kind::Pdf => fallback(Content::Pdf),
            Kind::Comic => {
                let pages = std::fs::File::open(&path)
                    .ok()
                    .and_then(|f| zip::ZipArchive::new(std::io::BufReader::new(f)).ok())
                    .map(|z| {
                        let mut names: Vec<String> = z
                            .file_names()
                            .filter_map(Result::ok)
                            .map(|n| n.to_string())
                            .filter(|n| is_image(n) && !n.starts_with("__MACOSX"))
                            .collect();
                        names.sort_by_key(|n| natural_key(n));
                        names
                    })
                    .unwrap_or_default();
                let mut p = fallback(Content::Comic { pages: pages.len() });
                p.cover = pages.first().cloned();
                p.pages = pages;
                p
            }
        }
    }

    /// 取得一本書的解析結果(依修改時間快取)
    fn get(&self, dir: &Path, file: &str) -> Option<Parsed> {
        if !is_safe_name(file) {
            return None;
        }
        let kind = kind_of(file)?;
        let path = dir.join(file);
        let md = std::fs::metadata(&path).ok()?;
        // MOBI: 用轉出檔案的時間, 轉好後才會重新解析
        let mtime = match kind {
            Kind::Mobi => std::fs::metadata(Self::converted_path(dir, file))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH),
            _ => md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        };
        if let Some(p) = self.cache.lock().unwrap().get(&path)
            && p.mtime == mtime
            && !matches!(p.content, Content::Converting { error: None })
        {
            return Some(p.clone());
        }
        let p = self.parse(dir, file, kind, mtime);
        self.cache.lock().unwrap().insert(path, p.clone());
        Some(p)
    }

    pub fn scan(&self, dir: &Path) -> Vec<Book> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let progress = self.load_progress(dir);
        let mut books: Vec<Book> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                // 跟著連結(symlink)找到實際的檔案
                let md = std::fs::metadata(e.path()).ok()?;
                if !md.is_file() || !is_safe_name(&name) {
                    return None;
                }
                let kind = kind_of(&name)?;
                let p = self.get(dir, &name)?;
                let prog = progress.get(&name);
                Some(Book {
                    kind,
                    title: p.title,
                    author: p.author,
                    size: md.len(),
                    has_cover: p.cover.is_some(),
                    percent: prog.map_or(0.0, |p| p.percent),
                    last_read: prog.map_or(0, |p| p.updated),
                    file: name,
                })
            })
            .collect();
        // 刪掉的書不必留在快取
        self.cache.lock().unwrap().retain(|p, _| p.exists());
        books.sort_by_key(|b| b.title.to_lowercase());
        books
    }

    pub fn content(&self, dir: &Path, file: &str) -> Option<(String, String, Content)> {
        self.get(dir, file).map(|p| (p.title, p.author, p.content))
    }

    /// 封面: (zip 裡的路徑, 是不是漫畫)
    pub fn cover(&self, dir: &Path, file: &str) -> Option<String> {
        self.get(dir, file)?.cover
    }

    pub fn txt_chapter_html(&self, dir: &Path, file: &str, n: usize) -> Option<String> {
        let p = self.get(dir, file)?;
        let book = p.txt?;
        book.chapters.get(n).map(|c| txt::chapter_html(&p.title, c))
    }

    pub fn comic_page(&self, dir: &Path, file: &str, n: usize) -> Option<String> {
        self.get(dir, file)?.pages.get(n).cloned()
    }

    // ------------------------------------------------------------ MOBI 轉檔

    /// 需要轉檔的 MOBI(沒有轉出的 EPUB, 也還沒在轉)
    pub fn pending_conversions(&self, dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let converting = self.converting.lock().unwrap();
        entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| is_safe_name(n) && kind_of(n) == Some(Kind::Mobi))
            .filter(|n| !Self::converted_path(dir, n).is_file() && !converting.contains_key(n))
            .collect()
    }

    /// 用 Calibre 轉成 EPUB(會花一點時間, 在背景 thread 執行)
    pub fn convert(&self, dir: &Path, file: &str) -> Result<(), String> {
        self.converting
            .lock()
            .unwrap()
            .insert(file.to_string(), None);
        let out = Self::converted_path(dir, file);
        let result = (|| {
            std::fs::create_dir_all(dir.join(CACHE_DIR)).map_err(|e| e.to_string())?;
            let tmp = out.with_extension("tmp.epub");
            let status = std::process::Command::new("nice")
                .args(["-n", "10", "ebook-convert"])
                .arg(dir.join(file))
                .arg(&tmp)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("無法執行 ebook-convert(請安裝 Calibre): {e}"))?;
            if !status.success() {
                let _ = std::fs::remove_file(&tmp);
                return Err("Calibre 轉檔失敗(檔案可能有 DRM 保護)".to_string());
            }
            std::fs::rename(&tmp, &out).map_err(|e| e.to_string())
        })();
        let mut converting = self.converting.lock().unwrap();
        match &result {
            Ok(()) => {
                converting.remove(file);
            }
            Err(e) => {
                converting.insert(file.to_string(), Some(e.clone()));
            }
        }
        result
    }

    // ------------------------------------------------------------ 進度

    fn load_progress(&self, dir: &Path) -> BTreeMap<String, Progress> {
        std::fs::read_to_string(dir.join(PROGRESS_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn progress(&self, dir: &Path, file: &str) -> Progress {
        self.load_progress(dir).remove(file).unwrap_or_default()
    }

    pub fn save_progress(&self, dir: &Path, file: &str, p: Progress) -> Result<(), String> {
        let _guard = self.progress.lock().unwrap();
        let mut all = self.load_progress(dir);
        all.insert(file.to_string(), p);
        // 先寫暫存檔再改名, 寫到一半不會壞掉
        let tmp = dir.join(format!("{PROGRESS_FILE}.tmp"));
        let json = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(PROGRESS_FILE)).map_err(|e| e.to_string())
    }

    /// 刪除書時一併移除進度與轉檔
    pub fn forget(&self, dir: &Path, file: &str) {
        let _guard = self.progress.lock().unwrap();
        let mut all = self.load_progress(dir);
        if all.remove(file).is_some()
            && let Ok(json) = serde_json::to_string_pretty(&all)
        {
            let _ = std::fs::write(dir.join(PROGRESS_FILE), json);
        }
        let _ = std::fs::remove_file(Self::converted_path(dir, file));
        self.cache.lock().unwrap().remove(&dir.join(file));
    }
}

/// 閱讀進度與書籤(存在 Pi 上, 各裝置同步)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    /// 第幾章(漫畫/PDF 為第幾頁)
    #[serde(default)]
    pub chapter: usize,
    /// 這一章讀到的位置(0~1)
    #[serde(default)]
    pub pos: f64,
    /// 整本書讀到的位置(0~1)
    #[serde(default)]
    pub percent: f64,
    /// 最後閱讀時間(Unix 秒)
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub chapter: usize,
    pub pos: f64,
    pub percent: f64,
    /// 書籤名稱(預設是章節標題)
    pub label: String,
    pub created: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_sort() {
        assert_eq!(kind_of("a.EPUB"), Some(Kind::Epub));
        assert_eq!(kind_of("a.azw3"), Some(Kind::Mobi));
        assert_eq!(kind_of("a.cbz"), Some(Kind::Comic));
        assert_eq!(kind_of("a.doc"), None);
        let mut v = vec!["p10.jpg", "p2.jpg", "p1.jpg", "cover.jpg"];
        v.sort_by_key(|n| natural_key(n));
        assert_eq!(v, ["cover.jpg", "p1.jpg", "p2.jpg", "p10.jpg"]);
    }
}
