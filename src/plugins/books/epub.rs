//! EPUB 解析: 書本資訊、章節順序(spine)、目錄(nav 或 NCX)、封面。
//!
//! EPUB 是一個 zip: `META-INF/container.xml` 指向 OPF, OPF 裡有書本資訊、所有檔案
//! (manifest)與閱讀順序(spine)。章節內容(XHTML、圖片、CSS)由網頁直接向伺服器要。

use std::io::Read;
use std::path::Path;

use roxmltree::{Document, Node};
use serde::Serialize;

/// 目錄的一個項目
#[derive(Debug, Clone, Serialize)]
pub struct TocItem {
    pub title: String,
    /// zip 裡的完整路徑, 可能帶 #錨點
    pub href: String,
    /// 層級(0 = 最上層)
    pub level: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EpubInfo {
    pub title: String,
    pub author: String,
    pub language: String,
    /// "rtl" 表示由右往左翻(直排書); 否則 "ltr"
    pub direction: String,
    /// 依閱讀順序的章節(zip 裡的完整路徑)
    pub chapters: Vec<String>,
    pub toc: Vec<TocItem>,
    /// 封面圖片在 zip 裡的路徑
    pub cover: Option<String>,
}

pub fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, String> {
    let mut f = zip
        .by_name(name)
        .map_err(|e| format!("找不到 {name}: {e}"))?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf)
        .map_err(|e| format!("無法讀取 {name}: {e}"))?;
    Ok(buf)
}

fn read_text<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String, String> {
    let bytes = read_entry(zip, name)?;
    // 去掉 BOM
    let s = String::from_utf8_lossy(&bytes);
    Ok(s.trim_start_matches('\u{feff}').to_string())
}

/// 解開 %XX 編碼(href 裡可能有 %20 之類)
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 相對路徑接在 base 目錄後面, 並處理 ./ 與 ../
pub fn resolve(base_dir: &str, href: &str) -> String {
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        base_dir.split('/').filter(|p| !p.is_empty()).collect()
    };
    for p in href.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    parts.join("/")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: Node) -> String {
    n.descendants()
        .filter(|d| d.is_text())
        .filter_map(|d| d.text())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse(path: &Path) -> Result<EpubInfo, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("無法開啟: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("不是有效的 EPUB: {e}"))?;
    parse_zip(&mut zip)
}

pub fn parse_zip<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<EpubInfo, String> {
    // container.xml → OPF
    let container = read_text(zip, "META-INF/container.xml")?;
    let doc = Document::parse(&container).map_err(|e| format!("container.xml 格式錯誤: {e}"))?;
    let opf_path = doc
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .ok_or("container.xml 沒有指定 OPF")?
        .to_string();
    let opf_dir = dir_of(&opf_path).to_string();
    let opf = read_text(zip, &opf_path)?;
    let doc = Document::parse(&opf).map_err(|e| format!("OPF 格式錯誤: {e}"))?;
    let root = doc.root_element();

    let mut info = EpubInfo {
        direction: "ltr".into(),
        ..Default::default()
    };

    // 書本資訊
    let mut cover_id = None;
    if let Some(meta) = child(root, "metadata") {
        for n in meta.children().filter(|n| n.is_element()) {
            match n.tag_name().name() {
                "title" if info.title.is_empty() => info.title = text_of(n),
                "creator" if info.author.is_empty() => info.author = text_of(n),
                "language" if info.language.is_empty() => info.language = text_of(n),
                // EPUB 2: <meta name="cover" content="id"/>
                "meta" if n.attribute("name") == Some("cover") => {
                    cover_id = n.attribute("content").map(str::to_string);
                }
                _ => {}
            }
        }
    }

    // manifest: id → (完整路徑, media-type, properties)
    let mut manifest = Vec::new();
    if let Some(m) = child(root, "manifest") {
        for item in m.children().filter(|n| n.tag_name().name() == "item") {
            let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) else {
                continue;
            };
            manifest.push((
                id.to_string(),
                resolve(&opf_dir, href),
                item.attribute("media-type").unwrap_or("").to_string(),
                item.attribute("properties").unwrap_or("").to_string(),
            ));
        }
    }
    let by_id = |id: &str| manifest.iter().find(|m| m.0 == id);

    // 封面: EPUB 3 的 cover-image, 或 EPUB 2 的 meta cover
    info.cover = manifest
        .iter()
        .find(|m| m.3.split_whitespace().any(|p| p == "cover-image"))
        .or_else(|| cover_id.as_deref().and_then(by_id))
        .filter(|m| m.2.starts_with("image/"))
        .map(|m| m.1.clone());

    // spine: 閱讀順序
    let mut ncx_id = None;
    if let Some(spine) = child(root, "spine") {
        if spine.attribute("page-progression-direction") == Some("rtl") {
            info.direction = "rtl".into();
        }
        ncx_id = spine.attribute("toc").map(str::to_string);
        for r in spine
            .children()
            .filter(|n| n.tag_name().name() == "itemref")
        {
            if let Some(m) = r.attribute("idref").and_then(by_id) {
                info.chapters.push(m.1.clone());
            }
        }
    }
    if info.chapters.is_empty() {
        return Err("EPUB 沒有任何章節".into());
    }

    // 目錄: EPUB 3 nav 優先, 否則 NCX
    let nav = manifest
        .iter()
        .find(|m| m.3.split_whitespace().any(|p| p == "nav"))
        .map(|m| m.1.clone());
    if let Some(nav) = nav
        && let Ok(text) = read_text(zip, &nav)
    {
        info.toc = parse_nav(&text, dir_of(&nav));
    }
    if info.toc.is_empty() {
        let ncx = ncx_id
            .as_deref()
            .and_then(by_id)
            .or_else(|| manifest.iter().find(|m| m.2 == "application/x-dtbncx+xml"))
            .map(|m| m.1.clone());
        if let Some(ncx) = ncx
            && let Ok(text) = read_text(zip, &ncx)
        {
            info.toc = parse_ncx(&text, dir_of(&ncx));
        }
    }
    Ok(info)
}

