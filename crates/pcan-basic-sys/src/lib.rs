//! PCAN-Basic 的固定寬度 FFI 型別、C 結構版面與執行期載入器。
//!
//! 本 crate 不做編譯期連結；程式只在開啟 PCAN 通道時載入原廠函式庫。

/// PCAN-Basic 動態函式庫介面。
pub mod api;
/// PCAN-Basic 公開常數。
pub mod consts;
/// PCAN-Basic 狀態分類。
pub mod status;
/// PCAN-Basic C 結構。
pub mod structs;
/// PCAN-Basic 純量型別。
pub mod types;

pub use api::{PcanApi, load, load_from};
pub use consts::*;
pub use status::{StatusOutcome, bus_state_of, classify, warnings_of};
pub use structs::{TPCANMsg, TPCANMsgFD, TPCANTimestamp};
pub use types::*;
