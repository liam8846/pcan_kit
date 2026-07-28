#![cfg_attr(not(target_os = "linux"), allow(missing_docs))]
#![cfg(target_os = "linux")]
//! 輕量、零配置熱路徑的 Linux `SocketCAN` 非同步後端。
//!
//! 本 crate 直接使用 `libc`，沒有採用 `socketcan 3.5`：後者即使只啟用
//! Tokio 仍引入 `nix`、`socket2`、`itertools`、`hex`、`log`、`nb`、
//! `embedded-can`、`mio`、`futures`、`bitflags` 與 `thiserror`。在工控
//! 產品的供應鏈稽核中，為少量 socket 系統呼叫承擔這些依賴不合理；其
//! 多型幀與錯誤語意也仍需再映射至 `pcan-core`。`libc` 已完整提供所需 ABI。

/// `SocketCAN` 錯誤幀純函式解析。
pub mod errframe;
mod socket;

pub use socket::{CanSocket, SocketCanConfig, SocketCanFactory};
