//! 資源回收桶, 照 freedesktop.org Trash 規格, 和桌面環境(檔案管理員)共用。
//!
//! - 和家目錄同一個磁碟: `~/.local/share/Trash/{files,info}`
//! - 其他磁碟(USB 隨身碟): `<掛載點>/.Trash-<uid>/{files,info}`
//!
//! 每個丟進來的項目在 info/ 有一個 `<名稱>.trashinfo`, 記錄原本的位置與刪除時間。

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::fsops;

fn uid() -> u32 {
    // /proc/self 的擁有者就是自己
    fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(1000)
}

fn home_trash() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| fsops::home().join(".local/share"))
        .join("Trash")
}

fn dev_of(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| m.dev())
}

/// 往上找到同一個磁碟的最上層(掛載點)
fn mount_point(p: &Path) -> PathBuf {
    let Ok(mut cur) = p.canonicalize() else {
        return PathBuf::from("/");
    };
    let dev = dev_of(&cur);
    while let Some(parent) = cur.parent() {
        if dev_of(parent) != dev {
            break;
        }
        cur = parent.to_path_buf();
    }
    cur
}

/// `path` 要丟到哪一個回收桶
fn trash_for(path: &Path) -> PathBuf {
    let home = home_trash();
    let home_dev = dev_of(&fsops::home());
    let dev = path.parent().and_then(dev_of);
    if dev.is_some() && dev == home_dev {
        home
    } else {
        mount_point(path.parent().unwrap_or(Path::new("/"))).join(format!(".Trash-{}", uid()))
    }
}

