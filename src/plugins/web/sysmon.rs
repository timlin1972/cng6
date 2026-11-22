//! 系統監控: 每秒收集一次系統資訊, 保留最近一段時間的歷史, 並即時推送給網頁。
//!
//! - `GET /api/system`        → 主機資訊 + 歷史資料(網頁開啟時先載入)
//! - `GET /api/system/stream` → Server-Sent Events, 每秒一筆新資料

use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::header;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use sysinfo::{
    Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind,
    ProcessesToUpdate, RefreshKind, System,
};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

/// 取樣間隔
const INTERVAL: Duration = Duration::from_secs(1);
/// 保留的歷史筆數(5 分鐘)
const HISTORY: usize = 300;

/// 不會變的主機資訊
#[derive(Serialize)]
struct Info {
    host: String,
    os: String,
    kernel: String,
    arch: String,
    cpu: String,
    cores: usize,
    interval_ms: u64,
}

#[derive(Serialize)]
struct Temp {
    name: String,
    c: f32,
}

#[derive(Serialize)]
struct Net {
    name: String,
    /// 每秒接收/傳送 bytes
    rx: f64,
    tx: f64,
}

#[derive(Serialize)]
struct DiskSpace {
    mount: String,
    used: u64,
    total: u64,
}

/// 一次取樣
#[derive(Serialize)]
struct Sample {
    /// Unix 時間 (ms)
    t: i64,
    /// CPU 總使用率 (%)
    cpu: f32,
    /// 每個核心的使用率 (%)
    cores: Vec<f32>,
    mem_used: u64,
    mem_total: u64,
    swap_used: u64,
    swap_total: u64,
    load: [f64; 3],
    temps: Vec<Temp>,
    net: Vec<Net>,
    /// 磁碟每秒讀取/寫入 bytes
    disk_read: f64,
    disk_write: f64,
    disks: Vec<DiskSpace>,
    uptime: u64,
    procs: usize,
}

/// 實際讀取系統資料(會阻塞, 在 blocking thread 執行)
struct Collector {
    sys: System,
    components: Components,
    disks: Disks,
    networks: Networks,
    last: Instant,
}

impl Collector {
    fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            sys,
            components: Components::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            last: Instant::now(),
        }
    }

    fn info(&self) -> Info {
        Info {
            host: System::host_name().unwrap_or_else(|| "?".into()),
            os: System::long_os_version().unwrap_or_else(|| "?".into()),
            kernel: System::kernel_version().unwrap_or_else(|| "?".into()),
            arch: System::cpu_arch(),
            cpu: self
                .sys
                .cpus()
                .first()
                .map(|c| c.brand().trim().to_string())
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "?".into()),
            cores: self.sys.cpus().len(),
            interval_ms: INTERVAL.as_millis() as u64,
        }
    }

    fn sample(&mut self) -> Sample {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        self.components.refresh(false);
        self.disks.refresh(true);
        self.networks.refresh(true);
        let secs = self.last.elapsed().as_secs_f64().max(0.001);
        self.last = Instant::now();

        let load = System::load_average();
        let temps = self
            .components
            .iter()
            .filter_map(|c| {
                c.temperature().map(|t| Temp {
                    name: c.label().to_string(),
                    c: t,
                })
            })
            .collect();

        let mut net: Vec<Net> = self
            .networks
            .iter()
            .filter(|(name, _)| *name != "lo" && !name.starts_with("veth"))
            .map(|(name, d)| Net {
                name: name.clone(),
                rx: d.received() as f64 / secs,
                tx: d.transmitted() as f64 / secs,
            })
            .collect();
        net.sort_by(|a, b| a.name.cmp(&b.name));

        // 同一個裝置可能掛載在多個地方, I/O 只算一次
        let mut seen_dev = HashSet::new();
        let (mut read, mut write) = (0u64, 0u64);
        let mut seen_mount = HashSet::new();
        let mut disks = Vec::new();
        for d in self.disks.iter() {
            if seen_dev.insert(d.name().to_os_string()) {
                let u = d.usage();
                read += u.read_bytes;
                write += u.written_bytes;
            }
            let mount = d.mount_point().display().to_string();
            if d.total_space() > 0 && seen_mount.insert(mount.clone()) {
                disks.push(DiskSpace {
                    mount,
                    used: d.total_space() - d.available_space(),
                    total: d.total_space(),
                });
            }
        }

        Sample {
            t: chrono::Utc::now().timestamp_millis(),
            cpu: self.sys.global_cpu_usage(),
            cores: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            load: [load.one, load.five, load.fifteen],
            temps,
            net,
            disk_read: read as f64 / secs,
            disk_write: write as f64 / secs,
            disks,
            uptime: System::uptime(),
            // Linux 上 thread 也會被列出來, 只算真正的 process
            procs: self
                .sys
                .processes()
                .values()
                .filter(|p| p.thread_kind().is_none())
                .count(),
        }
    }
}

/// 網頁伺服器共用的監控狀態
pub struct SysMon {
    info_json: String,
    /// 每筆取樣的 JSON
    history: Mutex<VecDeque<Arc<str>>>,
    tx: broadcast::Sender<Arc<str>>,
}

impl SysMon {
    /// 建立監控並在目前的 tokio runtime 開始取樣
    pub fn spawn() -> Arc<Self> {
        let collector = Collector::new();
        let (tx, _) = broadcast::channel(16);
        let mon = Arc::new(Self {
            info_json: serde_json::to_string(&collector.info()).unwrap_or_else(|_| "{}".into()),
            history: Mutex::new(VecDeque::with_capacity(HISTORY)),
            tx,
        });
        tokio::spawn(run(Arc::clone(&mon), collector));
        mon
    }
}

async fn run(mon: Arc<SysMon>, mut collector: Collector) {
    let mut tick = tokio::time::interval(INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // 第一次 CPU 使用率需要兩次取樣之間的差異, 先丟掉一筆
    let _ = collector.sample();
    loop {
        tick.tick().await;
        let Ok((c, sample)) = tokio::task::spawn_blocking(move || {
            let s = collector.sample();
            (collector, s)
        })
        .await
        else {
            return;
        };
        collector = c;
        let Ok(json) = serde_json::to_string(&sample) else {
            continue;
        };
        let json: Arc<str> = json.into();
        {
            let mut h = mon.history.lock().unwrap();
            if h.len() == HISTORY {
                h.pop_front();
            }
            h.push_back(Arc::clone(&json));
        }
        // 沒有人在看時 send 會失敗, 不影響
        let _ = mon.tx.send(json);
    }
}

/// `GET /api/system`: 主機資訊 + 歷史資料
pub async fn snapshot(State(mon): State<Arc<SysMon>>) -> Response {
    let history = mon.history.lock().unwrap();
    let mut body = String::with_capacity(history.iter().map(|s| s.len() + 1).sum::<usize>() + 64);
    body.push_str("{\"info\":");
    body.push_str(&mon.info_json);
    body.push_str(",\"history\":[");
    for (i, s) in history.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str(s);
    }
    body.push_str("]}");
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// `GET /api/system/stream`: 每秒推送一筆新資料
pub async fn stream(
    State(mon): State<Arc<SysMon>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let events = BroadcastStream::new(mon.tx.subscribe())
        // 落後太多(Lagged)就略過, 下一筆會跟上
        .filter_map(|r| r.ok())
        .map(|json| Ok(Event::default().data(&*json)));
    Sse::new(events).keep_alive(KeepAlive::default())
}
