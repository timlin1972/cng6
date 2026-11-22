//! Books plugin: 電子書書庫與閱讀器(在網頁上閱讀)。
//!
//! 支援 EPUB、MOBI / AZW / AZW3(用 Calibre 轉成 EPUB)、TXT、PDF、CBZ / ZIP 漫畫。
//! 書放在 `books.dir`(預設 books/), 閱讀進度與書籤存在書庫裡, 各裝置同步。

pub mod epub;
pub mod library;
pub mod txt;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::plugin::{Ctx, Plugin, complete_from_help};
use crate::webui::{Card, WebMeta};
use library::{Book, Kind, Library};

const DEFAULT_DIR: &str = "books";

struct Inner {
    dir: RwLock<PathBuf>,
    lib: Library,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone)]
pub struct Shared(Arc<Inner>);

impl Shared {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            dir: RwLock::new(DEFAULT_DIR.into()),
            lib: Library::default(),
        }))
    }

    pub fn dir(&self) -> PathBuf {
        self.0.dir.read().unwrap().clone()
    }

    pub fn lib(&self) -> &Library {
        &self.0.lib
    }

    pub fn books(&self) -> Vec<Book> {
        self.0.lib.scan(&self.dir())
    }
}

pub struct Books {
    shared: Shared,
    /// 背景轉檔的結果: (檔名, 錯誤)
    converted: Receiver<(String, Option<String>)>,
}

impl Books {
    pub fn new(shared: Shared) -> Self {
        let (tx, rx): (Sender<(String, Option<String>)>, _) = mpsc::channel();
        // 背景: 定時檢查需要轉檔的 MOBI / AZW3, 一本一本轉
        let worker = shared.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            loop {
                let dir = worker.dir();
                for file in worker.lib().pending_conversions(&dir) {
                    let err = worker.lib().convert(&dir, &file).err();
                    if tx.send((file, err)).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(30));
            }
        });
        Self {
            shared,
            converted: rx,
        }
    }

    fn names(&self) -> Vec<(String, String)> {
        self.shared
            .books()
            .into_iter()
            .map(|b| (b.file, b.title))
            .collect()
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Epub => "EPUB",
        Kind::Mobi => "MOBI",
        Kind::Txt => "TXT",
        Kind::Pdf => "PDF",
        Kind::Comic => "漫畫",
    }
}

impl Plugin for Books {
    fn name(&self) -> &str {
        "books"
    }

    fn description(&self) -> &str {
        "電子書: EPUB / MOBI / TXT / PDF / 漫畫"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("list", "列出書庫(含閱讀進度)"),
            ("info <file>", "書本資訊與章節數"),
            ("delete <file>", "刪除書(含閱讀進度與書籤)"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["info" | "delete"] => self.names(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![("dir", "存放電子書的資料夾", DEFAULT_DIR.into())]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dir" => {
                *self.shared.0.dir.write().unwrap() = PathBuf::from(value);
                Ok(())
            }
            _ => Err(format!("不支援的設定 '{key}'")),
        }
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        let dir = self.shared.dir();
        match args {
            ["list"] => {
                let books = self.shared.books();
                for b in &books {
                    let author = if b.author.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", b.author)
                    };
                    let read = if b.percent > 0.0 {
                        format!("  讀到 {:.0}%", b.percent * 100.0)
                    } else {
                        String::new()
                    };
                    ctx.info(format!(
                        "  [{}] {}{author}{read}",
                        kind_name(b.kind),
                        b.title
                    ));
                }
                ctx.info(format!("  共 {} 本 ({})", books.len(), dir.display()));
            }
            ["info", file] => match self.shared.lib().content(&dir, file) {
                Some((title, author, content)) => {
                    ctx.info(format!(
                        "{title}{}",
                        if author.is_empty() {
                            String::new()
                        } else {
                            format!(" — {author}")
                        }
                    ));
                    let detail = match content {
                        library::Content::Epub {
                            direction,
                            chapters,
                            toc,
                        } => format!(
                            "EPUB, {} 章, 目錄 {} 項{}",
                            chapters.len(),
                            toc.len(),
                            if direction == "rtl" {
                                ", 直排(由右往左)"
                            } else {
                                ""
                            }
                        ),
                        library::Content::Txt { encoding, chapters } => {
                            format!("TXT ({encoding}), {} 章", chapters.len())
                        }
                        library::Content::Pdf => "PDF".into(),
                        library::Content::Comic { pages } => format!("漫畫, {pages} 頁"),
                        library::Content::Converting { error: None } => "MOBI 轉檔中…".into(),
                        library::Content::Converting { error: Some(e) } => format!("無法讀取: {e}"),
                    };
                    ctx.info(format!("  {detail}"));
                    let p = self.shared.lib().progress(&dir, file);
                    if p.updated > 0 {
                        ctx.info(format!(
                            "  讀到 {:.0}%, 書籤 {} 個",
                            p.percent * 100.0,
                            p.bookmarks.len()
                        ));
                    }
                }
                None => ctx.error(format!("沒有書 '{file}'")),
            },
            ["delete", file] => {
                if !library::is_safe_name(file)
                    || library::kind_of(file).is_none()
                    || !dir.join(file).is_file()
                {
                    ctx.error(format!("沒有書 '{file}'"));
                    return;
                }
                match std::fs::remove_file(dir.join(file)) {
                    Ok(()) => {
                        self.shared.lib().forget(&dir, file);
                        ctx.info(format!("已刪除 {file}(含閱讀進度與書籤)"));
                    }
                    Err(e) => ctx.error(format!("無法刪除 {file}: {e}")),
                }
            }
            _ => ctx.error("用法: 輸入 'help books' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        while let Ok((file, err)) = self.converted.try_recv() {
            match err {
                None => ctx.info(format!("books: {file} 轉檔完成, 可以閱讀了")),
                Some(e) => ctx.error(format!("books: {file} 轉檔失敗: {e}")),
            }
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("書", "book", "orange"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("books")
    }

    fn web_card(&self) -> Option<Card> {
        let books = self.shared.books();
        let recent = books
            .iter()
            .filter(|b| b.last_read > 0)
            .max_by_key(|b| b.last_read);
        let summary = match recent {
            Some(b) => format!("最近在讀: {} ({:.0}%)", b.title, b.percent * 100.0),
            None => format!("{} 本書", books.len()),
        };
        let mut card = Card::new()
            .summary(summary)
            .table([("書", format!("{} 本", books.len()))]);
        if let Some(b) = recent {
            card = card.meter(
                b.title.clone(),
                b.percent,
                format!("{:.0}%", b.percent * 100.0),
                None,
            );
        }
        Some(card)
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}
