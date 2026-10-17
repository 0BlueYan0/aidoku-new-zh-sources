# WEBTOON 繁體中文

第 18 版以 Android 3.9.13 使用的 App API 取得完整章節清單，包含網頁沒有列出的 MANGA 集數與搶先看。保留原有網頁探索功能與章節主鍵。

支援免費／試閱內容。MANGA 使用 App 的圖片 token 與實際閱讀方向；購買、廣告與限時解鎖需使用官方 App。本版不支援登入或讀取帳號已購買的內容，這些章節會標示並鎖定。

## 驗證

在本目錄執行：

```powershell
cargo test --locked
cargo test --locked --features live-tests
cargo clippy --locked -- -D warnings
cargo clippy --locked --tests --features live-tests -- -D warnings
```

`live-tests` 會向 WEBTOON 發送唯讀請求，驗證完整清單、一般條漫閱讀、MANGA 試閱圖片及章節主鍵保存。離線測試使用 2026-10-17 擷取的匿名回應，不包含簽章、Cookie 或帳號資訊。

在專案根目錄打包與驗證：

```powershell
aidoku package sources/zh.webtoons
aidoku verify sources/zh.webtoons/package.aix
```

最低 Aidoku 版本為 0.8.3。iOS 實機仍須確認詳情清單、由右至左閱讀、付費標示、設定說明、既有書庫閱讀進度與重新開啟試閱後的圖片載入。

## 實作依據

- App API 請求使用 HMAC-SHA1；URL 前 255 字元加毫秒時間戳，結果經 Base64 與 URL 編碼。簽章 key 來自使用者提供的 APK 公開內嵌資源。
- `titleHomeEpisodeListV3` 的 `pageSize` 必須為 30；每批最多同時送出 5 個分頁請求。
- 以 `titleInfo.totalEpisodeCount` 驗證完整性；任何分頁失敗、重複或數量不符都回傳錯誤，避免以不完整清單覆蓋書庫。
- 網頁章節 URL 與新 App 章節 ID 的對應保存於來源設定，避免搶先看轉為免費或作品 URL 改名時改變主鍵。圖片 token 不寫入來源設定，每次重新開啟頁面清單時更新。
