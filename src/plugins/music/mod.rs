//! Music plugin: 管理 `music/` 資料夾的音樂、從 YouTube 下載、播放清單。
//!
//! 播放只在網頁上(PC 與手機), 這裡負責曲目資訊、下載與檔案管理;
//! 網頁伺服器透過 [`Shared`] 讀取曲目、封面、歌詞與下載進度。
//!
//! 下載用 yt-dlp: 最高音質的音訊(保留原始格式, 不重新編碼), 封面嵌入檔案並另存 `.jpg`,
//! 字幕轉成 `<名稱>.<語言>.srt`, 在網頁當歌詞顯示。

pub mod library;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use ratatui::Frame;
use ratatui::layout::Rect;
use serde::Serialize;

use crate::plugin::{Ctx, Plugin, complete_from_help};
use crate::webui::{Card, Status, WebMeta};
use library::{Cache, Track};

const DEFAULT_DIR: &str = "music";
/// yt-dlp 的 --sub-langs(可以用正規表示式)
const DEFAULT_SUB_LANGS: &str = "zh.*,ja,en";

/// 下載工作的狀態
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum JobState {
    /// 下載中: 百分比、剩餘時間、目前階段
    Running {
        percent: f64,
        eta: String,
        phase: String,
    },
    Done {
        file: String,
    },
    Failed {
        error: String,
    },
    Canceled,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: u64,
    pub url: String,
    pub title: String,
    #[serde(flatten)]
    pub state: JobState,
}

struct Job {
    view: JobView,
    child: Option<Arc<Mutex<Child>>>,
    /// 完成訊息已經輸出到 Message
    notified: bool,
}

#[derive(Default)]
struct Inner {
    dir: RwLock<PathBuf>,
    sub_langs: RwLock<String>,
    cache: Cache,
    jobs: Mutex<Vec<Job>>,
    next_id: AtomicU64,
    /// 曲目或播放清單改變時加一, 網頁用來判斷要不要重新整理
    version: AtomicU64,
}

/// 和網頁伺服器共用的狀態
#[derive(Clone, Default)]
pub struct Shared(Arc<Inner>);

impl Shared {
    pub fn new() -> Self {
        let s = Self::default();
        *s.0.dir.write().unwrap() = DEFAULT_DIR.into();
        *s.0.sub_langs.write().unwrap() = DEFAULT_SUB_LANGS.into();
        s.0.next_id.store(1, Ordering::Relaxed);
        s.0.version.store(1, Ordering::Relaxed);
        s
    }

    pub fn dir(&self) -> PathBuf {
        self.0.dir.read().unwrap().clone()
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.0.cache.scan(&self.dir())
    }

