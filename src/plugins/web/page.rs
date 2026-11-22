//! 網頁內容: 判斷來源裝置, 依裝置產生頁面。

use std::fmt;

/// 來源裝置類型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// 桌機/筆電等大畫面
    Pc,
    /// 手機等小畫面
    Mobile,
}

impl Device {
    fn as_str(self) -> &'static str {
        match self {
            Device::Pc => "pc",
            Device::Mobile => "mobile",
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 判斷來源裝置, 優先順序:
///
/// 1. 網址參數 `?device=pc|mobile`(手動指定, 方便測試)
/// 2. `Sec-CH-UA-Mobile` client hint(Chromium 系瀏覽器會送, `?1` 表示 mobile)
/// 3. User-Agent 含有 `Mobi`(MDN 建議的判斷方式, 涵蓋 iPhone / Android 手機)
///
/// 平板(Android 平板、iPadOS)通常會被判斷為 PC, 符合「大畫面」的分類。
pub fn detect_device(query: Option<&str>, ch_mobile: Option<&str>, user_agent: &str) -> Device {
    let forced =
        query
            .into_iter()
            .flat_map(|q| q.split('&'))
            .find_map(|kv| match kv.split_once('=') {
                Some(("device", "pc")) => Some(Device::Pc),
                Some(("device", "mobile")) => Some(Device::Mobile),
                _ => None,
            });
    if let Some(d) = forced {
        return d;
    }
    match ch_mobile.map(str::trim) {
        Some("?1") => return Device::Mobile,
        Some("?0") => return Device::Pc,
        _ => {}
    }
    if user_agent.contains("Mobi") {
        Device::Mobile
    } else {
        Device::Pc
    }
}

/// PC 版桌面(Dock + 視窗), 編譯時嵌入程式
const DESKTOP_HTML: &str = include_str!("assets/desktop.html");
const DESKTOP_CSS: &str = include_str!("assets/desktop.css");
const DESKTOP_JS: &str = include_str!("assets/desktop.js");
const APPS_CSS: &str = include_str!("assets/apps.css");
const APPS_JS: &str = include_str!("assets/apps.js");
/// 手機版
const MOBILE_HTML: &str = include_str!("assets/mobile.html");
const MOBILE_CSS: &str = include_str!("assets/mobile.css");
const MOBILE_JS: &str = include_str!("assets/mobile.js");
/// 音樂(PC 與手機共用)
const MUSIC_CSS: &str = include_str!("assets/music.css");
const MUSIC_JS: &str = include_str!("assets/music.js");
/// 天氣(PC 與手機共用)
const WEATHER_CSS: &str = include_str!("assets/weather.css");
const WEATHER_JS: &str = include_str!("assets/weather.js");
/// 世界時間(PC 與手機共用)
const WC_CSS: &str = include_str!("assets/worldclock.css");
const WC_JS: &str = include_str!("assets/worldclock.js");
const SYSTEM_CSS: &str = include_str!("assets/system.css");
const SYSTEM_JS: &str = include_str!("assets/system.js");
const SHELL_CSS: &str = include_str!("assets/shell.css");
const SHELL_JS: &str = include_str!("assets/shell.js");
const SETTINGS_CSS: &str = include_str!("assets/settings.css");
const SETTINGS_JS: &str = include_str!("assets/settings.js");

/// 依裝置產生頁面
pub fn render_page(device: Device) -> String {
    match device {
        Device::Pc => DESKTOP_HTML
            .replacen("/*CSS*/", DESKTOP_CSS, 1)
            .replacen("/*APPSCSS*/", APPS_CSS, 1)
            .replacen("/*APPSJS*/", APPS_JS, 1)
            .replacen("/*SYSCSS*/", SYSTEM_CSS, 1)
            .replacen("/*SYSJS*/", SYSTEM_JS, 1)
            .replacen("/*SHELLCSS*/", SHELL_CSS, 1)
            .replacen("/*SHELLJS*/", SHELL_JS, 1)
            .replacen("/*SETCSS*/", SETTINGS_CSS, 1)
            .replacen("/*SETJS*/", SETTINGS_JS, 1)
            .replacen("/*MUSICCSS*/", MUSIC_CSS, 1)
            .replacen("/*MUSICJS*/", MUSIC_JS, 1)
            .replacen("/*WEATHERCSS*/", WEATHER_CSS, 1)
            .replacen("/*WEATHERJS*/", WEATHER_JS, 1)
            .replacen("/*WCCSS*/", WC_CSS, 1)
            .replacen("/*WCJS*/", WC_JS, 1)
            .replacen("/*JS*/", DESKTOP_JS, 1),
        // 手機: 底部分頁(總覽 / 指令 / 設定), 共用 apps / shell / system 的程式
        Device::Mobile => MOBILE_HTML
            .replacen("/*APPSCSS*/", APPS_CSS, 1)
            .replacen("/*SYSCSS*/", SYSTEM_CSS, 1)
            .replacen("/*MOBILECSS*/", MOBILE_CSS, 1)
            .replacen("/*APPSJS*/", APPS_JS, 1)
            .replacen("/*SYSJS*/", SYSTEM_JS, 1)
            .replacen("/*SHELLJS*/", SHELL_JS, 1)
            .replacen("/*MUSICCSS*/", MUSIC_CSS, 1)
            .replacen("/*MUSICJS*/", MUSIC_JS, 1)
            .replacen("/*WEATHERCSS*/", WEATHER_CSS, 1)
            .replacen("/*WEATHERJS*/", WEATHER_JS, 1)
            .replacen("/*WCCSS*/", WC_CSS, 1)
            .replacen("/*WCJS*/", WC_JS, 1)
            .replacen("/*MOBILEJS*/", MOBILE_JS, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
    const ANDROID: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36";
    const ANDROID_TABLET: &str = "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
    const WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

    #[test]
    fn detect() {
        assert_eq!(detect_device(None, None, IPHONE), Device::Mobile);
        assert_eq!(detect_device(None, None, ANDROID), Device::Mobile);
        assert_eq!(detect_device(None, None, ANDROID_TABLET), Device::Pc);
        assert_eq!(detect_device(None, None, WINDOWS), Device::Pc);
        assert_eq!(detect_device(None, None, ""), Device::Pc);
        // client hint 優先於 User-Agent
        assert_eq!(detect_device(None, Some("?1"), WINDOWS), Device::Mobile);
        assert_eq!(detect_device(None, Some("?0"), ANDROID), Device::Pc);
        // 網址參數優先於全部
        assert_eq!(
            detect_device(Some("a=1&device=pc"), Some("?1"), IPHONE),
            Device::Pc
        );
        assert_eq!(
            detect_device(Some("device=mobile"), None, WINDOWS),
            Device::Mobile
        );
        assert_eq!(
            detect_device(Some("device=xx"), None, IPHONE),
            Device::Mobile
        );
    }
}
