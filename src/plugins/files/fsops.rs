//! 檔案操作: 根目錄、路徑檢查、列出、複製、搬移、zip 壓縮與解壓縮。
//!
//! 網頁傳來的路徑一律是「第幾個根目錄 + 相對路徑」, 由 [`resolve`] 檢查不會跳出根目錄
//! (`..`、指向外面的捷徑都不行)。

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// 一個可以管理的根目錄
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Root {
    pub name: String,
    pub path: PathBuf,
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// 解析設定: `名稱=路徑; 名稱=路徑`, 只寫路徑時用資料夾名稱
pub fn parse_roots(s: &str) -> Result<Vec<Root>, String> {
    let mut roots = Vec::new();
    for part in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, path) = match part.split_once('=') {
            Some((n, p)) => (n.trim().to_string(), expand(p.trim())),
            None => {
                let path = expand(part);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                (name, path)
            }
        };
        if !path.is_absolute() {
            return Err(format!("'{}' 不是絕對路徑", path.display()));
        }
        if name.is_empty() {
            return Err(format!("'{}' 沒有名稱", path.display()));
        }
        roots.push(Root { name, path });
    }
    if roots.is_empty() {
        return Err("至少要有一個根目錄".into());
    }
    Ok(roots)
}

/// 檔名可不可以用(新增、改名)
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && !name.contains(['/', '\0'])
}

/// 相對路徑 → 實際路徑, 確認在根目錄裡面。`must_exist = false` 用在要建立的新檔案。
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("根目錄 {} 無法使用: {e}", root.display()))?;
    let mut path = root.clone();
    for c in Path::new(rel.trim_start_matches('/')).components() {
        match c {
            Component::Normal(n) => path.push(n),
            Component::CurDir => {}
            _ => return Err("路徑不合法".into()),
        }
    }
    // 存在的路徑: 捷徑(symlink)展開後也必須在根目錄裡
    match path.canonicalize() {
        Ok(real) if real.starts_with(&root) => Ok(path),
        Ok(_) => Err("不能存取根目錄以外的位置".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err("找不到這個檔案或資料夾".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// 在資料夾 `dir`(已經 resolve 過)裡的新名稱
pub fn child(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!("檔名 '{name}' 不合法"));
    }
    Ok(dir.join(name))
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// 修改時間(秒)
    pub mtime: i64,
    /// 是捷徑(symlink)
    pub link: bool,
}

fn mtime_of(md: &fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    md.mtime()
}

pub fn list(dir: &Path) -> Result<Vec<Entry>, String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("無法讀取資料夾: {e}"))?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        // 捷徑看它指到的東西; 壞掉的捷徑當成大小 0 的檔案
        let md = fs::metadata(e.path()).or_else(|_| fs::symlink_metadata(e.path()));
        let (dir, size, mtime) = match md {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, mtime_of(&m)),
            Err(_) => (false, 0, 0),
        };
        out.push(Entry {
            name,
            dir,
            size,
            mtime,
            link,
        });
    }
    Ok(out)
}

/// 名稱重複時: `a.txt` → `a (2).txt`, 資料夾 `x` → `x (2)`
pub fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() && fs::symlink_metadata(dir.join(name)).is_err() {
        return name.to_string();
    }
    let is_dir = dir.join(name).is_dir();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !is_dir && !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|i| format!("{stem} ({i}){ext}"))
        .find(|n| fs::symlink_metadata(dir.join(n)).is_err())
        .unwrap()
}