    pub fn version(&self) -> u64 {
        self.0.version.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.0.version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn jobs(&self) -> Vec<JobView> {
        self.0
            .jobs
            .lock()
            .unwrap()
            .iter()
            .map(|j| j.view.clone())
            .collect()
    }

    fn running_jobs(&self) -> usize {
        self.0
            .jobs
            .lock()
            .unwrap()
            .iter()
            .filter(|j| matches!(j.view.state, JobState::Running { .. }))
            .count()
    }

    fn update_job(&self, id: u64, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self
            .0
            .jobs
            .lock()
            .unwrap()
            .iter_mut()
            .find(|j| j.view.id == id)
        {
            f(j);
        }
    }

    /// 開始下載, 回傳工作編號
    fn download(&self, url: &str) -> Result<u64, String> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("無法建立資料夾 {}: {e}", dir.display()))?;
        let langs = self.0.sub_langs.read().unwrap().clone();
        let mut cmd = Command::new("yt-dlp");
        cmd.current_dir(&dir)
            .args([
                "--no-playlist",
                // 最高音質的音訊; -x 但不指定格式 = 只把音訊取出, 不重新編碼
                "-f",
                "bestaudio/best",
                "-x",
                "--embed-metadata",
                // 封面: 嵌入音樂檔(其他播放器也看得到), 同時另存 .jpg(有 --write-thumbnail 時會保留)
                "--write-thumbnail",
                "--convert-thumbnails",
                "jpg",
                "--embed-thumbnail",
                // 字幕(歌詞) 轉成 srt
                "--write-subs",
                "--sub-langs",
                &langs,
                "--convert-subs",
                "srt",
                "-o",
                "%(title)s.%(ext)s",
                "--newline",
                "--no-simulate",
                "--progress-template",
                "download:[cng6] %(progress._percent_str)s|%(progress._eta_str)s",
                "--print",
                "before_dl:[title] %(title)s",
                "--print",
                "after_move:[file] %(filepath)s",
                url,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("無法執行 yt-dlp: {e} (請確認已安裝 yt-dlp)"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));

        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        self.0.jobs.lock().unwrap().push(Job {
            view: JobView {
                id,
                url: url.to_string(),
                title: url.to_string(),
                state: JobState::Running {
                    percent: 0.0,
                    eta: String::new(),
                    phase: "準備中".into(),
                },
            },
            child: Some(Arc::clone(&child)),
            notified: false,
        });

        // 錯誤訊息: 留最後幾行 stderr
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        if let Some(err) = stderr {
            let errors = Arc::clone(&errors);
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut e = errors.lock().unwrap();
                    e.push(line);
                    if e.len() > 5 {
                        e.remove(0);
                    }
                }
            });
        }

        let shared = self.clone();
        std::thread::spawn(move || {
            let mut file = None;
            if let Some(out) = stdout {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    shared.on_output(id, &line, &mut file);
                }
            }
            let status = child.lock().unwrap().wait();
            let state =
                match (status, file) {
                    (Ok(s), Some(f)) if s.success() => JobState::Done { file: f },
                    _ => {
                        let canceled =
                            shared.0.jobs.lock().unwrap().iter().any(|j| {
                                j.view.id == id && matches!(j.view.state, JobState::Canceled)
                            });
                        if canceled {
                            JobState::Canceled
                        } else {
                            let e = errors.lock().unwrap();
                            let msg = e
                                .iter()
                                .rev()
                                .find(|l| l.contains("ERROR"))
                                .or_else(|| e.last())
                                .cloned()
                                .unwrap_or_else(|| "下載失敗".into());
                            JobState::Failed {
                                error: msg.trim_start_matches("ERROR: ").to_string(),
                            }
                        }
                    }
                };
            shared.update_job(id, |j| {
                j.view.state = state;
                j.child = None;
            });
            shared.bump();
        });
        Ok(id)
    }

    /// 解析 yt-dlp 的輸出, 更新進度
    fn on_output(&self, id: u64, line: &str, file: &mut Option<String>) {
        if let Some(t) = line.strip_prefix("[title] ") {
            let t = t.to_string();
            self.update_job(id, |j| j.view.title = t);
        } else if let Some(p) = line.strip_prefix("[file] ") {
            *file = PathBuf::from(p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned());
        } else if let Some(rest) = line.strip_prefix("[cng6] ") {
            let (pct, eta) = rest.split_once('|').unwrap_or((rest, ""));
            let percent = pct
                .trim()
                .trim_end_matches('%')
                .trim()
                .parse()
                .unwrap_or(0.0);
            let eta = eta.trim().to_string();
            self.update_job(id, |j| {
                j.view.state = JobState::Running {
                    percent,
                    eta,
                    phase: "下載中".into(),
                }
            });
        } else if line.starts_with("[ExtractAudio]")
            || line.starts_with("[ThumbnailsConvertor]")
            || line.starts_with("[SubtitlesConvertor]")
            || line.starts_with("[Metadata]")
        {
            self.update_job(id, |j| {
                if let JobState::Running { phase, .. } = &mut j.view.state {
                    *phase = "處理中".into();
                }
            });
        }
    }

    fn cancel(&self, id: u64) -> Result<(), String> {
        let child = {
            let mut jobs = self.0.jobs.lock().unwrap();
            let job = jobs
                .iter_mut()
                .find(|j| j.view.id == id)
                .ok_or_else(|| format!("沒有下載工作 #{id}"))?;
            if !matches!(job.view.state, JobState::Running { .. }) {
                return Err(format!("下載工作 #{id} 已經結束"));
            }
            job.view.state = JobState::Canceled;
            job.child.clone()
        };
        if let Some(c) = child {
            let _ = c.lock().unwrap().kill();
        }
        Ok(())
    }
}

