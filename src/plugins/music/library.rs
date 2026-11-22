//! 音樂庫: 掃描資料夾、讀取曲目資訊、配對封面與歌詞、播放清單。
//!
//! 檔案命名(和 yt-dlp 下載的一致):
//! - 音樂: `<名稱>.mp3|m4a|opus|flac|ogg|wav|webm`
//! - 封面: `<名稱>.jpg|jpeg|png|webp`(沒有時用檔案內嵌的封面)
//! - 歌詞: `<名稱>.<語言>.srt|lrc|vtt` 或 `<名稱>.srt|lrc|vtt`
//! - 播放清單: `<清單名稱>.m3u8`, 一行一個檔名

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use lofty::prelude::*;
use serde::Serialize;

pub const AUDIO_EXT: &[&str] = &[
    "mp3", "m4a", "opus", "flac", "ogg", "oga", "wav", "webm", "aac",
];
const COVER_EXT: &[&str] = &["jpg", "jpeg", "png", "webp"];
const LYRICS_EXT: &[&str] = &["lrc", "srt", "vtt"];
pub const PLAYLIST_EXT: &str = "m3u8";

fn ext_of(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

pub fn is_audio(name: &str) -> bool {
    ext_of(name).is_some_and(|e| AUDIO_EXT.contains(&e.as_str()))
}

/// 檔名是否安全(不含路徑、不是隱藏檔)
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 400
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// 一首歌的歌詞檔
#[derive(Debug, Clone, Serialize)]
pub struct Lyrics {
    pub file: String,
    /// 語言代碼, 例如 zh-TW; 沒有標示時為空
    pub lang: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Track {
    /// 檔名(在資料夾中唯一, 當作識別)
    pub file: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 秒
    pub duration: f64,
    pub size: u64,
    /// 封面: 封面檔名; 只有內嵌封面時為 "embedded"; 沒有封面時為 None
    pub cover: Option<String>,
    pub lyrics: Vec<Lyrics>,
}

/// 從音樂檔讀出的資訊(讀檔比較慢, 依修改時間快取)
#[derive(Clone)]
struct Meta {
    mtime: SystemTime,
    size: u64,
    title: String,
    artist: String,
    album: String,
    duration: f64,
    embedded_cover: bool,
}

#[derive(Default)]
pub struct Cache(Mutex<HashMap<PathBuf, Meta>>);

fn read_meta(path: &Path, mtime: SystemTime, size: u64) -> Meta {
    let mut meta = Meta {
        mtime,
        size,
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration: 0.0,
        embedded_cover: false,
    };
    if let Ok(file) = lofty::read_from_path(path) {
        meta.duration = file.properties().duration().as_secs_f64();
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            meta.title = tag
                .title()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            meta.artist = tag
                .artist()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            meta.album = tag
                .album()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            meta.embedded_cover = !tag.pictures().is_empty();
        }
    }
    meta
}

impl Cache {
    /// 掃描資料夾, 回傳依檔名排序的曲目
    pub fn scan(&self, dir: &Path) -> Vec<Track> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut audio = Vec::new();
        let mut others: Vec<String> = Vec::new();
        for e in entries.flatten() {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            if !is_safe_name(&name) {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if !md.is_file() {
                continue;
            }
            if is_audio(&name) {
                audio.push((name, md));
            } else {
                others.push(name);
            }
        }

        // 封面與歌詞依「名稱」分組
        let mut covers: HashMap<&str, &str> = HashMap::new();
        let mut lyrics: HashMap<&str, Vec<Lyrics>> = HashMap::new();
        for name in &others {
            let Some(ext) = ext_of(name) else { continue };
            let base = stem(name);
            if COVER_EXT.contains(&ext.as_str()) {
                covers.entry(base).or_insert(name);
            } else if LYRICS_EXT.contains(&ext.as_str()) {
                // <名稱>.<語言>.srt 或 <名稱>.srt
                let (key, lang) = match base.rsplit_once('.') {
                    Some((k, l)) if is_lang(l) => (k, l.to_string()),
                    _ => (base, String::new()),
                };
                lyrics.entry(key).or_default().push(Lyrics {
                    file: name.clone(),
                    lang,
                });
            }
        }

        let mut cache = self.0.lock().unwrap();
        let mut tracks: Vec<Track> = audio
            .into_iter()
            .map(|(name, md)| {
                let path = dir.join(&name);
                let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let meta = match cache.get(&path) {
                    Some(m) if m.mtime == mtime && m.size == md.len() => m.clone(),
                    _ => {
                        let m = read_meta(&path, mtime, md.len());
                        cache.insert(path, m.clone());
                        m
                    }
                };
                let base = stem(&name);
                let cover = covers
                    .get(base)
                    .map(|c| c.to_string())
                    .or_else(|| meta.embedded_cover.then(|| "embedded".to_string()));
                let mut ly = lyrics.get(base).cloned().unwrap_or_default();
                ly.sort_by(|a, b| a.lang.cmp(&b.lang));
                Track {
                    title: if meta.title.is_empty() {
                        base.to_string()
                    } else {
                        meta.title.clone()
                    },
                    artist: meta.artist.clone(),
                    album: meta.album.clone(),
                    duration: meta.duration,
                    size: meta.size,
                    cover,
                    lyrics: ly,
                    file: name,
                }
            })
            .collect();
        // 刪掉的檔案不必留在快取
        cache.retain(|p, _| p.exists());
        tracks.sort_by_key(|t| t.file.to_lowercase());
        tracks
    }
}

/// 看起來像語言代碼: en、ja、zh-TW、zh-Hant、pt-BR…
fn is_lang(s: &str) -> bool {
    let mut parts = s.split('-');
    let first = parts.next().unwrap_or("");
    (2..=3).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// 可以修改的標籤欄位
pub const TAG_FIELDS: &[(&str, &str)] = &[("title", "歌名"), ("artist", "歌手"), ("album", "專輯")];

/// 修改音樂檔的標籤(歌名、歌手、專輯); value 是空字串時清除該欄位。
/// 其他標籤與內嵌封面保持不變。
pub fn set_tag(path: &Path, field: &str, value: &str) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::tag::Tag;

    let mut file = lofty::read_from_path(path).map_err(|e| format!("無法讀取標籤: {e}"))?;
    let primary = file.primary_tag_type();
    // 沒有主要標籤(例如只有 ID3v1 的 mp3): 先複製現有的標籤, 不然其他欄位會不見
    if file.primary_tag().is_none() {
        let mut tag = file
            .first_tag()
            .cloned()
            .unwrap_or_else(|| Tag::new(primary));
        tag.re_map(primary);
        file.insert_tag(tag);
    }
    let tag = file.primary_tag_mut().ok_or("這個格式不支援寫入標籤")?;
    let value = value.trim().to_string();
    match (field, value.is_empty()) {
        ("title", true) => tag.remove_title(),
        ("title", false) => tag.set_title(value),
        ("artist", true) => tag.remove_artist(),
        ("artist", false) => tag.set_artist(value),
        ("album", true) => tag.remove_album(),
        ("album", false) => tag.set_album(value),
        _ => {
            return Err(format!(
                "不能修改的欄位 '{field}', 可用 title / artist / album"
            ));
        }
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("無法寫入標籤: {e}"))
}

/// 內嵌封面 (資料, MIME)
pub fn embedded_cover(path: &Path) -> Option<(Vec<u8>, String)> {
    let file = lofty::read_from_path(path).ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pic = tag.pictures().first()?;
    let mime = pic
        .mime_type()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "image/jpeg".into());
    Some((pic.data().to_vec(), mime))
}

