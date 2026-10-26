# Windows／PEAK 驗收紀錄

將以下紀錄貼入 PR 描述或留言，完成後再核准 `hardware-acceptance`。

| 項目 | 紀錄 |
|---|---|
| PR | |
| 完整 PR head SHA | |
| 日期與驗收人 | |
| Windows／Linux 版本與架構 | |
| Rust 工具鏈 | |
| PEAK 裝置型號與通道 | |
| 驅動版本 | |
| PCAN-Basic DLL／so 版本 | |
| nominal／data bitrate | |
| 對端設備、接線與終端配置 | |

- [ ] Windows 原生 build、test 及範例執行通過。
- [ ] 載入實際 DLL／so，正常裝置與不存在通道的行為正確。
- [ ] 標準／擴充 ID、RTR、payload、時間戳及 echo 收發正確。
- [ ] CAN FD 各合法長度與 BRS 正確，或明列不適用原因與未驗證範圍。
- [ ] 反覆開啟、接收中關閉、重複關閉與重新開啟正常。
- [ ] 拔插、bus-off 與重連後設定重放正常。
- [ ] 記錄負載測試時間、丟幀／溢位計數及結果。

附上執行指令、摘要或日誌連結。失敗、未測或不適用的項目各自寫明，不能留白當通過。
若 head SHA 改變，先記錄新版與變更範圍，補驗受影響項目後才重新核准。