pub struct Music {
    shared: Shared,
}

impl Music {
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }

    fn track_names(&self) -> Vec<(String, String)> {
        self.shared
            .tracks()
            .into_iter()
            .map(|t| (t.file, t.title))
            .collect()
    }

    fn playlist_names(&self) -> Vec<(String, String)> {
        library::playlists(&self.shared.dir())
            .into_iter()
            .map(|(n, f)| (n, format!("{} 首", f.len())))
            .collect()
    }

    fn cmd_playlist(&mut self, args: &[&str], ctx: &mut Ctx) {
        let dir = self.shared.dir();
        let lists = library::playlists(&dir);
        let exists = |n: &str| lists.contains_key(n);
        let result: Result<String, String> = match args {
            [] | ["list"] => {
                if lists.is_empty() {
                    ctx.info("  (還沒有播放清單, 用 music playlist create <name> 建立)");
                }
                for (n, f) in &lists {
                    ctx.info(format!("  {n}  ({} 首)", f.len()));
                }
                return;
            }
            ["show", name] => match lists.get(*name) {
                Some(files) => {
                    for (i, f) in files.iter().enumerate() {
                        ctx.info(format!("  {:>3}. {f}", i + 1));
                    }
                    if files.is_empty() {
                        ctx.info("  (空的播放清單)");
                    }
                    return;
                }
                None => Err(format!("沒有播放清單 '{name}'")),
            },
            ["create", name] => {
                if !library::is_playlist_name(name) {
                    Err(format!("'{name}' 不能當作播放清單名稱"))
                } else if exists(name) {
                    Err(format!("播放清單 '{name}' 已經存在"))
                } else {
                    library::write_playlist(&dir, name, &[])
                        .map(|_| format!("已建立播放清單 {name}"))
                        .map_err(|e| e.to_string())
                }
            }
            ["delete", name] => {
                if !exists(name) {
                    Err(format!("沒有播放清單 '{name}'"))
                } else {
                    std::fs::remove_file(library::playlist_path(&dir, name))
                        .map(|_| format!("已刪除播放清單 {name}(歌曲沒有被刪除)"))
                        .map_err(|e| e.to_string())
                }
            }
            ["rename", old, new] => {
                if !exists(old) {
                    Err(format!("沒有播放清單 '{old}'"))
                } else if !library::is_playlist_name(new) || exists(new) {
                    Err(format!("'{new}' 不能使用(名稱不合法或已經存在)"))
                } else {
                    std::fs::rename(
                        library::playlist_path(&dir, old),
                        library::playlist_path(&dir, new),
                    )
                    .map(|_| format!("播放清單 {old} 已改名為 {new}"))
                    .map_err(|e| e.to_string())
                }
            }
            ["add", name, file] => match lists.get(*name) {
                None => Err(format!("沒有播放清單 '{name}'")),
                Some(_) if !dir.join(file).is_file() || !library::is_audio(file) => {
                    Err(format!("沒有音樂檔 '{file}'"))
                }
                Some(files) if files.iter().any(|f| f == file) => {
                    Err(format!("'{file}' 已經在 {name} 裡"))
                }
                Some(files) => {
                    let mut files = files.clone();
                    files.push(file.to_string());
                    library::write_playlist(&dir, name, &files)
                        .map(|_| format!("已加入 {name}: {file}"))
                        .map_err(|e| e.to_string())
                }
            },
            ["remove", name, file] => match lists.get(*name) {
                None => Err(format!("沒有播放清單 '{name}'")),
                Some(files) if !files.iter().any(|f| f == file) => {
                    Err(format!("'{file}' 不在 {name} 裡"))
                }
                Some(files) => {
                    let files: Vec<String> = files.iter().filter(|f| f != file).cloned().collect();
                    library::write_playlist(&dir, name, &files)
                        .map(|_| format!("已從 {name} 移除: {file}"))
                        .map_err(|e| e.to_string())
                }
            },
            // 調整順序: 把第 from 首移到第 to 首(從 1 開始)
            ["move", name, from, to] => {
                match (lists.get(*name), from.parse::<usize>(), to.parse::<usize>()) {
                    (None, ..) => Err(format!("沒有播放清單 '{name}'")),
                    (Some(files), Ok(a), Ok(b))
                        if (1..=files.len()).contains(&a) && (1..=files.len()).contains(&b) =>
                    {
                        let mut files = files.clone();
                        let f = files.remove(a - 1);
                        files.insert(b - 1, f);
                        library::write_playlist(&dir, name, &files)
                            .map(|_| format!("{name}: 第 {a} 首移到第 {b} 首"))
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("用法: music playlist move <name> <from> <to> (從 1 開始)".into()),
                }
            }
            _ => Err("用法: 輸入 'help music' 查看".into()),
        };
        match result {
            Ok(msg) => {
                self.shared.bump();
                ctx.info(msg);
            }
            Err(e) => ctx.error(e),
        }
    }
}