/// 所有可能用到的回收桶(依根目錄所在的磁碟)
pub fn trash_dirs(roots: &[fsops::Root]) -> Vec<PathBuf> {
    let mut out = vec![home_trash()];
    for r in roots {
        let t = trash_for(&r.path.join("x"));
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

fn ensure_dirs(trash: &Path) -> io::Result<()> {
    for sub in ["files", "info"] {
        let d = trash.join(sub);
        if !d.is_dir() {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&d)?;
        }
    }
    Ok(())
}

/// Path 欄位用的百分比編碼(保留 / 與常見的安全字元, 中文會編碼)
fn encode(path: &str) -> String {
    let mut s = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 丟進回收桶
pub fn trash(path: &Path) -> Result<(), String> {
    let abs = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .zip(path.file_name())
        .map(|(p, n)| p.join(n))
        .ok_or("路徑不合法")?;
    trash_to(&abs, &trash_for(&abs))
}

fn trash_to(abs: &Path, trash: &Path) -> Result<(), String> {
    ensure_dirs(trash).map_err(|e| format!("無法建立回收桶 {}: {e}", trash.display()))?;
    let base = abs.file_name().unwrap().to_string_lossy().into_owned();
    // files/ 與 info/ 都不能重複
    let mut name = base.clone();
    let mut i = 1;
    while fs::symlink_metadata(trash.join("files").join(&name)).is_ok()
        || trash.join("info").join(format!("{name}.trashinfo")).exists()
    {
        i += 1;
        name = format!("{base}.{i}");
    }
    let info = trash.join("info").join(format!("{name}.trashinfo"));
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    fs::write(
        &info,
        format!(
            "[Trash Info]\nPath={}\nDeletionDate={date}\n",
            encode(&abs.to_string_lossy())
        ),
    )
    .map_err(|e| format!("無法寫入回收桶: {e}"))?;
    if let Err(e) = fsops::move_path(abs, &trash.join("files").join(&name)) {
        let _ = fs::remove_file(&info);
        return Err(format!("無法移到回收桶: {e}"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// 第幾個回收桶
    pub trash: usize,
    /// 在回收桶裡的名稱
    pub id: String,
    /// 原本的名稱
    pub name: String,
    /// 原本的位置(含名稱)
    pub path: String,
    /// 刪除時間 `YYYY-MM-DDThh:mm:ss`
    pub deleted: String,
    pub dir: bool,
    pub size: u64,
}

fn read_info(info: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(info).ok()?;
    let mut path = None;
    let mut date = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Path=") {
            path = Some(decode(v.trim()));
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            date = v.trim().to_string();
        }
    }
    Some((path?, date))
}

/// 回收桶裡、原本位置在 `roots` 裡面的項目(新的在前)
pub fn list(roots: &[fsops::Root]) -> Vec<Item> {
    let dirs = trash_dirs(roots);
    let mut items = Vec::new();
    for (ti, t) in dirs.iter().enumerate() {
        let Ok(rd) = fs::read_dir(t.join("info")) else {
            continue;
        };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(id) = fname.strip_suffix(".trashinfo") else {
                continue;
            };
            let Some((mut path, deleted)) = read_info(&e.path()) else {
                continue;
            };
            // 規格允許相對於回收桶所在磁碟的路徑
            if !path.starts_with('/') {
                path = t.parent().unwrap_or(Path::new("/")).join(&path).to_string_lossy().into_owned();
            }
            if !roots.iter().any(|r| Path::new(&path).starts_with(&r.path)) {
                continue;
            }
            let Ok(md) = fs::symlink_metadata(t.join("files").join(id)) else {
                continue;
            };
            items.push(Item {
                trash: ti,
                id: id.to_string(),
                name: Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| id.to_string()),
                path,
                deleted,
                dir: md.is_dir(),
                size: if md.is_dir() { 0 } else { md.len() },
            });
        }
    }
    items.sort_by(|a, b| b.deleted.cmp(&a.deleted));
    items
}

fn locate(roots: &[fsops::Root], trash: usize, id: &str) -> Result<(PathBuf, PathBuf), String> {
    if !fsops::valid_name(id) {
        return Err("名稱不合法".into());
    }
    let t = trash_dirs(roots).into_iter().nth(trash).ok_or("沒有這個回收桶")?;
    let file = t.join("files").join(id);
    let info = t.join("info").join(format!("{id}.trashinfo"));
    if fs::symlink_metadata(&file).is_err() {
        return Err("回收桶裡沒有這個項目".into());
    }
    Ok((file, info))
}

/// 還原到原本的位置(同名時自動改名), 回傳還原後的路徑
pub fn restore(roots: &[fsops::Root], trash: usize, id: &str) -> Result<PathBuf, String> {
    let (file, info) = locate(roots, trash, id)?;
    restore_from(roots, &file, &info)
}

fn restore_from(roots: &[fsops::Root], file: &Path, info: &Path) -> Result<PathBuf, String> {
    let (orig, _) = read_info(info).ok_or("找不到原本的位置")?;
    let orig = PathBuf::from(orig);
    if !roots.iter().any(|r| orig.starts_with(&r.path)) {
        return Err("原本的位置不在可以管理的範圍".into());
    }
    let parent = orig.parent().ok_or("原本的位置不合法")?;
    fs::create_dir_all(parent).map_err(|e| format!("無法建立 {}: {e}", parent.display()))?;
    let name = orig.file_name().ok_or("原本的位置不合法")?.to_string_lossy();
    let dst = parent.join(fsops::unique_name(parent, &name));
    fsops::move_path(file, &dst).map_err(|e| format!("無法還原: {e}"))?;
    let _ = fs::remove_file(info);
    Ok(dst)
}

/// 永久刪除
pub fn purge(roots: &[fsops::Root], trash: usize, id: &str) -> Result<(), String> {
    let (file, info) = locate(roots, trash, id)?;
    fsops::remove_all(&file).map_err(|e| format!("無法刪除: {e}"))?;
    let _ = fs::remove_file(&info);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent() {
        let p = "/home/a b/中文#.txt";
        assert_eq!(encode(p), "/home/a%20b/%E4%B8%AD%E6%96%87%23.txt");
        assert_eq!(decode(&encode(p)), p);
        assert_eq!(decode("100%"), "100%");
    }

    #[test]
    fn trash_and_restore() {
        // 用暫存的回收桶, 不碰真正的
        let d = std::env::temp_dir().join(format!("cng6-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("root/sub")).unwrap();
        fs::write(d.join("root/sub/a.txt"), "x").unwrap();
        let roots = [fsops::Root {
            name: "r".into(),
            path: d.join("root"),
        }];
        let t = d.join("trash");
        trash_to(&d.join("root/sub/a.txt"), &t).unwrap();
        assert!(!d.join("root/sub/a.txt").exists());
        assert!(t.join("files/a.txt").exists());
        // 同名的再丟一次
        fs::write(d.join("root/sub/a.txt"), "y").unwrap();
        trash_to(&d.join("root/sub/a.txt"), &t).unwrap();
        assert!(t.join("files/a.txt.2").exists());
        let (orig, date) = read_info(&t.join("info/a.txt.trashinfo")).unwrap();
        assert_eq!(PathBuf::from(orig), d.join("root/sub/a.txt"));
        assert_eq!(date.len(), 19);
        // 還原: 原本位置已經有同名檔案時自動改名
        fs::write(d.join("root/sub/a.txt"), "z").unwrap();
        let back = restore_from(&roots, &t.join("files/a.txt"), &t.join("info/a.txt.trashinfo")).unwrap();
        assert_eq!(back, d.join("root/sub/a (2).txt"));
        assert_eq!(fs::read_to_string(back).unwrap(), "x");
        assert!(!t.join("info/a.txt.trashinfo").exists());
        // 範圍以外的位置不能還原
        let other = [fsops::Root {
            name: "o".into(),
            path: d.join("elsewhere"),
        }];
        assert!(restore_from(&other, &t.join("files/a.txt.2"), &t.join("info/a.txt.2.trashinfo")).is_err());
        fs::remove_dir_all(&d).unwrap();
    }
}
