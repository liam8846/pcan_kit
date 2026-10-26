## 問題與修改後行為

說明觸發條件、使用者可見的改變與相關 issue。

## 驗證

- 受驗 PR head SHA：
- 已執行的檢查與結果：
- 尚未完成的驗證：

## 平台與硬體驗收

- 是否影響 PCAN、共用執行邏輯、相依或工具鏈：
- Windows 原生執行結果：
- PEAK 實機結果與環境：
- 不適用項目與原因：

需要人工驗收時，依 [驗收紀錄範本](https://github.com/liam8846/pcan_kit/blob/master/docs/acceptance-template.md)
記錄結果，完成前保持 Draft。交叉編譯、FakeTransport 與測試替身不能代替實機驗收。
驗收後新增提交時，更新受驗 SHA 並補驗，再核准 `hardware-acceptance`。