fn fmt_size(b: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if b as f64 >= 1024.0 * MIB {
        format!("{:.1} GiB", b as f64 / 1024.0 / MIB)
    } else {
        format!("{:.0} MiB", b as f64 / MIB)
    }
}

impl Plugin for Music {
    fn name(&self) -> &str {
        "music"
    }

    fn description(&self) -> &str {
        "音樂: 網頁播放、YouTube 下載、播放清單"
    }

    fn is_panel(&self) -> bool {
        false
    }

    fn help(&self) -> Vec<(&str, &str)> {
        vec![
            ("list [keyword]", "列出歌曲(可以用關鍵字篩選)"),
            (
                "download <url>",
                "從 YouTube 下載最高音質的音訊、封面與歌詞",
            ),
            ("downloads", "下載工作與進度"),
            ("cancel <id>", "取消下載"),
            (
                "delete <file>",
                "刪除歌曲(含封面與歌詞, 也會從播放清單移除)",
            ),
            (
                "tag <file> <title|artist|album> [value]",
                "修改歌名/歌手/專輯(寫入音樂檔), 不寫內容表示清除",
            ),
            ("playlist list", "列出播放清單"),
            ("playlist show <name>", "播放清單的歌曲"),
            ("playlist create <name>", "建立播放清單"),
            ("playlist delete <name>", "刪除播放清單(不會刪除歌曲)"),
            ("playlist rename <old> <new>", "播放清單改名"),
            ("playlist add <name> <file>", "加入歌曲"),
            ("playlist remove <name> <file>", "移除歌曲"),
            ("playlist move <name> <from> <to>", "調整順序"),
            ("status", "資料夾、歌曲數與下載狀態"),
        ]
    }

    fn complete(&self, args: &[&str]) -> Vec<(String, String)> {
        match args {
            ["delete" | "tag"] => self.track_names(),
            ["tag", _] => library::TAG_FIELDS
                .iter()
                .map(|(f, d)| (f.to_string(), d.to_string()))
                .collect(),
            ["tag", _, _] => vec![("[value]".into(), "新的內容, 不寫表示清除".into())],
            ["cancel"] => self
                .shared
                .jobs()
                .into_iter()
                .filter(|j| matches!(j.state, JobState::Running { .. }))
                .map(|j| (j.id.to_string(), j.title))
                .collect(),
            [
                "playlist",
                "show" | "delete" | "rename" | "add" | "remove" | "move",
            ] => self.playlist_names(),
            ["playlist", "add", _] => self.track_names(),
            ["playlist", "remove", name] => library::playlists(&self.shared.dir())
                .remove(*name)
                .unwrap_or_default()
                .into_iter()
                .map(|f| (f, String::new()))
                .collect(),
            _ => complete_from_help(&self.help(), args),
        }
    }

