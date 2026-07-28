//! 直接取自 PCAN-Basic 公開標頭的通道、狀態、參數與位元率常數。

use crate::{TPCANBaudrate, TPCANHandle, TPCANMessageType, TPCANMode, TPCANParameter, TPCANStatus};

macro_rules! constant {
    ($(#[$meta:meta])* $name:ident: $ty:ty = $value:expr) => {
        $(#[$meta])*
        pub const $name: $ty = $value;
    };
}

constant!(/// 未指定任何 PCAN 通道。
PCAN_NONEBUS: TPCANHandle = 0x00);
constant!(/// ISA 通道 1。
PCAN_ISABUS1: TPCANHandle = 0x21);
constant!(/// ISA 通道 2。
PCAN_ISABUS2: TPCANHandle = 0x22);
constant!(/// ISA 通道 3。
PCAN_ISABUS3: TPCANHandle = 0x23);
constant!(/// ISA 通道 4。
PCAN_ISABUS4: TPCANHandle = 0x24);
constant!(/// ISA 通道 5。
PCAN_ISABUS5: TPCANHandle = 0x25);
constant!(/// ISA 通道 6。
PCAN_ISABUS6: TPCANHandle = 0x26);
constant!(/// ISA 通道 7。
PCAN_ISABUS7: TPCANHandle = 0x27);
constant!(/// ISA 通道 8。
PCAN_ISABUS8: TPCANHandle = 0x28);
constant!(/// Dongle 通道 1。
PCAN_DNGBUS1: TPCANHandle = 0x31);
constant!(/// PCI 通道 1。
PCAN_PCIBUS1: TPCANHandle = 0x41);
constant!(/// PCI 通道 2。
PCAN_PCIBUS2: TPCANHandle = 0x42);
constant!(/// PCI 通道 3。
PCAN_PCIBUS3: TPCANHandle = 0x43);
constant!(/// PCI 通道 4。
PCAN_PCIBUS4: TPCANHandle = 0x44);
constant!(/// PCI 通道 5。
PCAN_PCIBUS5: TPCANHandle = 0x45);
constant!(/// PCI 通道 6。
PCAN_PCIBUS6: TPCANHandle = 0x46);
constant!(/// PCI 通道 7。
PCAN_PCIBUS7: TPCANHandle = 0x47);
constant!(/// PCI 通道 8。
PCAN_PCIBUS8: TPCANHandle = 0x48);
constant!(/// PCI 通道 9。
PCAN_PCIBUS9: TPCANHandle = 0x409);
constant!(/// PCI 通道 10。
PCAN_PCIBUS10: TPCANHandle = 0x40a);
constant!(/// PCI 通道 11。
PCAN_PCIBUS11: TPCANHandle = 0x40b);
constant!(/// PCI 通道 12。
PCAN_PCIBUS12: TPCANHandle = 0x40c);
constant!(/// PCI 通道 13。
PCAN_PCIBUS13: TPCANHandle = 0x40d);
constant!(/// PCI 通道 14。
PCAN_PCIBUS14: TPCANHandle = 0x40e);
constant!(/// PCI 通道 15。
PCAN_PCIBUS15: TPCANHandle = 0x40f);
constant!(/// PCI 通道 16。
PCAN_PCIBUS16: TPCANHandle = 0x410);
constant!(/// USB 通道 1。
PCAN_USBBUS1: TPCANHandle = 0x51);
constant!(/// USB 通道 2。
PCAN_USBBUS2: TPCANHandle = 0x52);
constant!(/// USB 通道 3。
PCAN_USBBUS3: TPCANHandle = 0x53);
constant!(/// USB 通道 4。
PCAN_USBBUS4: TPCANHandle = 0x54);
constant!(/// USB 通道 5。
PCAN_USBBUS5: TPCANHandle = 0x55);
constant!(/// USB 通道 6。
PCAN_USBBUS6: TPCANHandle = 0x56);
constant!(/// USB 通道 7。
PCAN_USBBUS7: TPCANHandle = 0x57);
constant!(/// USB 通道 8。
PCAN_USBBUS8: TPCANHandle = 0x58);
constant!(/// USB 通道 9。
PCAN_USBBUS9: TPCANHandle = 0x509);
constant!(/// USB 通道 10。
PCAN_USBBUS10: TPCANHandle = 0x50a);
constant!(/// USB 通道 11。
PCAN_USBBUS11: TPCANHandle = 0x50b);
constant!(/// USB 通道 12。
PCAN_USBBUS12: TPCANHandle = 0x50c);
constant!(/// USB 通道 13。
PCAN_USBBUS13: TPCANHandle = 0x50d);
constant!(/// USB 通道 14。
PCAN_USBBUS14: TPCANHandle = 0x50e);
constant!(/// USB 通道 15。
PCAN_USBBUS15: TPCANHandle = 0x50f);
constant!(/// USB 通道 16。
PCAN_USBBUS16: TPCANHandle = 0x510);
constant!(/// PCC 通道 1。
PCAN_PCCBUS1: TPCANHandle = 0x61);
constant!(/// PCC 通道 2。
PCAN_PCCBUS2: TPCANHandle = 0x62);
constant!(/// LAN 通道 1。
PCAN_LANBUS1: TPCANHandle = 0x801);
constant!(/// LAN 通道 2。
PCAN_LANBUS2: TPCANHandle = 0x802);
constant!(/// LAN 通道 3。
PCAN_LANBUS3: TPCANHandle = 0x803);
constant!(/// LAN 通道 4。
PCAN_LANBUS4: TPCANHandle = 0x804);
constant!(/// LAN 通道 5。
PCAN_LANBUS5: TPCANHandle = 0x805);
constant!(/// LAN 通道 6。
PCAN_LANBUS6: TPCANHandle = 0x806);
constant!(/// LAN 通道 7。
PCAN_LANBUS7: TPCANHandle = 0x807);
constant!(/// LAN 通道 8。
PCAN_LANBUS8: TPCANHandle = 0x808);
constant!(/// LAN 通道 9。
PCAN_LANBUS9: TPCANHandle = 0x809);
constant!(/// LAN 通道 10。
PCAN_LANBUS10: TPCANHandle = 0x80a);
constant!(/// LAN 通道 11。
PCAN_LANBUS11: TPCANHandle = 0x80b);
constant!(/// LAN 通道 12。
PCAN_LANBUS12: TPCANHandle = 0x80c);
constant!(/// LAN 通道 13。
PCAN_LANBUS13: TPCANHandle = 0x80d);
constant!(/// LAN 通道 14。
PCAN_LANBUS14: TPCANHandle = 0x80e);
constant!(/// LAN 通道 15。
PCAN_LANBUS15: TPCANHandle = 0x80f);
constant!(/// LAN 通道 16。
PCAN_LANBUS16: TPCANHandle = 0x810);

constant!(/// 操作成功。
PCAN_ERROR_OK: TPCANStatus = 0x00000);
constant!(/// 控制器傳送緩衝區已滿。
PCAN_ERROR_XMTFULL: TPCANStatus = 0x00001);
constant!(/// 控制器接收溢位。
PCAN_ERROR_OVERRUN: TPCANStatus = 0x00002);
constant!(/// 匯流排錯誤計數升高。
PCAN_ERROR_BUSLIGHT: TPCANStatus = 0x00004);
constant!(/// 匯流排進入警告區。
PCAN_ERROR_BUSHEAVY: TPCANStatus = 0x00008);
constant!(/// `BUSHEAVY` 的新版別名。
PCAN_ERROR_BUSWARNING: TPCANStatus = PCAN_ERROR_BUSHEAVY);
constant!(/// 控制器進入 Bus-Off。
PCAN_ERROR_BUSOFF: TPCANStatus = 0x00010);
constant!(/// 任意匯流排錯誤的組合遮罩。
PCAN_ERROR_ANYBUSERR: TPCANStatus = 0x0041c);
constant!(/// 接收佇列為空。
PCAN_ERROR_QRCVEMPTY: TPCANStatus = 0x00020);
constant!(/// 接收佇列溢位。
PCAN_ERROR_QOVERRUN: TPCANStatus = 0x00040);
constant!(/// 傳送佇列已滿。
PCAN_ERROR_QXMTFULL: TPCANStatus = 0x00080);
constant!(/// 硬體登錄測試失敗。
PCAN_ERROR_REGTEST: TPCANStatus = 0x00100);
constant!(/// 找不到驅動程式。
PCAN_ERROR_NODRIVER: TPCANStatus = 0x00200);
constant!(/// 硬體已由其他客戶端使用。
PCAN_ERROR_HWINUSE: TPCANStatus = 0x00400);
constant!(/// 網路資源已由其他客戶端使用。
PCAN_ERROR_NETINUSE: TPCANStatus = 0x00800);
constant!(/// 硬體代碼無效。
PCAN_ERROR_ILLHW: TPCANStatus = 0x01400);
constant!(/// 網路代碼無效。
PCAN_ERROR_ILLNET: TPCANStatus = 0x01800);
constant!(/// 客戶端代碼無效。
PCAN_ERROR_ILLCLIENT: TPCANStatus = 0x01c00);
constant!(/// 通道控制代碼無效。
PCAN_ERROR_ILLHANDLE: TPCANStatus = 0x01c00);
constant!(/// 驅動資源不足。
PCAN_ERROR_RESOURCE: TPCANStatus = 0x02000);
constant!(/// 參數種類無效。
PCAN_ERROR_ILLPARAMTYPE: TPCANStatus = 0x04000);
constant!(/// 參數值無效。
PCAN_ERROR_ILLPARAMVAL: TPCANStatus = 0x08000);
constant!(/// 未知錯誤。
PCAN_ERROR_UNKNOWN: TPCANStatus = 0x10000);
constant!(/// 資料內容無效。
PCAN_ERROR_ILLDATA: TPCANStatus = 0x20000);
constant!(/// 控制器進入 error-passive。
PCAN_ERROR_BUSPASSIVE: TPCANStatus = 0x40000);
constant!(/// 操作模式無效。
PCAN_ERROR_ILLMODE: TPCANStatus = 0x80000);
constant!(/// 操作成功但需注意附帶狀況。
PCAN_ERROR_CAUTION: TPCANStatus = 0x0200_0000);
constant!(/// 通道尚未初始化。
PCAN_ERROR_INITIALIZE: TPCANStatus = 0x0400_0000);
constant!(/// 此操作在目前狀態不合法。
PCAN_ERROR_ILLOPERATION: TPCANStatus = 0x0800_0000);

constant!(/// 標準資料幀。
PCAN_MESSAGE_STANDARD: TPCANMessageType = 0x00);
constant!(/// 遠端請求幀。
PCAN_MESSAGE_RTR: TPCANMessageType = 0x01);
constant!(/// 擴充識別碼幀。
PCAN_MESSAGE_EXTENDED: TPCANMessageType = 0x02);
constant!(/// CAN FD 幀。
PCAN_MESSAGE_FD: TPCANMessageType = 0x04);
constant!(/// CAN FD 位元率切換。
PCAN_MESSAGE_BRS: TPCANMessageType = 0x08);
constant!(/// CAN FD error-state indicator。
PCAN_MESSAGE_ESI: TPCANMessageType = 0x10);
constant!(/// 本機傳送回音幀。
PCAN_MESSAGE_ECHO: TPCANMessageType = 0x20);
constant!(/// 錯誤幀。
PCAN_MESSAGE_ERRFRAME: TPCANMessageType = 0x40);
constant!(/// 狀態幀。
PCAN_MESSAGE_STATUS: TPCANMessageType = 0x80);

constant!(/// 裝置識別碼參數。
PCAN_DEVICE_ID: TPCANParameter = 0x01);
constant!(/// 五伏特電源參數。
PCAN_5VOLTS_POWER: TPCANParameter = 0x02);
constant!(/// 接收事件參數。
PCAN_RECEIVE_EVENT: TPCANParameter = 0x03);
constant!(/// 訊息過濾器參數。
PCAN_MESSAGE_FILTER: TPCANParameter = 0x04);
constant!(/// API 版本參數。
PCAN_API_VERSION: TPCANParameter = 0x05);
constant!(/// 通道版本參數。
PCAN_CHANNEL_VERSION: TPCANParameter = 0x06);
constant!(/// Bus-Off 自動復歸參數。
PCAN_BUSOFF_AUTORESET: TPCANParameter = 0x07);
constant!(/// 唯聽模式參數。
PCAN_LISTEN_ONLY: TPCANParameter = 0x08);
constant!(/// 記錄檔路徑參數。
PCAN_LOG_LOCATION: TPCANParameter = 0x09);
constant!(/// 記錄功能狀態參數。
PCAN_LOG_STATUS: TPCANParameter = 0x0a);
constant!(/// 記錄功能設定參數。
PCAN_LOG_CONFIGURE: TPCANParameter = 0x0b);
constant!(/// 寫入記錄文字參數。
PCAN_LOG_TEXT: TPCANParameter = 0x0c);
constant!(/// 通道狀況參數。
PCAN_CHANNEL_CONDITION: TPCANParameter = 0x0d);
constant!(/// 接收狀態參數。
PCAN_RECEIVE_STATUS: TPCANParameter = 0x0f);
constant!(/// 控制器編號參數。
PCAN_CONTROLLER_NUMBER: TPCANParameter = 0x10);
constant!(/// 11-bit 接受過濾器參數。
PCAN_ACCEPTANCE_FILTER_11BIT: TPCANParameter = 0x14);
constant!(/// 29-bit 接受過濾器參數。
PCAN_ACCEPTANCE_FILTER_29BIT: TPCANParameter = 0x15);
constant!(/// 允許狀態幀參數。
PCAN_ALLOW_STATUS_FRAMES: TPCANParameter = 0x18);
constant!(/// 允許 RTR 幀參數。
PCAN_ALLOW_RTR_FRAMES: TPCANParameter = 0x19);
constant!(/// 允許錯誤幀參數。
PCAN_ALLOW_ERROR_FRAMES: TPCANParameter = 0x1a);
constant!(/// 位元率資訊參數。
PCAN_BITRATE_INFO: TPCANParameter = 0x1d);
constant!(/// 允許回音幀參數。
PCAN_ALLOW_ECHO_FRAMES: TPCANParameter = 0x1e);

constant!(/// 關閉布林參數。
PCAN_PARAMETER_OFF: u32 = 0x00);
constant!(/// 開啟布林參數。
PCAN_PARAMETER_ON: u32 = 0x01);
constant!(/// 關閉所有硬體過濾。
PCAN_FILTER_CLOSE: u32 = 0);
constant!(/// 開放所有硬體過濾。
PCAN_FILTER_OPEN: u32 = 1);
constant!(/// 使用自訂硬體過濾。
PCAN_FILTER_CUSTOM: u32 = 2);
constant!(/// 標準識別碼過濾模式。
PCAN_MODE_STANDARD: TPCANMode = 0x00);
constant!(/// 擴充識別碼過濾模式。
PCAN_MODE_EXTENDED: TPCANMode = 0x02);

constant!(/// 古典 CAN 1 Mbit/s BTR0BTR1。
PCAN_BAUD_1M: TPCANBaudrate = 0x0014);
constant!(/// 古典 CAN 800 kbit/s BTR0BTR1。
PCAN_BAUD_800K: TPCANBaudrate = 0x0016);
constant!(/// 古典 CAN 500 kbit/s BTR0BTR1。
PCAN_BAUD_500K: TPCANBaudrate = 0x001c);
constant!(/// 古典 CAN 250 kbit/s BTR0BTR1。
PCAN_BAUD_250K: TPCANBaudrate = 0x011c);
constant!(/// 古典 CAN 125 kbit/s BTR0BTR1。
PCAN_BAUD_125K: TPCANBaudrate = 0x031c);
constant!(/// 古典 CAN 100 kbit/s BTR0BTR1。
PCAN_BAUD_100K: TPCANBaudrate = 0x432f);
constant!(/// 古典 CAN 95.238 kbit/s BTR0BTR1。
PCAN_BAUD_95K: TPCANBaudrate = 0xc34e);
constant!(/// 古典 CAN 83.333 kbit/s BTR0BTR1。
PCAN_BAUD_83K: TPCANBaudrate = 0x852b);
constant!(/// 古典 CAN 50 kbit/s BTR0BTR1。
PCAN_BAUD_50K: TPCANBaudrate = 0x472f);
constant!(/// 古典 CAN 47.619 kbit/s BTR0BTR1。
PCAN_BAUD_47K: TPCANBaudrate = 0x1414);
constant!(/// 古典 CAN 33.333 kbit/s BTR0BTR1。
PCAN_BAUD_33K: TPCANBaudrate = 0x8b2f);
constant!(/// 古典 CAN 20 kbit/s BTR0BTR1。
PCAN_BAUD_20K: TPCANBaudrate = 0x532f);
constant!(/// 古典 CAN 10 kbit/s BTR0BTR1。
PCAN_BAUD_10K: TPCANBaudrate = 0x672f);
constant!(/// 古典 CAN 5 kbit/s BTR0BTR1。
PCAN_BAUD_5K: TPCANBaudrate = 0x7f7f);