/// EPUB 3 nav 文件: <nav epub:type="toc"><ol><li><a href>…
fn parse_nav(text: &str, base: &str) -> Vec<TocItem> {
    let Ok(doc) = Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    ) else {
        return Vec::new();
    };
    let nav = doc.descendants().find(|n| {
        n.tag_name().name() == "nav"
            && n.attributes()
                .any(|a| a.name() == "type" && a.value().split_whitespace().any(|v| v == "toc"))
    });
    let Some(nav) = nav.or_else(|| doc.descendants().find(|n| n.tag_name().name() == "nav")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(n: Node, level: usize, base: &str, out: &mut Vec<TocItem>) {
        for li in n.children().filter(|c| c.tag_name().name() == "li") {
            if let Some(a) = li.children().find(|c| c.tag_name().name() == "a")
                && let Some(href) = a.attribute("href")
            {
                out.push(TocItem {
                    title: text_of(a),
                    href: resolve_href(base, href),
                    level,
                });
            }
            if let Some(ol) = li.children().find(|c| c.tag_name().name() == "ol") {
                walk(ol, level + 1, base, out);
            }
        }
    }
    if let Some(ol) = nav.children().find(|c| c.tag_name().name() == "ol") {
        walk(ol, 0, base, &mut out);
    }
    out
}

/// EPUB 2 NCX: <navMap><navPoint><navLabel><text>… <content src>
fn parse_ncx(text: &str, base: &str) -> Vec<TocItem> {
    let Ok(doc) = Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    ) else {
        return Vec::new();
    };
    let Some(map) = doc.descendants().find(|n| n.tag_name().name() == "navMap") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(n: Node, level: usize, base: &str, out: &mut Vec<TocItem>) {
        for p in n.children().filter(|c| c.tag_name().name() == "navPoint") {
            let title = child(p, "navLabel").map(text_of).unwrap_or_default();
            if let Some(src) = child(p, "content").and_then(|c| c.attribute("src")) {
                out.push(TocItem {
                    title,
                    href: resolve_href(base, src),
                    level,
                });
            }
            walk(p, level + 1, base, out);
        }
    }
    walk(map, 0, base, &mut out);
    out
}

/// href 可能帶 #錨點: 路徑部分要 resolve, 錨點保留
fn resolve_href(base: &str, href: &str) -> String {
    match href.split_once('#') {
        Some((p, frag)) if !p.is_empty() => format!("{}#{frag}", resolve(base, p)),
        Some((_, frag)) => format!("#{frag}"),
        None => resolve(base, href),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(resolve("OEBPS", "Text/ch1.xhtml"), "OEBPS/Text/ch1.xhtml");
        assert_eq!(
            resolve("OEBPS/Text", "../Images/a%20b.jpg"),
            "OEBPS/Images/a b.jpg"
        );
        assert_eq!(resolve("", "ch1.xhtml"), "ch1.xhtml");
        assert_eq!(
            resolve_href("OEBPS", "Text/c.xhtml#p3"),
            "OEBPS/Text/c.xhtml#p3"
        );
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
    }

    #[test]
    fn build_and_parse() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            let mut add = |name: &str, body: &str| {
                w.start_file(name, o).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            };
            add("mimetype", "application/epub+zip");
            add(
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
            );
            add(
                "OEBPS/content.opf",
                r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>測試書</dc:title><dc:creator>作者</dc:creator><meta name="cover" content="img"/></metadata>
                <manifest><item id="c1" href="Text/c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="Text/c2.xhtml" media-type="application/xhtml+xml"/><item id="img" href="Images/cover.jpg" media-type="image/jpeg"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest>
                <spine toc="ncx" page-progression-direction="rtl"><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#,
            );
            add(
                "OEBPS/toc.ncx",
                r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>第一章</text></navLabel><content src="Text/c1.xhtml"/><navPoint><navLabel><text>1-1</text></navLabel><content src="Text/c1.xhtml#s1"/></navPoint></navPoint><navPoint><navLabel><text>第二章</text></navLabel><content src="Text/c2.xhtml"/></navPoint></navMap></ncx>"#,
            );
            w.finish().unwrap();
        }
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(buf.into_inner())).unwrap();
        let info = parse_zip(&mut zip).unwrap();
        assert_eq!(info.title, "測試書");
        assert_eq!(info.author, "作者");
        assert_eq!(info.direction, "rtl");
        assert_eq!(
            info.chapters,
            ["OEBPS/Text/c1.xhtml", "OEBPS/Text/c2.xhtml"]
        );
        assert_eq!(info.cover.as_deref(), Some("OEBPS/Images/cover.jpg"));
        let toc: Vec<_> = info
            .toc
            .iter()
            .map(|t| (t.title.as_str(), t.level))
            .collect();
        assert_eq!(toc, [("第一章", 0), ("1-1", 1), ("第二章", 0)]);
        assert_eq!(info.toc[1].href, "OEBPS/Text/c1.xhtml#s1");
    }
}