    fn settings(&self) -> Vec<(&str, &str, String)> {
        vec![
            ("dir", "存放音樂的資料夾", DEFAULT_DIR.into()),
            (
                "sub_langs",
                "下載的歌詞(字幕)語言, 逗號分隔, 可用正規表示式 (yt-dlp --sub-langs)",
                DEFAULT_SUB_LANGS.into(),
            ),
        ]
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dir" => {
                *self.shared.0.dir.write().unwrap() = PathBuf::from(value);
                self.shared.bump();
            }
            "sub_langs" => {
                if value.trim().is_empty() {
                    return Err("請輸入至少一個語言".into());
                }
                *self.shared.0.sub_langs.write().unwrap() = value.trim().to_string();
            }
            _ => return Err(format!("不支援的設定 '{key}'")),
        }
        Ok(())
    }

    fn handle_command(&mut self, args: &[&str], ctx: &mut Ctx) {
        match args {
            ["list", rest @ ..] => {
                let kw = rest.join(" ").to_lowercase();
                let tracks: Vec<Track> = self
                    .shared
                    .tracks()
                    .into_iter()
                    .filter(|t| {
                        kw.is_empty()
                            || t.file.to_lowercase().contains(&kw)
                            || t.title.to_lowercase().contains(&kw)
                            || t.artist.to_lowercase().contains(&kw)
                    })
                    .collect();
                for t in &tracks {
                    let artist = if t.artist.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", t.artist)
                    };
                    ctx.info(format!(
                        "  {}{artist}  [{}:{:02}]",
                        t.title,
                        t.duration as u64 / 60,
                        t.duration as u64 % 60
                    ));
                }
                ctx.info(format!("  共 {} 首", tracks.len()));
            }
            ["download", url] => match self.shared.download(url) {
                Ok(id) => ctx.info(format!(
                    "開始下載 #{id}: {url} (用 'music downloads' 查看進度)"
                )),
                Err(e) => ctx.error(e),
            },
            ["download", ..] => ctx.error("用法: music download <url>"),
            ["downloads"] => {
                let jobs = self.shared.jobs();
                if jobs.is_empty() {
                    ctx.info("  (沒有下載工作)");
                }
                for j in jobs {
                    let state = match &j.state {
                        JobState::Running {
                            percent,
                            eta,
                            phase,
                        } => {
                            format!(
                                "{phase} {percent:.0}%{}",
                                if eta.is_empty() {
                                    String::new()
                                } else {
                                    format!(" 剩 {eta}")
                                }
                            )
                        }
                        JobState::Done { file } => format!("完成 → {file}"),
                        JobState::Failed { error } => format!("失敗: {error}"),
                        JobState::Canceled => "已取消".into(),
                    };
                    ctx.info(format!("  #{} {}  {state}", j.id, j.title));
                }
            }
            ["cancel", id] => match id.parse() {
                Ok(id) => match self.shared.cancel(id) {
                    Ok(()) => ctx.info(format!("已取消下載 #{id}")),
                    Err(e) => ctx.error(e),
                },
                Err(_) => ctx.error("用法: music cancel <id>"),
            },
            ["delete", file] => {
                let dir = self.shared.dir();
                if !library::is_safe_name(file)
                    || !library::is_audio(file)
                    || !dir.join(file).is_file()
                {
                    ctx.error(format!("沒有音樂檔 '{file}'"));
                    return;
                }
                match library::delete_track(&dir, file) {
                    Ok(removed) => {
                        // 從所有播放清單移除
                        for (name, files) in library::playlists(&dir) {
                            if files.iter().any(|f| f == file) {
                                let kept: Vec<String> =
                                    files.into_iter().filter(|f| f != file).collect();
                                let _ = library::write_playlist(&dir, &name, &kept);
                            }
                        }
                        self.shared.bump();
                        ctx.info(format!("已刪除 {file}"));
                        if removed.len() > 1 {
                            ctx.info(format!("  一併刪除: {}", removed[1..].join(", ")));
                        }
                    }
                    Err(e) => ctx.error(format!("無法刪除 {file}: {e}")),
                }
            }
            ["tag", file, field, value @ ..] => {
                let dir = self.shared.dir();
                if !library::is_safe_name(file)
                    || !library::is_audio(file)
                    || !dir.join(file).is_file()
                {
                    ctx.error(format!("沒有音樂檔 '{file}'"));
                    return;
                }
                let value = value.join(" ");
                match library::set_tag(&dir.join(file), field, &value) {
                    Ok(()) => {
                        self.shared.bump();
                        let name = library::TAG_FIELDS
                            .iter()
                            .find(|(f, _)| f == field)
                            .map_or(*field, |(_, n)| *n);
                        if value.trim().is_empty() {
                            ctx.info(format!("{file}: 已清除{name}"));
                        } else {
                            ctx.info(format!("{file}: {name} = {}", value.trim()));
                        }
                    }
                    Err(e) => ctx.error(format!("{file}: {e}")),
                }
            }
            ["playlist", rest @ ..] => self.cmd_playlist(rest, ctx),
            ["status"] => {
                let tracks = self.shared.tracks();
                let size: u64 = tracks.iter().map(|t| t.size).sum();
                ctx.info(format!("資料夾:   {}", self.shared.dir().display()));
                ctx.info(format!("歌曲:     {} 首, {}", tracks.len(), fmt_size(size)));
                ctx.info(format!(
                    "播放清單: {} 個",
                    library::playlists(&self.shared.dir()).len()
                ));
                ctx.info(format!("下載中:   {}", self.shared.running_jobs()));
            }
            _ => ctx.error("用法: 輸入 'help music' 查看"),
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx) {
        // 下載結束時在 Message 顯示一行
        let mut jobs = self.shared.0.jobs.lock().unwrap();
        for j in jobs.iter_mut().filter(|j| !j.notified) {
            match &j.view.state {
                JobState::Running { .. } => continue,
                JobState::Done { file } => ctx.info(format!("music: 下載完成 {file}")),
                JobState::Failed { error } => {
                    ctx.error(format!("music: 下載失敗 {}: {error}", j.view.title))
                }
                JobState::Canceled => ctx.warn(format!("music: 已取消下載 {}", j.view.title)),
            }
            j.notified = true;
        }
        // 只保留最近 20 個已結束的工作
        let finished = jobs
            .iter()
            .filter(|j| !matches!(j.view.state, JobState::Running { .. }))
            .count();
        if finished > 20 {
            let mut drop = finished - 20;
            jobs.retain(|j| {
                if drop > 0 && j.notified && !matches!(j.view.state, JobState::Running { .. }) {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    fn web_meta(&self) -> Option<WebMeta> {
        Some(WebMeta::new("音樂", "music", "pink"))
    }

    fn web_view(&self) -> Option<&str> {
        Some("music")
    }

    fn web_card(&self) -> Option<Card> {
        let tracks = self.shared.tracks();
        let lists = library::playlists(&self.shared.dir()).len();
        let running = self.shared.running_jobs();
        let mut summary = format!("{} 首 · {lists} 個播放清單", tracks.len());
        if running > 0 {
            summary.push_str(&format!(" · 下載中 {running}"));
        }
        let mut card = Card::new().summary(summary).table([
            ("歌曲", format!("{} 首", tracks.len())),
            ("大小", fmt_size(tracks.iter().map(|t| t.size).sum())),
            ("播放清單", format!("{lists} 個")),
        ]);
        for j in self.shared.jobs() {
            if let JobState::Running { percent, phase, .. } = j.state {
                card = card.meter(
                    j.title,
                    percent / 100.0,
                    format!("{phase} {percent:.0}%"),
                    None,
                );
            }
        }
        if running > 0 {
            card = card.status(Status::Good);
        }
        Some(card)
    }

    fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
}
