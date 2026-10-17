//! Explicit settings-based coin purchases. Reader requests only queue candidates.
use aidoku::{
	alloc::{String, Vec},
	imports::{defaults::{defaults_get, defaults_set, DefaultValue}, std::current_date},
	prelude::*,
	ButtonSetting, Chapter, GroupSetting, Manga, Result, Setting,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha1::{Digest, Sha1};
use crate::{api, auth, helper::episode_no, models::has_valid_right, unlock_models::*};

const CANDIDATES_KEY: &str = "unlock_candidates";
const MENU_KEY: &str = "unlock_menu";
const ARMED_KEY: &str = "unlock_reviewed_quote";
const ATTEMPTS_KEY: &str = "unlock_unresolved_attempts";
const STATUS_KEY: &str = "unlock_status";
const BALANCE_KEY: &str = "unlock_balance";
const COUNTER_KEY: &str = "unlock_nonce_counter";

#[derive(Clone, Deserialize, Serialize)]
struct Target {
	title_no: i32,
	episode_no: i32,
	manga_title: String,
	chapter_title: String,
}
#[derive(Deserialize, Serialize)]
struct Menu {
	account_id: String,
	cookie_hash: String,
	target: Target,
	offers: Vec<Offer>,
	created_at: i64,
	nonce: String,
}
#[derive(Deserialize, Serialize)]
struct Attempt { quote: Quote }

fn load<T: DeserializeOwned>(key: &str) -> Option<T> {
	serde_json::from_str(&defaults_get::<String>(key)?).ok()
}
fn save<T: Serialize>(key: &str, value: &T) -> Result<()> {
	let raw = serde_json::to_string(value).map_err(|_| error!("無法儲存解鎖資料"))?;
	defaults_set(key, DefaultValue::String(raw.clone()));
	if defaults_get::<String>(key).as_deref() != Some(raw.as_str()) {
		return Err(error!("無法確認解鎖資料已儲存，未送出扣款"));
	}
	Ok(())
}
fn erase(key: &str) {
	defaults_set(key, DefaultValue::String(String::new()));
	defaults_set(key, DefaultValue::Null);
}
fn status(message: impl Into<String>) {
	defaults_set(STATUS_KEY, DefaultValue::String(message.into()));
}
fn fingerprint(value: &str) -> String {
	Sha1::digest(value.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
fn current_cookie_hash() -> Option<String> { auth::api_cookie().map(|cookie| fingerprint(&cookie)) }

pub fn clear_session_state() {
	for key in [CANDIDATES_KEY, MENU_KEY, ARMED_KEY, STATUS_KEY, BALANCE_KEY] { erase(key); }
	// Uncertain financial attempts survive sign-out. Their account ID is checked
	// against a fresh verified member before another purchase can be prepared.
}

pub fn queue_candidate(manga: &Manga, chapter: &Chapter) -> Result<()> {
	let title = manga.key.parse::<i32>().ok().filter(|n| *n > 0)
		.ok_or_else(|| error!("無效的作品編號"))?;
	let episode = episode_no(&chapter.key).ok_or_else(|| error!("無效的章節編號"))?;
	let target = Target {
		title_no: title, episode_no: episode, manga_title: manga.title.clone(),
		chapter_title: chapter.title.clone().unwrap_or_else(|| format!("章節 {episode}")),
	};
	let mut candidates: Vec<Target> = load(CANDIDATES_KEY).unwrap_or_default();
	candidates.retain(|item| item.title_no != title || item.episode_no != episode);
	candidates.insert(0, target);
	candidates.truncate(5);
	save(CANDIDATES_KEY, &candidates)?;
	// Read-ahead must never arm a purchase or replace a user-reviewed quote.
	println!("[webtoons] queued unlock candidate: title={title}, episode={episode}; no debit sent");
	Ok(())
}

fn profile() -> Result<(String, String)> {
	auth::ensure_session();
	let cookie = auth::api_cookie().ok_or_else(|| error!("請先登入 WEBTOON 帳號"))?;
	let Some(member) = api::member_with_cookie(&cookie)? else {
		auth::mark_relogin(&cookie);
		return Err(error!("登入已失效，請重新登入"));
	};
	if auth::api_cookie().as_deref() != Some(cookie.as_str()) {
		return Err(error!("登入狀態已變更，請重新選擇解鎖方案"));
	}
	Ok((member.id, cookie))
}
fn next_nonce() -> String {
	let next = defaults_get::<String>(COUNTER_KEY).and_then(|n| n.parse::<u64>().ok())
		.unwrap_or(0).wrapping_add(1);
	defaults_set(COUNTER_KEY, DefaultValue::String(format!("{next}")));
	format!("{}-{next}", current_date())
}

fn attempts() -> Result<Vec<Attempt>> {
	let Some(raw) = defaults_get::<String>(ATTEMPTS_KEY) else { return Ok(Vec::new()); };
	if raw.is_empty() { return Ok(Vec::new()); }
	serde_json::from_str(&raw).map_err(|_| error!("交易紀錄無法讀取，禁止再次扣款，請至官方 App 查核"))
}
fn unresolved(account: &str, product: &str) -> Result<bool> {
	Ok(attempts()?.iter()
		.any(|attempt| attempt.quote.account_id == account && attempt.quote.offer.product_id == product))
}
fn clear_resolved(account: &str, product: &str) -> Result<()> {
	let mut attempts = attempts()?;
	attempts.retain(|attempt| attempt.quote.account_id != account || attempt.quote.offer.product_id != product);
	save(ATTEMPTS_KEY, &attempts)
}

/// Persist before the only non-idempotent request; neither timeouts nor duplicate
/// notifications may send the same account/product debit a second time.
fn submit_once(quote: &Quote, submit: impl FnOnce() -> Result<BuyResult>) -> Result<BuyResult> {
	if unresolved(&quote.account_id, &quote.offer.product_id)? {
		return Err(error!("上次解鎖結果尚未確認，不能重複送出扣款"));
	}
	let mut attempts = attempts()?;
	attempts.push(Attempt { quote: quote.clone() });
	save(ATTEMPTS_KEY, &attempts)?;
	submit()
}

fn load_menu(title: i32, episode: i32) -> Result<()> {
	erase(ARMED_KEY);
	erase(MENU_KEY);
	let mut target = load::<Vec<Target>>(CANDIDATES_KEY).unwrap_or_default().into_iter()
		.find(|target| target.title_no == title && target.episode_no == episode)
		.ok_or_else(|| error!("請先開啟想解鎖的章節，再回來源設定選擇"))?;
	let (account, cookie) = profile()?;
	let rights = api::product_rights(&format!("{title}"))?;
	let product_id = format!("linewebtoon-WEBTOON-{title}-{episode}");
	if has_valid_right(&rights, &format!("{title}"), episode, current_date()*1000) {
		clear_resolved(&account, &product_id)?;
		status("此章節已解鎖，請回作品詳情重新整理後閱讀。");
		return Ok(());
	}
	if unresolved(&account, &product_id)? {
		return Err(error!("這個章節有未確認的解鎖紀錄，請先按「同步解鎖權限」，或到官方 App 查核交易紀錄"));
	}
	let product = api::product_menu(title, episode)?;
	let balance = api::coin_balance()?;
	let offers = product.coin_offers(title, episode, current_date()*1000)?;
	if !product.title_info.title_name.is_empty() { target.manga_title = product.title_info.title_name; }
	if let Some(name) = product.product.episode_title { target.chapter_title = name; }
	defaults_set(BALANCE_KEY, DefaultValue::String(format!("可用代幣：{}", balance.amount)));
	save(MENU_KEY, &Menu {
		account_id: account, cookie_hash: fingerprint(&cookie), target, offers,
		created_at: current_date(), nonce: next_nonce(),
	})?;
	status("已載入方案。選擇下方價格只會準備報價，尚不會扣除代幣。");
	Ok(())
}

fn arm_offer(nonce: &str, index: usize) -> Result<()> {
	let menu: Menu = load(MENU_KEY).ok_or_else(|| error!("請先載入章節的解鎖方案"))?;
	if menu.nonce != nonce || current_cookie_hash().as_deref() != Some(menu.cookie_hash.as_str()) {
		return Err(error!("登入或方案已變更，請重新載入"));
	}
	if !(0..=180).contains(&(current_date() - menu.created_at)) {
		return Err(error!("報價已過期，請重新載入"));
	}
	let offer = menu.offers.get(index).ok_or_else(|| error!("無效的解鎖方案"))?.clone();
	save(ARMED_KEY, &Quote {
		account_id: menu.account_id, title_no: menu.target.title_no, episode_no: menu.target.episode_no,
		manga_title: menu.target.manga_title, chapter_title: menu.target.chapter_title,
		offer, prepared_at: menu.created_at, nonce: format!("{nonce}-{index}"),
	})?;
	status("尚未扣款。核對「待確認的解鎖」後，按「確認使用代幣解鎖」並確認，才會送出一次扣款。");
	Ok(())
}

fn confirm() -> Result<()> {
	let Some(quote) = load::<Quote>(ARMED_KEY) else {
		// A duplicate button callback must not overwrite a completed purchase result.
		return Ok(());
	};
	// Consume the reviewed quote before any I/O. Retrying requires a fresh review,
	// and an unresolved transaction below will block that review for this product.
	erase(ARMED_KEY);
	let (account, cookie) = profile()?;
	if account != quote.account_id {
		return Err(error!("帳號已變更，未送出扣款。請重新選擇解鎖方案"));
	}
	let title = format!("{}", quote.title_no);
	let rights = api::product_rights(&title)?;
	if has_valid_right(&rights, &title, quote.episode_no, current_date()*1000) {
		clear_resolved(&account, &quote.offer.product_id)?;
		status("此章節已解鎖，沒有再次扣款。請回作品重新整理後閱讀。");
		return Ok(());
	}
	let product = api::product_menu(quote.title_no, quote.episode_no)?;
	let balance = api::coin_balance()?;
	quote.validate(&account, current_date(), &product, &balance)?;
	if auth::api_cookie().as_deref() != Some(cookie.as_str()) {
		return Err(error!("登入狀態已變更，未送出扣款"));
	}

	let result = submit_once(&quote, || api::buy_product(&quote.offer, &cookie));
	let complete = result.as_ref().is_ok_and(BuyResult::complete);
	println!("[webtoons] confirmed coin request: title={}, episode={}, complete={complete}; no automatic retry",
		quote.title_no, quote.episode_no);
	// After this point errors must not be described as "no debit": a timeout can
	// hide a successful payment. Only a fresh right resolves the persisted guard.
	let owned = api::product_rights(&title).ok()
		.is_some_and(|rights| has_valid_right(&rights, &title, quote.episode_no, current_date()*1000));
	if owned {
		let _ = clear_resolved(&account, &quote.offer.product_id);
		erase(MENU_KEY);
		status("章節已解鎖。請回作品詳情下拉重新整理，再開啟閱讀。");
	} else if complete {
		status("扣款已完成，解鎖權限尚未更新。請按「同步解鎖權限」查詢；不會再次送出扣款。");
	} else {
		status("解鎖結果未確認，請按「同步解鎖權限」或到官方 App 查核交易紀錄；此章節不會自動或重複扣款。");
	}
	Ok(())
}

fn sync_rights() -> Result<()> {
	let (account, _) = profile()?;
	let mut targets: Vec<(i32, i32)> = load::<Vec<Target>>(CANDIDATES_KEY).unwrap_or_default().iter()
		.map(|target| (target.title_no, target.episode_no)).collect();
	for attempt in attempts()? {
		if attempt.quote.account_id == account {
			targets.push((attempt.quote.title_no, attempt.quote.episode_no));
		}
	}
	targets.sort_unstable();
	targets.dedup();
	let mut resolved = 0;
	let mut current_title = None;
	let mut rights = Vec::new();
	for (title, episode) in targets {
		if current_title != Some(title) {
			rights = api::product_rights(&format!("{title}"))?;
			current_title = Some(title);
		}
		if has_valid_right(&rights, &format!("{title}"), episode, current_date()*1000) {
			clear_resolved(&account, &format!("linewebtoon-WEBTOON-{title}-{episode}"))?;
			resolved += 1;
		}
	}
	if resolved > 0 {
		status(format!("已確認 {resolved} 個章節可閱讀。請回作品詳情下拉重新整理。"));
	} else {
		status("已查詢目前權限，尚未取得待解鎖章節的新權限。若剛在官方 App 看完廣告或購買，請稍後再同步。");
	}
	Ok(())
}

pub fn handle_notification(notification: &str) -> bool {
	let result = match notification {
		"confirmUnlock" => confirm(),
		"syncUnlock" => sync_rights(),
		"cancelUnlock" => { erase(ARMED_KEY); erase(MENU_KEY); status("已取消待確認方案，沒有送出新的扣款。"); Ok(()) },
		_ => {
			if let Some(rest) = notification.strip_prefix("loadUnlock:") {
				let parts: Vec<&str> = rest.split(':').collect();
				if parts.len() != 2 { return true; }
				match (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
					(Ok(title), Ok(episode)) => load_menu(title, episode),
					_ => Err(error!("無效的章節選擇")),
				}
			} else if let Some(rest) = notification.strip_prefix("selectUnlock:") {
				let Some((nonce, index)) = rest.split_once(':') else { return true; };
				match index.parse::<usize>() {
					Ok(index) => arm_offer(nonce, index),
					Err(_) => Err(error!("無效的方案選擇")),
				}
			} else { return false; }
		}
	};
	if let Err(err) = result {
		// All submit errors are handled inside confirm, after storing the guard.
		// Errors reaching here happened before any new debit could be submitted.
		let message = match &err {
			aidoku::AidokuError::Message(message) => message.clone(),
			_ => String::from("連線或資料讀取失敗，請重新載入方案或查核權限"),
		};
		status(format!("未送出新的扣款：{message}"));
		println!("[webtoons] unlock preparation/check failed: {err:?}");
	}
	true
}

fn button(key: String, title: String, notification: String) -> Setting {
	ButtonSetting {
		key: key.into(), title: title.into(), notification: Some(notification.into()),
		refreshes: Some(aidoku::alloc::vec!["settings".into()]), ..Default::default()
	}.into()
}
fn group(key: &'static str, title: &'static str, footer: String, items: Vec<Setting>) -> Setting {
	GroupSetting { key: key.into(), title: title.into(), footer: Some(footer.into()), items, ..Default::default() }.into()
}

/// Settings rendering reads local state only. Server checks happen only after
/// explicit load/confirm/sync actions, never on opening the settings screen.
pub fn settings() -> Vec<Setting> {
	let mut settings = aidoku::alloc::vec![group("unlockStatus", "解鎖狀態",
		defaults_get::<String>(STATUS_KEY).filter(|value| !value.is_empty())
			.unwrap_or_else(|| String::from("先開啟想解鎖的章節，再在下方載入方案。選方案後，最後確認才會扣除代幣。")), Vec::new())];
	if let Some(quote) = load::<Quote>(ARMED_KEY) {
		settings.push(group("unlockReview", "待確認的解鎖", format!("{}\n{}\n{}\n{}\n尚未扣款。核對後再按「確認使用代幣解鎖」。",
			quote.manga_title, quote.chapter_title, quote.offer.label(),
			defaults_get::<String>(BALANCE_KEY).unwrap_or_default()), Vec::new()));
	}
	if let Some(menu) = load::<Menu>(MENU_KEY) {
		if current_cookie_hash().as_deref() == Some(menu.cookie_hash.as_str()) {
			let items = menu.offers.iter().enumerate().map(|(index, offer)| {
				button(format!("unlockOffer{index}"), offer.label(), format!("selectUnlock:{}:{index}", menu.nonce))
			}).collect();
			let hint = if menu.offers.is_empty() {
				"目前沒有可用的代幣方案；廣告或限時解鎖請使用官方 App，完成後回來同步。"
			} else { "點選方案只會準備報價，不會扣款。" };
			settings.push(group("unlockOffers", "選擇解鎖方案",
				format!("{}\n{}\n{hint}", menu.target.manga_title, menu.target.chapter_title), items));
		}
	}
	let candidates: Vec<Target> = load(CANDIDATES_KEY).unwrap_or_default();
	if !candidates.is_empty() {
		let items = candidates.into_iter().map(|target| button(
			format!("unlockTarget{}_{}", target.title_no, target.episode_no),
			format!("{}／{}", target.manga_title, target.chapter_title),
			format!("loadUnlock:{}:{}", target.title_no, target.episode_no),
		)).collect();
		settings.push(group("unlockCandidates", "待解鎖章節", String::from("選擇章節載入站方的方案與價格；此步驟不扣款。"), items));
	}
	settings
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;
	use core::cell::Cell;

	fn quote() -> Quote {
		Quote { account_id: "test-account".into(), title_no: 10568, episode_no: 2,
			manga_title: "咒術迴戰".into(), chapter_title: "第1集".into(),
			offer: Offer { product_id: "linewebtoon-WEBTOON-10568-2".into(), product_sale_unit_id: "sale-2".into(),
				policy_price_currency: "TW_WCOIN".into(), policy_price: 27, sale_unit_type: "COMPLETE".into(),
				end_sale_date: 32503647599000, rental_days: None }, prepared_at: 100, nonce: "q1".into() }
	}

	#[aidoku_test]
	fn a_timeout_or_duplicate_confirmation_never_repeats_the_debit() {
		erase(ATTEMPTS_KEY);
		let calls = Cell::new(0);
		assert!(submit_once(&quote(), || { calls.set(calls.get()+1); Err(error!("timeout")) }).is_err());
		assert!(submit_once(&quote(), || { calls.set(calls.get()+1); Err(error!("must not send again")) }).is_err());
		assert_eq!(calls.get(), 1);
		assert_eq!(load::<Vec<Attempt>>(ATTEMPTS_KEY).unwrap().len(), 1);
	}

	#[aidoku_test]
	fn read_ahead_can_add_candidates_but_cannot_replace_the_reviewed_quote() {
		clear_session_state();
		save(ARMED_KEY, &quote()).unwrap();
		queue_candidate(&Manga { key: "3123".into(), title: "最強的夢".into(), ..Default::default() },
			&Chapter { key: "271".into(), title: Some("第11話".into()), ..Default::default() }).unwrap();
		assert_eq!(load::<Quote>(ARMED_KEY).unwrap().nonce, "q1");
		assert!(load::<Vec<Attempt>>(ATTEMPTS_KEY).unwrap_or_default().is_empty());
	}

	#[aidoku_test]
	fn logout_discards_quotes_without_losing_the_uncertain_transaction_guard() {
		erase(ATTEMPTS_KEY);
		let _ = submit_once(&quote(), || Err(error!("unknown result")));
		save(ARMED_KEY, &quote()).unwrap();
		clear_session_state();
		assert!(load::<Quote>(ARMED_KEY).is_none());
		assert_eq!(load::<Vec<Attempt>>(ATTEMPTS_KEY).unwrap().len(), 1);
	}

	#[aidoku_test]
	fn corrupted_transaction_history_blocks_submission_instead_of_starting_over() {
		defaults_set(ATTEMPTS_KEY, DefaultValue::String("not-json".into()));
		let calls = Cell::new(0);
		assert!(submit_once(&quote(), || { calls.set(calls.get()+1); Err(error!("must not send")) }).is_err());
		assert_eq!(calls.get(), 0);
	}

	#[aidoku_test]
	fn canceling_a_quote_does_not_clear_an_uncertain_transaction() {
		erase(ATTEMPTS_KEY);
		let _ = submit_once(&quote(), || Err(error!("unknown")));
		save(ARMED_KEY, &quote()).unwrap();
		assert!(handle_notification("cancelUnlock"));
		assert!(load::<Quote>(ARMED_KEY).is_none());
		assert_eq!(load::<Vec<Attempt>>(ATTEMPTS_KEY).unwrap().len(), 1);
	}
}
