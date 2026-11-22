//! Plugin 給網頁(PC 桌面與手機)的描述。
//!
//! 新 plugin 依需要提供, 越簡單的 plugin 越不用寫前端:
//!
//! 1. 什麼都不提供: 指令與設定仍會自動出現在網頁 CLI 與 Settings
//! 2. 名稱、圖示與顏色: 提供 [`WebMeta`]
//! 3. 狀態卡片: 提供 [`Card`](數字、進度條、表格、開關、按鈕…), 由前端通用的程式繪製;
//!    PC 是 Dock 上的一個視窗, 手機是總覽裡的一張卡片
//! 4. 自訂畫面: `web_view()` 回傳前端模組名稱(例如 `"system"`), 由對應的 JS 繪製

use serde::Serialize;

/// 網頁上顯示的名稱、圖示與顏色
#[derive(Debug, Clone, Serialize)]
pub struct WebMeta {
    pub title: String,
    /// 圖示名稱, 前端 `apps.js` 的 ICONS 裡要有; 沒有的會用通用圖示
    pub icon: &'static str,
    /// 圖示底色: blue / green / teal / purple / orange / red / gray / dark / indigo / pink
    pub color: &'static str,
}

impl WebMeta {
    pub fn new(title: impl Into<String>, icon: &'static str, color: &'static str) -> Self {
        Self {
            title: title.into(),
            icon,
            color,
        }
    }
}

/// 狀態(顏色與圖示由前端決定, 一律搭配文字, 不只靠顏色表達)
///
/// 依嚴重程度排序, 可以用 `max` 取得最嚴重的狀態
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// 停用 / 未執行
    Off,
    Good,
    Warning,
    Critical,
}

impl Status {
    /// 依門檻判斷: 超過 crit 為 Critical, 超過 warn 為 Warning
    pub fn by(value: f64, warn: f64, crit: f64) -> Self {
        if value >= crit {
            Status::Critical
        } else if value >= warn {
            Status::Warning
        } else {
            Status::Good
        }
    }
}

/// 卡片裡的一個元件
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Widget {
    /// 大數字, 例如 CPU 42%
    Stat {
        label: String,
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// 進度條, ratio 0~1
    Meter {
        label: String,
        ratio: f64,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
    },
    /// 名稱/值 表格
    Table { rows: Vec<(String, String)> },
    /// 一段文字
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
    },
    /// 連結(例如網址)
    Link { label: String, url: String },
    /// 開關: 切換時執行 `settings set <setting> true|false`
    Toggle {
        label: String,
        setting: String,
        value: bool,
    },
    /// 一排按鈕: 按下時執行指令
    Buttons { buttons: Vec<Button> },
}

#[derive(Debug, Clone, Serialize)]
pub struct Button {
    pub label: String,
    /// 要執行的指令(從最上層開始, 例如 `/web restart`)
    pub command: String,
    /// 危險的動作會先確認
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

impl Button {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
            confirm: false,
        }
    }

    pub fn confirm(mut self) -> Self {
        self.confirm = true;
        self
    }
}

/// 狀態卡片
#[derive(Debug, Clone, Default, Serialize)]
pub struct Card {
    /// 整張卡片的狀態(手機總覽會把有警告的卡片排到前面)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// 一行摘要, 例如「執行中 · port 9760」
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub widgets: Vec<Widget>,
}

/// 用串接的方式組卡片:
/// `Card::new().stat("CPU", "42", Some("%"), Some(Status::Good)).buttons(...)`
impl Card {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    pub fn summary(mut self, text: impl Into<String>) -> Self {
        self.summary = Some(text.into());
        self
    }

    pub fn stat(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
        unit: Option<&str>,
        status: Option<Status>,
    ) -> Self {
        self.widgets.push(Widget::Stat {
            label: label.into(),
            value: value.into(),
            unit: unit.map(str::to_string),
            status,
            hint: None,
        });
        self
    }

    /// 給最後一個 Stat 加上小字說明
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        if let Some(Widget::Stat { hint, .. }) = self.widgets.last_mut() {
            *hint = Some(text.into());
        }
        self
    }

    pub fn meter(
        mut self,
        label: impl Into<String>,
        ratio: f64,
        text: impl Into<String>,
        status: Option<Status>,
    ) -> Self {
        self.widgets.push(Widget::Meter {
            label: label.into(),
            ratio: ratio.clamp(0.0, 1.0),
            text: text.into(),
            status,
        });
        self
    }

    pub fn table<K: Into<String>, V: Into<String>>(
        mut self,
        rows: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.widgets.push(Widget::Table {
            rows: rows
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        });
        self
    }

    pub fn text(mut self, text: impl Into<String>, status: Option<Status>) -> Self {
        self.widgets.push(Widget::Text {
            text: text.into(),
            status,
        });
        self
    }

    pub fn link(mut self, label: impl Into<String>, url: impl Into<String>) -> Self {
        self.widgets.push(Widget::Link {
            label: label.into(),
            url: url.into(),
        });
        self
    }

    pub fn toggle(
        mut self,
        label: impl Into<String>,
        setting: impl Into<String>,
        value: bool,
    ) -> Self {
        self.widgets.push(Widget::Toggle {
            label: label.into(),
            setting: setting.into(),
            value,
        });
        self
    }

    pub fn buttons(mut self, buttons: Vec<Button>) -> Self {
        self.widgets.push(Widget::Buttons { buttons });
        self
    }
}

/// `/api/apps` 回傳的一個 plugin
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    /// plugin 名稱(指令與設定的前綴)
    pub name: String,
    pub description: String,
    #[serde(flatten)]
    pub meta: WebMeta,
    /// 有沒有設定(Settings 會列出這個分組)
    pub has_settings: bool,
    /// 自訂畫面的前端模組名稱
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_json() {
        let card = Card::new()
            .status(Status::Warning)
            .stat("CPU", "42", Some("%"), Some(Status::Good))
            .hint("4 核心")
            .buttons(vec![Button::new("重新啟動", "/web restart").confirm()]);
        let v = serde_json::to_value(&card).unwrap();
        assert_eq!(v["status"], "warning");
        assert_eq!(v["widgets"][0]["type"], "stat");
        assert_eq!(v["widgets"][0]["hint"], "4 核心");
        assert_eq!(v["widgets"][1]["buttons"][0]["confirm"], true);
        assert!(v.get("summary").is_none());
    }
}
