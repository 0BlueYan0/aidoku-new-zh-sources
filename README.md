# aidoku-new-zh-sources
Aidoku 新版中文圖源<br>
舊版圖源：https://github.com/suiyuran/aidoku-zh-sources <br>
作者已經加入警告文字說明舊版 source 未來會無法使用<br>

## 如何使用

進入[這裡](https://0blueyan0.github.io/aidoku-new-zh-sources)並點擊 Add Repository

## Cloudflare 驗證轉發伺服器

巴卡漫畫、18漫畫整站都有 Cloudflare 驗證，App 常常要驗證好幾次才打得開。有一台一直開著、裝了 Docker 的電腦或 NAS 的話，可以架設 [aidoku-cf-relay](https://github.com/0BlueYan0/aidoku-cf-relay) 轉發伺服器：

- 架好後，在這兩個圖源設定頁的「轉發伺服器」填入網址與密碼，所有頁面都改由它開啟，若運作正常是不會跳出驗證頁面，當然這跟IP品質有關係，速度上也要看轉發伺服器和原站之間的連線速度。
- 架設方法見 aidoku-cf-relay 的說明。在家裡以外的地方使用時，建議搭配 [Tailscale](https://tailscale.com/)。
- 不架設也能照常使用，欄位留空就是原本的連線方式。

## 圖源列表

> [!NOTE]
> Komiic 漫畫、紳士漫畫、巴卡漫畫在 2026-10-17 改了圖源 id，避免和 Aidoku 官方清單的同名圖源互相覆蓋。之前裝過的話，請先移除舊的再重新安裝，書庫與閱讀進度要重新加入，有登入的要重新登入。

> [!WARNING]
> **巴卡漫畫：站方會偵測並封鎖使用第三方工具的帳號，登入閱讀或下載可能導致封號，請自行評估。**

- [BOOK☆WALKER 台灣](https://www.bookwalker.com.tw/)（只列已購買的書，需網頁登入）
- [CCC追漫台](https://www.creative-comic.tw/)（登入可以顯示金幣、點數餘額）
- [Line Webtoons 繁體中文](https://www.webtoons.com/zh-hant/)
- [Komiic 漫畫 (new-zh)](https://komiic.com/)（使用帳號密碼登入，非cookie，並且能夠維持登入狀態）
- [TIBIU 漫畫在線](https://comic.tibiu.net/)
- [紳士漫畫 (new-zh)](https://www.wnacg.com/)（可自訂網址避免無法連接、網頁登入後可看韓漫）
- [喜漫漫畫](https://www.favcomic.com/)（可用帳號密碼登入並顯示金幣餘額，支援四個鏡像網域與圖片線路切換）
- [嬉皮漫畫](https://m.hipmh.com/)
- [輕書架](https://www.lightnovel.app/)（小說與漫畫，需帳密登入）
- [18漫畫](https://18mh.org/)（站方有 Cloudflare 驗證，載入不出內容時可在設定頁手動通過驗證，可搭配自架的 Cloudflare 轉發伺服器）
- [漫畫狗](https://dogemanga.com/)
- [HO5HO](https://www.ho5ho.com/)
- [漫小肆韓漫](https://www.jjmhw9.top/)（支援七個鏡像網域，也可自訂網址）
- [vomic漫](https://www.vomicmh.com/)（閱讀需帳密登入，可切換原圖畫質）
- [嗶哩輕小說](https://tw.linovelib.com/)
- [漫網](https://www.manwang.net/)
- [巴卡漫畫 (new-zh)](https://bakamh.com/)（可用帳號密碼登入閱讀需登入的章節，換網址會自動重新登入，支援五個鏡像網域，也可自訂網址，站方有 Cloudflare 驗證，載入不出內容時可在設定頁手動通過驗證，可搭配自架的 Cloudflare 轉發伺服器，**⚠ 站方會偵測並封鎖使用第三方工具的帳號，登入閱讀或下載可能導致封號，請自行評估**）
- [漫畫1234](https://m.wmh1234.com/)
- [漫畫客](http://yumanhua.com/)
