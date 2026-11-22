//! 系統設定檔(`settings.conf`)的讀寫。
//!
//! 格式為一行一個 `key = value`, `#` 開頭為註解。只有和預設值不同的設定會成為有效的行,
//! 預設值會以註解寫出, 方便手動編輯時參考。

use std::collections::BTreeMap;
use std::path::PathBuf;

/// 讀取設定檔的結果: (設定值, 格式警告)
pub type Parsed = (BTreeMap<String, String>, Vec<String>);

pub struct Settings {
    path: PathBuf,
    /// 和預設值不同的設定
    values: BTreeMap<String, String>,
}

impl Settings {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            values: BTreeMap::new(),
        }
    }

    pub fn path(&self) -> String {
        self.path.display().to_string()
    }

    /// 讀取設定檔。檔案不存在時回傳 Ok(None), 其餘回傳 (設定值, 格式警告)。
    pub fn read(&self) -> Result<Option<Parsed>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => Ok(Some(parse(&content))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("無法讀取 {}: {e}", self.path())),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// 設定值, 等於預設值時移除(回到預設)
    pub fn set(&mut self, key: &str, value: &str, default: &str) {
        if value == default {
            self.values.remove(key);
        } else {
            self.values.insert(key.to_string(), value.to_string());
        }
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// 寫入設定檔。`defs` 是所有設定的 (key, 說明, 預設值)。
    pub fn save(&self, defs: &[(String, String, String)]) -> Result<(), String> {
        let mut out = String::from(
            "# cng6 設定檔\n\
             # 由 `settings set` / `settings reset` 自動寫入, 也可以手動編輯後用 `settings load` 重新讀取。\n\
             # 格式: key = value; 以 # 開頭的行是註解, 註解掉的設定代表使用預設值。\n",
        );
        for (key, desc, default) in defs {
            out.push_str(&format!("\n# {desc}\n"));
            match self.values.get(key) {
                Some(v) => out.push_str(&format!("{key} = {v}\n")),
                None => out.push_str(&format!("# {key} = {default}\n")),
            }
        }
        // 目前沒有對應定義的設定(例如 plugin 被移除)也保留下來
        let unknown: Vec<_> = self
            .values
            .iter()
            .filter(|(k, _)| !defs.iter().any(|(d, _, _)| d == *k))
            .collect();
        if !unknown.is_empty() {
            out.push_str("\n# 未知的設定\n");
            for (k, v) in unknown {
                out.push_str(&format!("{k} = {v}\n"));
            }
        }
        std::fs::write(&self.path, out).map_err(|e| format!("無法寫入 {}: {e}", self.path()))
    }

    pub fn replace(&mut self, values: BTreeMap<String, String>) {
        self.values = values;
    }
}

/// 解析設定檔內容, 回傳 (設定值, 格式警告)
pub fn parse(content: &str) -> Parsed {
    let mut values = BTreeMap::new();
    let mut warnings = Vec::new();
    for (n, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() => {
                // 值內的多個空白壓成一個
                let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
                values.insert(k.trim().to_string(), v);
            }
            _ => warnings.push(format!("第 {} 行格式錯誤: {line}", n + 1)),
        }
    }
    (values, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lines() {
        let (v, w) = parse(
            "# comment\n\
             system.interval = 2\n\
             \n\
             panel.message.rect =  0  50 100 100 \n\
             # panel.system.rect = 0 0 50 50\n\
             bad line\n",
        );
        assert_eq!(v.get("system.interval").map(String::as_str), Some("2"));
        assert_eq!(
            v.get("panel.message.rect").map(String::as_str),
            Some("0 50 100 100")
        );
        assert!(!v.contains_key("panel.system.rect"));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn save_and_parse_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cng6-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");
        let mut s = Settings::new(&path);
        let defs = vec![
            ("a.x".to_string(), "A".to_string(), "1".to_string()),
            ("b.y".to_string(), "B".to_string(), "2".to_string()),
        ];
        s.set("a.x", "5", "1");
        s.set("b.y", "2", "2"); // 等於預設, 不會寫入
        s.save(&defs).unwrap();
        let (v, w) = s.read().unwrap().unwrap();
        assert!(w.is_empty());
        assert_eq!(v.len(), 1);
        assert_eq!(v["a.x"], "5");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
