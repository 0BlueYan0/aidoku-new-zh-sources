# 來源更新紀錄

各來源新增的功能、修正的問題與版本變更。

日期是該筆紀錄寫入的日期，與合併日可能差一兩天，準確的合併時間見相關紀錄的 PR。舊來源 ID 的更新也收錄在現行來源下。

## 來源索引

目前收錄 20 個來源。點選來源名稱可查看歷次更新，各來源表格的第一列就是目前版本。

| 來源 | 來源 ID |
|---|---|
| [BOOK☆WALKER 台灣](#zhbookwalker) | `zh.bookwalker` |
| [CCC追漫台](#zhcreativecomic) | `zh.creativecomic` |
| [Webtoons 繁體中文](#zhwebtoons) | `zh.webtoons` |
| [Komiic 漫畫 (new-zh)](#zhkomiic2) | `zh.komiic2` |
| [TIBIU 漫畫在線](#zhtibiu) | `zh.tibiu` |
| [紳士漫畫 (new-zh)](#zhwnacg2) | `zh.wnacg2` |
| [喜漫漫畫](#zhfavcomic) | `zh.favcomic` |
| [嬉皮漫畫](#zhhipmh) | `zh.hipmh` |
| [輕書架](#zhlightnovel) | `zh.lightnovel` |
| [18漫畫](#zh18mh) | `zh.18mh` |
| [漫畫狗](#zhdogemanga) | `zh.dogemanga` |
| [HO5HO](#zhho5ho) | `zh.ho5ho` |
| [漫小肆韓漫](#zhmsxhm) | `zh.msxhm` |
| [vomic漫](#zhvomicmh) | `zh.vomicmh` |
| [嗶哩輕小說](#zhlinovelib) | `zh.linovelib` |
| [漫網](#zhmanwang) | `zh.manwang` |
| [巴卡漫畫 (new-zh)](#zhbakamh2) | `zh.bakamh2` |
| [漫畫1234](#zhwmh1234) | `zh.wmh1234` |
| [漫畫客](#zhyumanhua) | `zh.yumanhua` |
| [W漫畫](#zhwmanhua) | `zh.wmanhua` |

## 各來源更新

各來源由新到舊排列。

<a id="zhbookwalker"></a>

### BOOK☆WALKER 台灣

`zh.bookwalker`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v2 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-09-23 | v1 | 新增來源。標記已封存集數，修正同一本書重新開啟時的續期與 991 錯誤。 | [PR #9][pr9] |

<a id="zhcreativecomic"></a>

### CCC追漫台

`zh.creativecomic`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v2 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-09-22 | v1 | 新增來源。支援網頁登入、帳號資訊、金幣／點數顯示與自動續期。免費章節不再誤標付費，章節標題說明購買途徑。修正下拉刷新卡住、設定頁按鈕失效、清除登入與離線誤判。 | [PR #8][pr8] |

<a id="zhwebtoons"></a>

### Webtoons 繁體中文

`zh.webtoons`

舊 ID 為 `zh-hant.webtoons`。

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v19 | 改善載入失敗時的處理。篩選器補標題，移除沒有對應列表的「全部」，修正部分搜尋文字無法正確查詢。 | [PR #33][pr33] |
| 2026-10-17 | v18 | 補上 App 章節清單與免費閱讀、官方網頁登入及已購章節閱讀。修正登入狀態、章節標題與圖片相容性，保留公開網頁閱讀的備援路徑。 | [PR #32][pr32] |
| 2026-10-16 | v17 | 首頁各列同時載入，加快內容與封面出現。 | [PR #30][pr30] |
| 2026-09-15 | v16 | 修正來源圖示與套件相容性。 | [8ca5c70][8ca5c70] |
| 2026-03-19 | v15 | 來源 ID 從 `zh-hant.webtoons` 改為 `zh.webtoons`。 | [2fa3901][2fa3901] |
| 2026-03-18 | v14 | 首頁先顯示各列再逐列載入。不再讀到快取的舊資料。 | [547361e][547361e] |
| 2026-03-12 | v12 | 改善章節無法載入時的錯誤處理。 | [PR #1][pr1] |
| 2026-02-26 | v11 | 新增繁體中文來源。 | [59dde24][59dde24] |

<a id="zhkomiic2"></a>

### Komiic 漫畫 (new-zh)

`zh.komiic2`

舊 ID 為 `zh.komiic`。

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v2 | 改善載入失敗時的處理。修正分類篩選，並補上篩選器標題。顯示名稱加上 `(new-zh)`。 | [PR #33][pr33] |
| 2026-10-17 | v1（新 ID） | 改為 `zh.komiic2`，避免與官方來源互相覆蓋，版號重新開始。舊來源須移除後重新安裝，書庫、進度與登入不自動搬移。 | [PR #31][pr31] |
| 2026-10-16 | 舊 ID v13 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-09-22 | 舊 ID v12 | 修正設定頁無法操作，統一帳號群組位置與說明文字。 | [PR #8][pr8] |
| 2026-09-17 | 舊 ID v11 | 保留登入狀態，登入過期時自動重新登入。登入被拒後停止反覆嘗試，減少無法登入時的等待。 | [PR #4][pr4] |
| 2026-09-15 | 舊 ID v10 | 修正來源圖示與套件相容性。 | [8ca5c70][8ca5c70] |
| 2026-03-18 | 舊 ID v9 | 顯示名稱改回「Komiic 漫畫」。首頁先顯示各列再逐列載入。不再讀到快取的舊資料。 | [547361e][547361e] |
| 2026-03-14 | 舊 ID v7 | 新增 Komiic 來源。 | [PR #2][pr2] |

<a id="zhtibiu"></a>

### TIBIU 漫畫在線

`zh.tibiu`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v6 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-09-18 | v5 | 新增來源，以訪客身分使用。修正部分簡介造成列表消失，章節依章節號排序，預設條漫閱讀方向，介面文字改繁體。 | [PR #5][pr5] |

<a id="zhwnacg2"></a>

### 紳士漫畫 (new-zh)

`zh.wnacg2`

舊 ID 為 `zh.wnacg`。

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-11-03 | v5 | 新增「AI 翻譯」設定（預設關閉，須登入）：日語的同人誌、單行本、雜誌可在閱讀時顯示網站已翻好的繁體或簡體譯文。 | [PR #46][pr46] |
| 2026-11-01 | v4 | 新增兩個鏡像網域。列表不再重複列出合集內的單話。 | [PR #42][pr42] |
| 2026-10-30 | v3 | 支援網頁登入後閱讀韓漫，韓漫合集依章節呈現。 | [PR #39][pr39] |
| 2026-10-20 | v2 | 改善載入失敗時的處理。顯示名稱加上 `(new-zh)`。 | [PR #33][pr33] |
| 2026-10-17 | v1（新 ID） | 改為 `zh.wnacg2`，避免與官方來源互相覆蓋，版號重新開始。舊來源須移除後重新安裝，書庫、進度與登入不自動搬移。 | [PR #31][pr31] |
| 2026-10-16 | 舊 ID v6 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-09-18 | 舊 ID v5 | 新增來源。修正部分作品閱讀方向錯誤。 | [PR #6][pr6] |

<a id="zhfavcomic"></a>

### 喜漫漫畫

`zh.favcomic`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v5 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v4 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-10-09 | v3 | 改善作品分級判斷與登入狀態自動更新，減少反覆登入。 | [PR #20][pr20] |
| 2026-09-22 | v2 | 修正設定頁失效，統一帳號群組與文案。改善裝置數上限及登入失敗提示，裝置額滿時停止重試。修正登出後設定刷新、帳號資訊更新與登出逾時。 | [PR #8][pr8] |
| 2026-09-18 | v1 | 新增來源。 | [PR #7][pr7] |

<a id="zhhipmh"></a>

### 嬉皮漫畫

`zh.hipmh`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-11-02 | v3 | 修正最新章節未更新。 | [PR #44][pr44] |
| 2026-10-20 | v2 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-09-24 | v1 | 新增來源。 | [PR #10][pr10] |

<a id="zhlightnovel"></a>

### 輕書架

`zh.lightnovel`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v2 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-01 | v1 | 新增來源，支援自動簽到。 | [PR #11][pr11] |

<a id="zh18mh"></a>

### 18漫畫

`zh.18mh`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v4 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-15 | v3 | 支援自架 Cloudflare 轉發伺服器。 | [PR #28][pr28] |
| 2026-10-02 | v2 | 新增來源。 | [PR #12][pr12] |

<a id="zhdogemanga"></a>

### 漫畫狗

`zh.dogemanga`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v2 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-03 | v1 | 新增來源。 | [PR #13][pr13] |

<a id="zhho5ho"></a>

### HO5HO

`zh.ho5ho`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v4 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v3 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-10-03 | v2 | 新增來源。 | [PR #14][pr14] |

<a id="zhmsxhm"></a>

### 漫小肆韓漫

`zh.msxhm`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v9 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v8 | 書庫封面跟著設定頁所選網域載入。首頁各列同時載入。 | [PR #30][pr30] |
| 2026-10-03 | v7 | 新增來源。 | [PR #15][pr15] |

<a id="zhvomicmh"></a>

### vomic漫

`zh.vomicmh`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v4 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v3 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-10-03 | v2 | 新增來源。 | [PR #16][pr16] |

<a id="zhlinovelib"></a>

### 嗶哩輕小說

`zh.linovelib`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v3 | 改善載入失敗時的處理。標示站方未開放的章節，開啟時顯示錯誤提示，修正多題材篩選。將推薦／鮮花的期間排序改成更明確的名稱。 | [PR #33][pr33] |
| 2026-10-16 | v2 | 書庫封面跟著設定頁所選網域載入。 | [PR #30][pr30] |
| 2026-10-04 | v1 | 新增來源。 | [PR #17][pr17] |

<a id="zhmanwang"></a>

### 漫網

`zh.manwang`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v3 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v2 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-10-09 | v1 | 新增來源。 | [PR #19][pr19] |

<a id="zhbakamh2"></a>

### 巴卡漫畫 (new-zh)

`zh.bakamh2`

舊 ID 為 `zh.bakamh`。

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-30 | v5 | 修正下載或連續閱讀後章節變成表情圖。 | [PR #41][pr41] |
| 2026-10-29 | v4 | 修正閱讀頁圖片被換成站方阻擋圖。 | [PR #37][pr37] |
| 2026-10-28 | v3 | 修正站方改版後首頁與搜尋無法顯示作品。 | [PR #36][pr36] |
| 2026-10-20 | v2 | 改善載入失敗時的處理。顯示名稱加上 `(new-zh)`。 | [PR #33][pr33] |
| 2026-10-17 | v1（新 ID） | 改為 `zh.bakamh2`，避免與官方來源互相覆蓋，版號重新開始。舊來源須移除後重新安裝，書庫、進度與登入不自動搬移。 | [PR #31][pr31] |
| 2026-10-16 | 舊 ID v6 | 書庫封面跟著所選網域載入。改善首頁載入速度及空白搜尋造成來源卡住的問題。 | [PR #30][pr30] |
| 2026-10-15 | 舊 ID v5 | 支援自架 Cloudflare 轉發伺服器。 | [PR #28][pr28] |
| 2026-10-13 | 舊 ID v4 | 改善閱讀、搜尋與列表遇到 Cloudflare 驗證時的處理。 | [PR #26][pr26] |
| 2026-10-10 | 舊 ID v3 | 新增來源。 | [PR #23][pr23] |

<a id="zhwmh1234"></a>

### 漫畫1234

`zh.wmh1234`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-24 | v4 | 修正站方改版後讀不到章節，並自動遷移以保留既有閱讀記錄。 | [PR #35][pr35] |
| 2026-10-20 | v3 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v2 | 首頁各列同時載入。 | [PR #30][pr30] |
| 2026-10-10 | v1 | 新增來源。 | [PR #24][pr24] |

<a id="zhyumanhua"></a>

### 漫畫客

`zh.yumanhua`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-10-20 | v3 | 改善載入失敗時的處理，詳見[共通更新](#共通更新)。 | [PR #33][pr33] |
| 2026-10-16 | v2 | 修正 App 自動搜尋時來源長時間卡住的問題。 | [PR #30][pr30] |
| 2026-10-13 | v1 | 新增來源。 | [PR #27][pr27] |

<a id="zhwmanhua"></a>

### W漫畫

`zh.wmanhua`

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-11-03 | v1 | 新增來源。 | [PR #52][pr52] |

## 共通更新

### 2026-10-20：改善載入失敗與內容不完整時的處理

- 改善各來源的錯誤提示，遇到缺頁、缺圖或空白內容時回報失敗，避免把不完整內容當成完整結果。
- 改善載入失敗時的處理，避免既有作品詳情與章節清單被錯誤結果覆蓋。
- 各來源依實際情況調整，版本及個別修正見上方來源紀錄。

相關紀錄：[PR #33][pr33]。

## 已移除來源

### koobone（multi.koobone）

| 日期 | 版本 | 更新內容 | 相關紀錄 |
|---|---|---|---|
| 2026-02-26 | 已移除 | 移除未完成的來源。 | [1c4c6e9][1c4c6e9] |
| 2026-02-26 | v1（未完成） | 加入 koobone 來源，尚未完成。 | [25f2907][25f2907] |

[1c4c6e9]: https://github.com/0BlueYan0/aidoku-new-zh-sources/commit/1c4c6e95e67770e36ca20cba0b6bd888e77c6a7f
[25f2907]: https://github.com/0BlueYan0/aidoku-new-zh-sources/commit/25f2907642d7925c10e22d82dfef5a4487b4d763
[2fa3901]: https://github.com/0BlueYan0/aidoku-new-zh-sources/commit/2fa3901b6170b05f58e6e8d555ed62f8f145b71d
[547361e]: https://github.com/0BlueYan0/aidoku-new-zh-sources/commit/547361e3eaedf27a10b30eaea054bddf992ca616
[59dde24]: https://github.com/0BlueYan0/aidoku-new-zh-sources/commit/59dde2463f36f2d4aa1f6c102ac21aa4be4b1d64
[8ca5c70]: https://github.com/0BlueYan0/aidoku-new-zh-sources/commit/8ca5c70cb8267af66ec7e7d242371773a2b1744c
[pr1]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/1
[pr10]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/10
[pr11]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/11
[pr12]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/12
[pr13]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/13
[pr14]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/14
[pr15]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/15
[pr16]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/16
[pr17]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/17
[pr19]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/19
[pr2]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/2
[pr20]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/20
[pr23]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/23
[pr24]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/24
[pr26]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/26
[pr27]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/27
[pr28]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/28
[pr30]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/30
[pr31]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/31
[pr32]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/32
[pr33]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/33
[pr35]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/35
[pr36]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/36
[pr37]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/37
[pr39]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/39
[pr4]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/4
[pr41]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/41
[pr42]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/42
[pr44]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/44
[pr46]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/46
[pr5]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/5
[pr52]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/52
[pr6]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/6
[pr7]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/7
[pr8]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/8
[pr9]: https://github.com/0BlueYan0/aidoku-new-zh-sources/pull/9
