# 變更紀錄

本檔案由 [git-cliff](https://git-cliff.org) 依 commit 訊息自動產生，請勿手動編輯。
版本號遵循 [SemVer](https://semver.org/lang/zh-TW/)；0.x 期間破壞性變更會提升 minor 版本。

## [0.2.4](https://github.com/liam8846/pcan_kit/releases/tag/v0.2.4) — 2026-09-19

### 新功能

- 新增 ActiveFeatures 區分能力契約並強化傳輸層關閉與排程器生命週期安全 ([#21](https://github.com/liam8846/pcan_kit/pull/21)) ([`0909279`](https://github.com/liam8846/pcan_kit/commit/09092796bb3ef1f43cb7f91fb2b6e0ec2d78d08b))

### 修正

- 簡化 enumerate 中的 map_or 以符合 clippy 規範 ([`94bf2b4`](https://github.com/liam8846/pcan_kit/commit/94bf2b4228eb55d374c0ac727cd72eb3d691026c))

### 相依套件

- bump thiserror from 2.0.19 to 2.0.20 ([`75fb86b`](https://github.com/liam8846/pcan_kit/commit/75fb86be095a5baf6a5c457e8fd2659300152b20))
- bump taiki-e/install-action from 2 to 2.85.4 ([`f621a0e`](https://github.com/liam8846/pcan_kit/commit/f621a0e2dd92c53e0cad53545ec1508e9035701b))
- bump actions/deploy-pages from 4 to 5 ([`e42eb87`](https://github.com/liam8846/pcan_kit/commit/e42eb87e2e0fbfd649a92b641efa10327e2d710b))
- bump softprops/action-gh-release from 2 to 3 ([`aced602`](https://github.com/liam8846/pcan_kit/commit/aced6028d4c6c3f64e309a4deb9a1b19384a393d))
- bump taiki-e/install-action from 2.85.4 to 2.87.1 ([`8d5372a`](https://github.com/liam8846/pcan_kit/commit/8d5372ac82e7a77f9d4b34e4d65dc513527c9917))
- bump bitflags from 2.13.1 to 2.13.2 ([`94df68e`](https://github.com/liam8846/pcan_kit/commit/94df68e17f9200df130196b3ed65dd88a99c4a79))

## [0.2.3](https://github.com/liam8846/pcan_kit/releases/tag/v0.2.3) — 2026-07-29

### 新功能

- Capabilities 新增錯誤幀與狀態幀能力回報 ([#14](https://github.com/liam8846/pcan_kit/pull/14)) ([`39b2334`](https://github.com/liam8846/pcan_kit/commit/39b233430a74844b5936ac1b87a6e21fbeb1762f))

## [0.2.2](https://github.com/liam8846/pcan_kit/releases/tag/v0.2.2) — 2026-07-29

### 新功能

- 新增 PCAN 與 SocketCAN 通道列舉 API ([`a5eda7e`](https://github.com/liam8846/pcan_kit/commit/a5eda7e74162c5fde001b59bbc481cfa4d2d1348))

### 修正

- 修正 PCAN-Basic 參數常數值避免開啟時誤設唯讀參數 ([`2c304f2`](https://github.com/liam8846/pcan_kit/commit/2c304f2ff5fc74559031f8b31174be1e47d3ed67))

## [0.2.1](https://github.com/liam8846/pcan_kit/releases/tag/v0.2.1) — 2026-07-29

### 新功能

- 新增週期陳舊酬載統計並補上描述字串零拷貝與事件排空落後處理 ([#10](https://github.com/liam8846/pcan_kit/pull/10)) ([`330e476`](https://github.com/liam8846/pcan_kit/commit/330e4765ac223636f1476f410ce0b9f50b4cd4a6))

### 修正

- 忽略長度不符的週期酬載更新以避免併發時越界 ([#7](https://github.com/liam8846/pcan_kit/pull/7)) ([`5a68a38`](https://github.com/liam8846/pcan_kit/commit/5a68a38eb1e726d64ac47b78bc4ad5b509a7b55c))
- 將 PCAN 阻塞式開啟移出非同步執行期並修正交易緩衝滿誤判為斷線 ([#8](https://github.com/liam8846/pcan_kit/pull/8)) ([`b22e126`](https://github.com/liam8846/pcan_kit/commit/b22e12644a5d701f4078f921d0e47dd92c2b00be))

### 測試

- 修正工作者遺失事件測試的訂閱競態 ([#9](https://github.com/liam8846/pcan_kit/pull/9)) ([`ce73664`](https://github.com/liam8846/pcan_kit/commit/ce7366458c0580e41e4eeec9d5afdda34c72cbfd))

### 維護

- hide CodeRabbit review details ([`7dc2317`](https://github.com/liam8846/pcan_kit/commit/7dc23179a066b78edc5d57fdad6086b32c8fb9f5))
- 移除 CodeRabbit 設定檔改用預設審查行為 ([#11](https://github.com/liam8846/pcan_kit/pull/11)) ([`366d94f`](https://github.com/liam8846/pcan_kit/commit/366d94f0d94199adaea6c88777b2ef5c4c5ba716))
- 將 workspace 版本升至 0.2.1 ([#12](https://github.com/liam8846/pcan_kit/pull/12)) ([`ed9bfb7`](https://github.com/liam8846/pcan_kit/commit/ed9bfb7ee6232777e3edf5f3518f601e93c69d93))

## [0.2.0](https://github.com/liam8846/pcan_kit/releases/tag/v0.2.0) — 2026-07-28

### 新功能

- 建立 workspace 骨架與 pcan-core 核心型別層 ([`7d02c70`](https://github.com/liam8846/pcan_kit/commit/7d02c70f13adaae611a347462adc6c3bb257a284))
- 新增 pcan-link 連線監督層 ([`229cd67`](https://github.com/liam8846/pcan_kit/commit/229cd678eff390ed48ec7f36aa6d9f78225259c7))
- 新增 PCAN-Basic 與 SocketCAN 後端、門面 crate 與繁中文件 ([`0f44431`](https://github.com/liam8846/pcan_kit/commit/0f4443179389b63f14f086b4a56cc37b5ebebab2))

### 修正

- 讓非 Linux 目標的 vcan 測試保留 crate 文件以修正 Windows clippy ([`8a43345`](https://github.com/liam8846/pcan_kit/commit/8a43345763d4d7d376c161d36ed3286504f4326e))
- 為背景任務加上異常結束守衛並修正連線釋放時的空轉與洩漏 ([`9d4e4e5`](https://github.com/liam8846/pcan_kit/commit/9d4e4e5d84c294421288ca99bdc1532b5489c26e))

### 文件

- 新增持續整合首次執行檢查清單 ([`3bf1d74`](https://github.com/liam8846/pcan_kit/commit/3bf1d7432e017af69210fe9fda3d72065901ca55))

### 測試

- 新增 vcan 整合測試並修正 MSRV 與授權檔 ([`d6dbe89`](https://github.com/liam8846/pcan_kit/commit/d6dbe8926414d66c1367d87e6014466cb476160f))

### 持續整合

- 建立 GitHub Actions 持續整合與發佈流程 ([`031dc2e`](https://github.com/liam8846/pcan_kit/commit/031dc2ea634c924444c9d09d928ca5975c1aa243))
- 升級 GitHub Actions 版本以消除 Node 20 淘汰警告 ([`ed9ae07`](https://github.com/liam8846/pcan_kit/commit/ed9ae076106a352efcf4b7f3c79754decbdaf5f7))
- 升級 libloading 至 0.9 ([`6e46897`](https://github.com/liam8846/pcan_kit/commit/6e4689797d2937ade72a9f4a37b3730ca60d855e))

### 維護

- 加入 LF 換行正規化並將內部檢查清單移出儲存庫 ([`0bb9e6f`](https://github.com/liam8846/pcan_kit/commit/0bb9e6fdb99725632381246df77796254130808c))
- 將 workspace 版本升至 0.2.0 ([`127cfe3`](https://github.com/liam8846/pcan_kit/commit/127cfe3c6e2a937f9cf669b281cd7be5d8d44a17))

