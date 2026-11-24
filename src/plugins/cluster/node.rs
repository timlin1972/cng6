//! 機器之間交換的資料: 每台定時送出自己的基本狀態。

use serde::{Deserialize, Serialize};
use sysinfo::{
    Components, CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System,
};

/// 一台機器的基本狀態
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Info {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub cpu: String,
    pub cores: usize,
    /// cng6 版本
    pub version: String,
    /// CPU 使用率 (%)
    pub cpu_usage: f32,
    pub load: [f64; 3],
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// 根目錄(/)所在磁碟
    pub disk_used: u64,
    pub disk_total: u64,
    /// 最高的溫度 (°C)
    pub temp: Option<f32>,
    /// 開機多久(秒)
    pub uptime: u64,
    /// cng6 啟動時間(Unix 秒)
    pub started: i64,
    /// 區域網路 IP
    pub ips: Vec<String>,
}

/// MQTT 上傳送的訊息(加密前的 JSON)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub name: String,
    /// server / client
    pub role: String,
    /// 所屬的 server
    pub server: String,
    /// 每次連線不同, 用來辨認離線通知是不是這次連線的
    pub session: String,
    /// 送出時間(Unix 秒, 送出那台的時鐘)
    pub ts: i64,
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<Info>,
}

/// 收集本機的狀態
pub struct Collector {
    sys: System,
    components: Components,
    disks: Disks,
    networks: Networks,
    started: i64,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            sys: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            components: Components::new_with_refreshed_list(),
            // 只列出掛載點, 不查容量: WSL 的 /mnt/c 等 Windows 磁碟(9p)
            // 在 Windows 硬碟休眠或忙碌時 statvfs 會卡住好幾秒
            disks: Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing()),
            networks: Networks::new_with_refreshed_list(),
            started: chrono::Utc::now().timestamp(),
        }
    }

    pub fn collect(&mut self) -> Info {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.components.refresh(false);
        // 只更新根目錄的容量, 其他磁碟不碰(見 new())
        if let Some(d) = self
            .disks
            .list_mut()
            .iter_mut()
            .find(|d| d.mount_point() == std::path::Path::new("/"))
        {
            d.refresh_specifics(DiskRefreshKind::nothing().with_storage());
        }
        self.networks.refresh(true);
        let load = System::load_average();
        let (disk_used, disk_total) = self
            .disks
            .iter()
            .find(|d| d.mount_point() == std::path::Path::new("/"))
            .map(|d| (d.total_space() - d.available_space(), d.total_space()))
            .unwrap_or((0, 0));
        let temp = self
            .components
            .iter()
            .filter_map(|c| c.temperature())
            .filter(|t| t.is_finite())
            .fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.max(t))));
        // 區域網路 IP: 不含 loopback、docker 與 IPv6 link-local
        let mut ips: Vec<String> = self
            .networks
            .iter()
            .filter(|(name, _)| {
                *name != "lo"
                    && !name.starts_with("docker")
                    && !name.starts_with("br-")
                    && !name.starts_with("veth")
            })
            .flat_map(|(_, d)| d.ip_networks().iter().map(|n| n.addr))
            .filter(|a| match a {
                std::net::IpAddr::V4(v4) => !v4.is_loopback(),
                std::net::IpAddr::V6(v6) => {
                    !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80
                }
            })
            .map(|a| a.to_string())
            .collect();
        ips.sort_by_key(|ip| (ip.contains(':'), ip.clone()));
        Info {
            os: System::long_os_version().unwrap_or_default(),
            kernel: System::kernel_version().unwrap_or_default(),
            arch: System::cpu_arch(),
            cpu: self
                .sys
                .cpus()
                .first()
                .map(|c| c.brand().trim().to_string())
                .unwrap_or_default(),
            cores: self.sys.cpus().len(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            cpu_usage: self.sys.global_cpu_usage(),
            load: [load.one, load.five, load.fifteen],
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            disk_used,
            disk_total,
            temp,
            uptime: System::uptime(),
            started: self.started,
            ips,
        }
    }
}

/// 本機的名稱(hostname)
pub fn hostname() -> String {
    System::host_name()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "cng6".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let m = Message {
            name: "pi5".into(),
            role: "server".into(),
            server: "pi5".into(),
            session: "ab".into(),
            ts: 1,
            online: true,
            info: Some(Info {
                cores: 4,
                ..Default::default()
            }),
        };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Message>(&s).unwrap(), m);
        // 離線訊息沒有 info
        let off = Message {
            online: false,
            info: None,
            ..m
        };
        assert!(!serde_json::to_string(&off).unwrap().contains("info"));
    }

    #[test]
    fn collects() {
        let mut c = Collector::new();
        let i = c.collect();
        assert!(i.cores > 0);
        assert!(i.mem_total > 0);
        // 只更新根目錄, 但根目錄的容量要拿得到
        assert!(i.disk_total > 0);
        assert!(!hostname().is_empty());
    }
}