/// 刪除一首歌與它的封面、歌詞檔, 回傳刪除的檔案
pub fn delete_track(dir: &Path, file: &str) -> std::io::Result<Vec<String>> {
    std::fs::remove_file(dir.join(file))?;
    let mut removed = vec![file.to_string()];
    let base = stem(file);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            let Some(ext) = ext_of(&name) else { continue };
            let s = stem(&name);
            let is_side = (COVER_EXT.contains(&ext.as_str()) && s == base)
                || (LYRICS_EXT.contains(&ext.as_str())
                    && (s == base
                        || s.rsplit_once('.')
                            .is_some_and(|(k, l)| k == base && is_lang(l))));
            // 同名的其他音樂檔(不同格式)共用封面時就不要刪
            let shared = AUDIO_EXT
                .iter()
                .any(|a| dir.join(format!("{base}.{a}")).exists());
            if is_side && !shared && std::fs::remove_file(e.path()).is_ok() {
                removed.push(name);
            }
        }
    }
    Ok(removed)
}

// ---------------------------------------------------------------- 播放清單

/// 播放清單名稱是否合法(會成為檔名)
pub fn is_playlist_name(name: &str) -> bool {
    is_safe_name(name) && name.chars().count() <= 60 && !name.ends_with(' ')
}

/// 所有播放清單: 名稱 → 曲目檔名
pub fn playlists(dir: &Path) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if let Some(list) = name.strip_suffix(&format!(".{PLAYLIST_EXT}"))
            && is_safe_name(&name)
        {
            out.insert(list.to_string(), read_playlist(&e.path()));
        }
    }
    out
}

fn read_playlist(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        // 只接受同一個資料夾裡的檔案
        .filter(|l| is_safe_name(l))
        .map(str::to_string)
        .collect()
}

pub fn playlist_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{PLAYLIST_EXT}"))
}

pub fn write_playlist(dir: &Path, name: &str, files: &[String]) -> std::io::Result<()> {
    let mut out = String::from("#EXTM3U\n");
    for f in files {
        out.push_str(f);
        out.push('\n');
    }
    std::fs::write(playlist_path(dir, name), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_codes() {
        assert!(is_lang("en"));
        assert!(is_lang("zh-TW"));
        assert!(is_lang("zh-Hant"));
        assert!(!is_lang("MUSIC VIDEO"));
        assert!(!is_lang("Official Music Video"));
        assert!(!is_lang("e"));
    }

    #[test]
    fn scan_pairs_sidecars() {
        let dir = std::env::temp_dir().join(format!("cng6-music-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "A song.mp3",
            "A song.jpg",
            "A song.zh-TW.srt",
            "A song.en.srt",
            "B.flac",
            "B.lrc",
            "notes.txt",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let tracks = Cache::default().scan(&dir);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].file, "A song.mp3");
        assert_eq!(tracks[0].title, "A song");
        assert_eq!(tracks[0].cover.as_deref(), Some("A song.jpg"));
        let langs: Vec<_> = tracks[0].lyrics.iter().map(|l| l.lang.as_str()).collect();
        assert_eq!(langs, ["en", "zh-TW"]);
        assert_eq!(tracks[1].lyrics[0].lang, "");

        let removed = delete_track(&dir, "A song.mp3").unwrap();
        assert_eq!(removed.len(), 4);
        assert!(dir.join("notes.txt").exists());

        write_playlist(&dir, "最愛", &["B.flac".into()]).unwrap();
        // 不是真的音樂檔: 寫標籤要回報錯誤, 不能 panic
        assert!(set_tag(&dir.join("B.flac"), "artist", "X").is_err());
        assert_eq!(playlists(&dir)["最愛"], ["B.flac"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
