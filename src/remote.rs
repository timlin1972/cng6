//! 遠端 CLI: 網頁送來的指令請求。
//!
//! 網頁伺服器在自己的 thread 收到請求後, 透過 channel 交給主迴圈, 由 `App` 用和 TUI
//! 相同的指令樹執行。每個遠端連線(session)有自己的層次, 輸出只回給該連線。

use std::sync::mpsc;

use tokio::sync::oneshot;

use crate::plugin::{Level, SettingRow};

/// 請求種類, 對應 TUI 的 Enter / Tab / `?`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Exec,
    Complete,
    Describe,
    /// 取得所有設定(網頁 Settings 視窗)
    Settings,
}

pub struct Request {
    /// 網頁端產生的連線識別
    pub session: String,
    /// 來源 IP(記錄到 TUI 的 Message 用)
    pub peer: String,
    pub op: Op,
    /// Exec: 整行指令; Complete / Describe: 游標前的文字
    pub line: String,
    pub reply: oneshot::Sender<Reply>,
}

#[derive(Debug, Default)]
pub struct Reply {
    pub output: Vec<(Level, String)>,
    /// 執行後所在的層次
    pub path: Vec<String>,
    /// Tab 補完後, 游標前文字要換成這個
    pub replacement: Option<String>,
    /// Op::Settings 的結果
    pub settings: Option<Vec<SettingRow>>,
}

pub type Sender = mpsc::Sender<Request>;
pub type Receiver = mpsc::Receiver<Request>;

pub fn channel() -> (Sender, Receiver) {
    mpsc::channel()
}
