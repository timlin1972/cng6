//! 異常通知: 機器離線、沒有回應、溫度太高、磁碟或記憶體快滿。
//!
//! 狀態改變時(開始 / 恢復)才通知一次; 恢復要低於門檻一點才算, 避免來回跳。

use std::collections::BTreeMap;

use serde::Serialize;

use super::mqtt::Node;

/// 門檻
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// °C
    pub temp: f32,
    /// %
    pub disk: f64,
    /// %
    pub mem: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            temp: 75.0,
            disk: 90.0,
            mem: 90.0,
        }
    }
}

/// 恢復時要比門檻低這麼多
const HYSTERESIS: f64 = 5.0;
/// 保留的通知數
const KEEP: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub ts: i64,
    pub node: String,
    /// offline / stale / temp / disk / mem
    pub kind: &'static str,
    pub text: String,
    /// 還沒恢復
    pub active: bool,
}

#[derive(Debug, Default)]
pub struct Alerts {
    /// 目前有問題的: (機器, 種類) → 通知
    active: BTreeMap<(String, &'static str), Alert>,
    /// 最近的通知(新的在前)
    pub history: Vec<Alert>,
    /// 第一次檢查只記下狀態, 不通知(避免一啟動就把很久以前離線的機器都報一次)
    started: bool,
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 }
}

/// 一台機器目前的問題: (種類, 數值, 是否超過門檻, 是否已恢復)
fn conditions(n: &Node, stale: bool, l: &Limits) -> Vec<(&'static str, String, bool, bool)> {
    let mut out = Vec::new();
    if !n.me {
        out.push(("offline", "離線".to_string(), !n.online, n.online));
        out.push(("stale", "沒有回應".to_string(), n.online && stale, !stale));
    }
    // 離線的機器只報離線, 其他資料已經過時
    let live = n.me || (n.online && !stale);
    if let (Some(i), true) = (&n.info, live) {
        if let Some(t) = i.temp {
            out.push((
                "temp",
                format!("溫度 {t:.0}°C"),
                t >= l.temp,
                (t as f64) < l.temp as f64 - HYSTERESIS,
            ));
        }
        let d = pct(i.disk_used, i.disk_total);
        out.push(("disk", format!("磁碟 {d:.0}%"), d >= l.disk, d < l.disk - HYSTERESIS));
        let m = pct(i.mem_used, i.mem_total);
        out.push(("mem", format!("記憶體 {m:.0}%"), m >= l.mem, m < l.mem - HYSTERESIS));
    }
    out
}

impl Alerts {
    pub fn active(&self) -> Vec<Alert> {
        self.active.values().cloned().collect()
    }

    /// 檢查所有機器; 回傳要通知的訊息 (是否為問題, 文字)
    pub fn check(
        &mut self,
        nodes: &BTreeMap<String, Node>,
        is_stale: impl Fn(&Node) -> bool,
        limits: &Limits,
        now: i64,
    ) -> Vec<(bool, String)> {
        let mut out = Vec::new();
        let first = !self.started;
        self.started = true;
        for n in nodes.values() {
            for (kind, text, bad, ok) in conditions(n, is_stale(n), limits) {
                let key = (n.name.clone(), kind);
                let was = self.active.contains_key(&key);
                if bad && !was {
                    let a = Alert {
                        ts: now,
                        node: n.name.clone(),
                        kind,
                        text: text.clone(),
                        active: true,
                    };
                    self.active.insert(key, a.clone());
                    // 啟動時已經離線的機器不通知, 其他問題照樣通知
                    if !(first && kind == "offline") {
                        out.push((true, format!("{}: {text}", n.name)));
                        self.push(a);
                    }
                } else if was && ok {
                    self.active.remove(&key);
                    let msg = match kind {
                        "offline" | "stale" => "恢復連線".to_string(),
                        _ => format!("{text}, 恢復正常"),
                    };
                    for h in self.history.iter_mut().filter(|h| h.node == n.name && h.kind == kind) {
                        h.active = false;
                    }
                    out.push((false, format!("{}: {msg}", n.name)));
                } else if was && bad {
                    // 更新數值
                    if let Some(a) = self.active.get_mut(&key) {
                        a.text = text;
                    }
                }
            }
        }
        // 已經不在群組裡的機器(被 forget)
        self.active.retain(|(name, _), _| nodes.contains_key(name));
        out
    }

    fn push(&mut self, a: Alert) {
        self.history.insert(0, a);
        self.history.truncate(KEEP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::cluster::node::Info;

    fn node(name: &str, online: bool, temp: f32) -> Node {
        Node {
            name: name.into(),
            role: "client".into(),
            online,
            me: false,
            last_seen: 0,
            session: String::new(),
            ts: 0,
            info: Some(Info {
                temp: Some(temp),
                mem_total: 100,
                mem_used: 10,
                disk_total: 100,
                disk_used: 10,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn rises_and_clears() {
        let mut a = Alerts::default();
        let l = Limits::default();
        let mut nodes = BTreeMap::new();
        // 啟動時就離線: 不通知
        nodes.insert("old".to_string(), node("old", false, 40.0));
        nodes.insert("pi".to_string(), node("pi", true, 50.0));
        assert!(a.check(&nodes, |_| false, &l, 1).is_empty());
        // 溫度太高
        nodes.insert("pi".to_string(), node("pi", true, 80.0));
        let out = a.check(&nodes, |_| false, &l, 2);
        assert_eq!(out, vec![(true, "pi: 溫度 80°C".to_string())]);
        // 還在門檻附近: 不重複通知, 也不算恢復
        nodes.insert("pi".to_string(), node("pi", true, 73.0));
        assert!(a.check(&nodes, |_| false, &l, 3).is_empty());
        nodes.insert("pi".to_string(), node("pi", true, 60.0));
        let out = a.check(&nodes, |_| false, &l, 4);
        assert!(out[0].1.contains("恢復正常"));
        // 離線 → 恢復
        nodes.insert("pi".to_string(), node("pi", false, 60.0));
        assert_eq!(a.check(&nodes, |_| false, &l, 5)[0].1, "pi: 離線");
        nodes.insert("pi".to_string(), node("pi", true, 60.0));
        assert_eq!(a.check(&nodes, |_| false, &l, 6)[0].1, "pi: 恢復連線");
        assert_eq!(a.history.len(), 2);
        assert!(a.active().iter().all(|x| x.node == "old"));
    }
}