/// 複製檔案或整個資料夾(捷徑複製成捷徑)
pub fn copy_all(src: &Path, dst: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(src)?;
    if md.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)
    } else if md.is_dir() {
        fs::create_dir(dst)?;
        fs::set_permissions(dst, md.permissions())?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_all(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

pub fn remove_all(p: &Path) -> io::Result<()> {
    if fs::symlink_metadata(p)?.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// 搬移: 同一個磁碟直接改名, 不同磁碟先複製再刪除
pub fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        // EXDEV: 跨磁碟
        Err(e) if e.raw_os_error() == Some(18) => {
            copy_all(src, dst)?;
            remove_all(src)
        }
        Err(e) => Err(e),
    }
}

/// 把資料夾搬/複製到自己裡面
pub fn is_inside(src: &Path, dst_dir: &Path) -> bool {
    match (src.canonicalize(), dst_dir.canonicalize()) {
        (Ok(s), Ok(d)) => d.starts_with(s),
        _ => false,
    }
}

// ---------------------------------------------------------------- zip

/// 已經壓縮過的格式直接存放, 不再壓縮(Pi 的 CPU 省一點)
fn already_compressed(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "avif" | "mp3" | "m4a" | "aac" | "ogg"
            | "opus" | "flac" | "mp4" | "m4v" | "mkv" | "mov" | "webm" | "avi" | "zip" | "gz"
            | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "epub" | "cbz" | "pdf" | "docx"
            | "xlsx" | "pptx" | "woff2"
    )
}

fn zip_time(md: &fs::Metadata) -> Option<zip::DateTime> {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let t = Local.timestamp_opt(mtime_of(md), 0).single()?;
    zip::DateTime::from_date_and_time(
        t.year().clamp(1980, 2107) as u16,
        t.month() as u8,
        t.day() as u8,
        t.hour() as u8,
        t.minute() as u8,
        t.second() as u8,
    )
    .ok()
}

/// 把 `items`(實際路徑)壓縮成 zip 寫到 `out`(不需要 Seek, 可以邊壓邊送)。
/// 資料夾會整個放進去, 捷徑略過。
pub fn write_zip<W: Write>(items: &[PathBuf], out: W) -> zip::result::ZipResult<()> {
    use zip::write::SimpleFileOptions;
    let mut zw = zip::ZipWriter::new_stream(out);
    let mut stack: Vec<(PathBuf, String)> = items
        .iter()
        .filter_map(|p| Some((p.clone(), p.file_name()?.to_string_lossy().into_owned())))
        .collect();
    stack.reverse();
    while let Some((path, name)) = stack.pop() {
        let Ok(md) = fs::symlink_metadata(&path) else {
            continue;
        };
        let mut opts = SimpleFileOptions::default();
        if let Some(t) = zip_time(&md) {
            opts = opts.last_modified_time(t);
        }
        if md.is_dir() {
            zw.add_directory(format!("{name}/"), opts)?;
            let mut children: Vec<_> = fs::read_dir(&path)
                .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
                .unwrap_or_default();
            children.sort();
            for c in children.into_iter().rev() {
                let child_name = format!("{name}/{}", c.to_string_lossy());
                stack.push((path.join(&c), child_name));
            }
        } else if md.is_file() {
            use std::os::unix::fs::PermissionsExt;
            let method = if already_compressed(&name) {
                zip::CompressionMethod::Stored
            } else {
                zip::CompressionMethod::Deflated
            };
            opts = opts
                .compression_method(method)
                .unix_permissions(md.permissions().mode() & 0o777)
                // 串流寫入沒辦法事後補 zip64 欄位, 大檔案要先講
                .large_file(md.len() >= 0xFFFF_0000);
            let Ok(mut f) = fs::File::open(&path) else {
                continue;
            };
            zw.start_file(name, opts)?;
            io::copy(&mut f, &mut zw)?;
        }
    }
    zw.finish()?;
    Ok(())
}

/// 解壓縮到 `dest`(會先建立)。路徑跳出 `dest` 的項目與捷徑一律略過。
/// 回傳解出的檔案數。
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<usize, String> {
    use std::os::unix::fs::PermissionsExt;
    let f = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(f)).map_err(|e| format!("不是有效的 zip: {e}"))?;
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut count = 0;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else {
            continue;
        };
        if e.is_symlink() {
            continue;
        }
        let out = dest.join(rel);
        if e.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut w = fs::File::create(&out).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = e.read(&mut buf).map_err(|e| format!("解壓縮失敗: {e}"))?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        if let Some(mode) = e.unix_mode() {
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o755 | 0o600));
        }
        count += 1;
    }
    Ok(count)
}

/// 資料夾裡的項目數與總大小(預覽資訊用, 最多看 `limit` 個項目)
pub fn dir_usage(dir: &Path, limit: usize) -> (u64, u64, bool) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > limit {
                return (files, bytes, true);
            }
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            } else {
                files += 1;
                bytes += md.len();
            }
        }
    }
    (files, bytes, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cng6-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roots() {
        let r = parse_roots("家=~; USB=/media/usb ;/srv/data").unwrap();
        assert_eq!(r[0].name, "家");
        assert_eq!(r[0].path, home());
        assert_eq!(r[1].path, PathBuf::from("/media/usb"));
        assert_eq!(r[2].name, "data");
        assert!(parse_roots("x=relative/path").is_err());
        assert!(parse_roots(" ; ").is_err());
    }

    #[test]
    fn resolve_stays_inside() {
        let d = tmp("resolve");
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink("/etc", d.join("out")).unwrap();
        std::os::unix::fs::symlink(d.join("a"), d.join("in")).unwrap();
        assert!(resolve(&d, "a/b/f.txt").is_ok());
        assert!(resolve(&d, "/a/b").is_ok());
        assert!(resolve(&d, "").is_ok());
        assert!(resolve(&d, "in/b/f.txt").is_ok());
        assert!(resolve(&d, "../etc").is_err());
        assert!(resolve(&d, "a/../../etc").is_err());
        assert!(resolve(&d, "out").is_err());
        assert!(resolve(&d, "out/passwd").is_err());
        assert!(resolve(&d, "nope").is_err());
        assert!(child(&d, "x/y").is_err());
        assert!(child(&d, "..").is_err());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_copy_move_zip() {
        let d = tmp("ops");
        fs::create_dir_all(d.join("src/sub")).unwrap();
        fs::write(d.join("src/a.txt"), "hello").unwrap();
        fs::write(d.join("src/sub/b.jpg"), [1u8, 2, 3]).unwrap();
        assert_eq!(unique_name(&d, "new.txt"), "new.txt");
        fs::write(d.join("new.txt"), "").unwrap();
        assert_eq!(unique_name(&d, "new.txt"), "new (2).txt");
        assert_eq!(unique_name(&d, "src"), "src (2)");

        copy_all(&d.join("src"), &d.join("copy")).unwrap();
        assert_eq!(fs::read_to_string(d.join("copy/a.txt")).unwrap(), "hello");
        assert!(is_inside(&d.join("src"), &d.join("src/sub")));
        assert!(!is_inside(&d.join("src"), &d.join("copy")));
        move_path(&d.join("copy"), &d.join("moved")).unwrap();
        assert!(!d.join("copy").exists());

        let mut buf = Vec::new();
        write_zip(&[d.join("src"), d.join("new.txt")], &mut buf).unwrap();
        fs::write(d.join("x.zip"), &buf).unwrap();
        assert_eq!(extract_zip(&d.join("x.zip"), &d.join("out")).unwrap(), 3);
        assert_eq!(fs::read_to_string(d.join("out/src/a.txt")).unwrap(), "hello");
        assert_eq!(fs::read(d.join("out/src/sub/b.jpg")).unwrap(), [1, 2, 3]);
        assert_eq!(dir_usage(&d.join("src"), 100), (2, 8, false));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn zip_slip_is_skipped() {
        let d = tmp("slip");
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            zw.start_file("../evil.txt", o).unwrap();
            zw.write_all(b"x").unwrap();
            zw.start_file("ok.txt", o).unwrap();
            zw.write_all(b"y").unwrap();
            zw.finish().unwrap();
        }
        fs::write(d.join("e.zip"), buf.into_inner()).unwrap();
        assert_eq!(extract_zip(&d.join("e.zip"), &d.join("out")).unwrap(), 1);
        assert!(!d.join("evil.txt").exists());
        fs::remove_dir_all(&d).unwrap();
    }
}
