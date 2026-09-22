//! Identity: the guest uuid, OAuth tokens, and the account summary.
//!
//! Every API call must identify itself, either with a `uuid` header (guests) or a
//! bearer token (signed in). Without one the API answers `403 uuid錯誤`.
//!
//! Only the tokens are persisted, never the password: CCC issues a refresh token, so
//! there is no reason to keep credentials on the device.

use aidoku::{
	alloc::{String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		js::WebView,
		net::Request,
	},
	prelude::*,
	HashMap,
};
use serde::Deserialize;

use crate::crypto;
use crate::helper::{read_envelope, API_URL, BASE_URL, DEVICE, USER_AGENT};

const UUID_KEY: &str = "guestUuid";
const ACCESS_TOKEN_KEY: &str = "accessToken";
const REFRESH_TOKEN_KEY: &str = "refreshToken";
/// Set when a session is picked up, so the `login` notification that follows can be told
/// apart from a sign-out; Aidoku posts the same notification name for both.
const JUST_LOGGED_IN_KEY: &str = "justLoggedIn";
/// Names of the entries the web login view actually handed over. Recorded so the
/// settings footer can say what arrived when no token could be found - the app's
/// delivery of `localStorageKeys` is undocumented and no shipped source relies on it.
const SEEN_KEYS_KEY: &str = "webLoginKeys";
/// How many times the web login view has called back. Distinguishes "never called" from
/// "called but handed over nothing", which look identical otherwise.
const CALL_COUNT_KEY: &str = "webLoginCalls";
/// What the last storage sync saw, so a failure can be read off the settings screen.
const SYNC_RESULT_KEY: &str = "syncResult";

/// How many times each notification has reached this source. TEMPORARY, for diagnosis:
/// when both buttons look like they do nothing, the first thing worth knowing is whether
/// the press arrives here at all, and the log server on this setup receives nothing.
const NOTIFY_COUNT_KEYS: [(&str, &str); 3] = [
	("login", "nLogin"),
	("syncLogin", "nSync"),
	("clearLogin", "nClear"),
];

/// The OAuth client the site embeds in its own web bundle. Both values are served to
/// every visitor of creative-comic.tw, so neither is a secret; they are reproduced here
/// because the token endpoint requires them.
const CLIENT_ID: &str = "2";
const CLIENT_SECRET: &str = "9eAhsCX3VWtyqTmkUo5EEaoH4MNPxrn6ZRwse7tE";

#[derive(Deserialize)]
struct TokenResponse {
	#[serde(default)]
	access_token: Option<String>,
	#[serde(default)]
	refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct Member {
	#[serde(default)]
	nickname: Option<String>,
	#[serde(default)]
	name: Option<String>,
	#[serde(default)]
	coin: Option<i64>,
	#[serde(default)]
	point: Option<i64>,
}

fn stored(key: &str) -> Option<String> {
	defaults_get::<String>(key).filter(|value| !value.is_empty())
}

pub fn access_token() -> Option<String> {
	stored(ACCESS_TOKEN_KEY)
}

pub fn is_logged_in() -> bool {
	access_token().is_some()
}

/// The credential page images are encrypted against: the access token when signed in,
/// otherwise the site's public guest secret.
pub fn image_secret() -> String {
	access_token().unwrap_or_else(|| String::from(crypto::GUEST_SECRET))
}

/// Fetch a guest uuid once and keep it, mirroring what the website stores in
/// localStorage. A fresh uuid per launch would look like a flood of new visitors.
///
/// This must only be called from the top of a `Source` entry point, never while another
/// request is being assembled: it blocks on a request of its own, and the app runs a
/// limited number of requests at once, so nesting one inside another can wedge them all.
pub fn ensure_guest_uuid() {
	if is_logged_in() || stored(UUID_KEY).is_some() {
		return;
	}
	let _ = fetch_guest_uuid();
}

fn fetch_guest_uuid() -> Option<String> {
	let url = format!("{API_URL}/guest");
	let request = Request::get(&url)
		.ok()?
		.header("User-Agent", USER_AGENT)
		.header("device", DEVICE)
		.header("Accept-Language", "zh");
	let uuid: String = read_envelope(request).ok()?;
	if uuid.is_empty() {
		return None;
	}
	defaults_set(UUID_KEY, DefaultValue::String(uuid.clone()));
	Some(uuid)
}

/// Attach whichever credential this install has.
///
/// Deliberately does no networking: this runs while a request is being built, and
/// fetching the uuid here would block that request on another one. `ensure_guest_uuid`
/// handles that at the entry points instead.
pub fn authorize(request: Request) -> Request {
	let mut request = request;
	if let Some(token) = access_token() {
		let header = format!("Bearer {token}");
		request.set_header("Authorization", header.as_str());
	} else if let Some(uuid) = stored(UUID_KEY) {
		request.set_header("uuid", uuid.as_str());
	}
	request
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
	let mut body = String::new();
	for (index, (key, value)) in pairs.iter().enumerate() {
		if index > 0 {
			body.push('&');
		}
		body.push_str(&crate::helper::encode_component(key));
		body.push('=');
		body.push_str(&crate::helper::encode_component(value));
	}
	body
}

/// POST the OAuth token endpoint. Returns true when new tokens were stored.
fn request_token(pairs: &[(&str, &str)]) -> bool {
	let url = format!("{API_URL}/token");
	let body = form_encode(pairs);
	let request = match Request::post(&url) {
		Ok(request) => request
			.header("User-Agent", USER_AGENT)
			.header("device", DEVICE)
			.header("Accept-Language", "zh")
			.header("Content-Type", "application/x-www-form-urlencoded")
			.body(body),
		Err(_) => {
			println!("[ccc] ERROR building the token request");
			return false;
		}
	};

	// The endpoint answers with a bare OAuth2 payload rather than the usual envelope.
	let response: TokenResponse = match request.json_owned() {
		Ok(response) => response,
		Err(_) => {
			println!("[ccc] ERROR the token endpoint did not return a usable payload");
			return false;
		}
	};

	let Some(access) = response.access_token.filter(|value| !value.is_empty()) else {
		println!("[ccc] ERROR the token endpoint returned no access token");
		return false;
	};

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access));
	if let Some(refresh) = response.refresh_token.filter(|value| !value.is_empty()) {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	true
}

/// Take the tokens out of whatever the web login view handed over.
///
/// The site keeps its session in `localStorage` (`accessToken` / `refreshToken`) rather
/// than in a cookie, which is why `settings.json` asks for those keys. The app merges
/// what it collected into this one map, so both are looked up here by name.
pub fn capture_web_login(values: &HashMap<String, String>) -> bool {
	// Record the call count and the key names (never the values) so a failed sign-in can
	// be diagnosed from the settings screen instead of needing a log server.
	let calls = defaults_get::<i32>(CALL_COUNT_KEY).unwrap_or(0) + 1;
	defaults_set(CALL_COUNT_KEY, DefaultValue::Int(calls));
	let mut seen: Vec<String> = values.keys().cloned().collect();
	seen.sort();
	let summary = if seen.is_empty() {
		String::from("（沒有任何項目）")
	} else {
		seen.join(", ")
	};
	defaults_set(SEEN_KEYS_KEY, DefaultValue::String(summary));

	let access = values
		.get("accessToken")
		.filter(|value| !value.is_empty())
		.cloned();
	let Some(access) = access else {
		return false;
	};

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access));
	if let Some(refresh) = values
		.get("refreshToken")
		.filter(|value| !value.is_empty())
		.cloned()
	{
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
	true
}

/// Swap the refresh token for a fresh pair. Returns true when it worked.
pub fn refresh() -> bool {
	let Some(refresh_token) = stored(REFRESH_TOKEN_KEY) else {
		return false;
	};
	request_token(&[
		("grant_type", "refresh_token"),
		("client_id", CLIENT_ID),
		("client_secret", CLIENT_SECRET),
		("refresh_token", refresh_token.as_str()),
	])
}

fn read_storage(webview: &WebView, key: &str) -> Option<String> {
	// Quoting is safe here: every key is a literal defined in this file.
	let script = format!("localStorage.getItem('{key}') || ''");
	let value = webview.eval(&script).ok()?;
	let value = value.trim();
	if value.is_empty() || value == "null" || value == "undefined" {
		return None;
	}
	Some(String::from(value))
}

/// Pick the session up out of the login web view's storage.
///
/// Aidoku never calls `handle_web_login` for this site: that callback is driven by
/// cookie updates and CCC sets no cookies at all, keeping its session in `localStorage`
/// instead. The tokens do exist after signing in, so this drives a web view against the
/// same origin and reads them out directly.
pub fn sync_from_web_view() -> bool {
	let webview = WebView::new();

	let request = match Request::get(BASE_URL) {
		Ok(request) => request.header("User-Agent", USER_AGENT),
		Err(_) => {
			defaults_set(
				SYNC_RESULT_KEY,
				DefaultValue::String(String::from("無法建立請求")),
			);
			return false;
		}
	};
	if webview.load_blocking(request).is_err() {
		defaults_set(
			SYNC_RESULT_KEY,
			DefaultValue::String(String::from("無法載入 CCC 網頁")),
		);
		return false;
	}
	webview.wait_for_load();

	let Some(access) = read_storage(&webview, "accessToken") else {
		// Reaching here means the web view loaded but its storage held no session -
		// most likely the login view and this one do not share a data store.
		defaults_set(
			SYNC_RESULT_KEY,
			DefaultValue::String(String::from(
				"網頁已載入，但它的 localStorage 裡沒有 accessToken（登入頁與背景網頁可能不共用儲存空間）",
			)),
		);
		return false;
	};

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access));
	if let Some(refresh) = read_storage(&webview, "refreshToken") {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
	defaults_set(SYNC_RESULT_KEY, DefaultValue::Null);
	true
}

/// Keys written by versions 8 to 12 that no later version reads.
const ORPHAN_KEYS: [&str; 4] = ["tokenExpiresAt", "tokenFailedAt", "needsRelogin", "loggedIn"];
/// Marks that the cleanup below has already run on this device.
const STATE_PURGED_KEY: &str = "statePurged";

/// Drop the session once, on devices that ran versions 8 to 12.
///
/// Those versions renewed the session by themselves, and CCC rotates the refresh token on
/// every renewal, so a device that ran them can be left holding an access token the
/// server rejects alongside a refresh token it has already spent. Nothing here clears
/// such a pair on its own - a dead token is simply attached to every request, which CCC
/// answers 401 on every endpoint - so the reader would be stuck signing in to a source
/// that never accepts the result. Dropping it costs one sign-in and ends that.
///
/// Defaults writes only. This runs while the source is loading, where a request would
/// block everything behind it.
pub fn purge_experimental_state() {
	if defaults_get::<bool>(STATE_PURGED_KEY).unwrap_or(false) {
		return;
	}
	// Written first, so a cleanup that somehow fails part way cannot run on every load.
	defaults_set(STATE_PURGED_KEY, DefaultValue::Bool(true));
	for key in ORPHAN_KEYS {
		defaults_set(key, DefaultValue::Null);
	}
	clear();
	println!("[ccc] cleared session state left by an earlier version");
}

pub fn clear() {
	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::Null);
	defaults_set(REFRESH_TOKEN_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	defaults_set(SEEN_KEYS_KEY, DefaultValue::Null);
	defaults_set(CALL_COUNT_KEY, DefaultValue::Null);
	defaults_set(SYNC_RESULT_KEY, DefaultValue::Null);
}

/// Aidoku posts the same notification for signing in and signing out, so the flag set
/// when the session was picked up is what tells them apart.
pub fn handle_login_notification() {
	let just_logged_in = defaults_get::<bool>(JUST_LOGGED_IN_KEY).unwrap_or(false);
	if just_logged_in {
		defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	} else {
		clear();
	}
}

/// Record that a notification arrived. TEMPORARY, see `NOTIFY_COUNT_KEYS`.
pub fn note_notification(name: &str) {
	for (notification, key) in NOTIFY_COUNT_KEYS {
		if notification == name {
			let count = defaults_get::<i32>(key).unwrap_or(0) + 1;
			defaults_set(key, DefaultValue::Int(count));
		}
	}
}

/// What this device has, in text the reader can send back. TEMPORARY, see
/// `NOTIFY_COUNT_KEYS`. Defaults reads only - no request is made.
fn diagnostics() -> String {
	let count = |key: &str| defaults_get::<i32>(key).unwrap_or(0);
	let mark = |present: bool| if present { "有" } else { "無" };
	format!(
		"診斷（暫時顯示）\n按鈕次數：登入 {} ・同步 {} ・清除 {}\ntoken {} ・uuid {} ・登入頁回呼 {} 次（交回「{}」）\n最後同步：{}",
		count("nLogin"),
		count("nSync"),
		count("nClear"),
		mark(is_logged_in()),
		mark(stored(UUID_KEY).is_some()),
		defaults_get::<i32>(CALL_COUNT_KEY).unwrap_or(0),
		stored(SEEN_KEYS_KEY).unwrap_or_else(|| String::from("沒有")),
		stored(SYNC_RESULT_KEY).unwrap_or_else(|| String::from("沒有紀錄")),
	)
}

fn fetch_member() -> Option<Member> {
	let request = crate::helper::api_request("/member").ok()?;
	read_envelope::<Member>(request).ok()
}

/// The account summary shown in settings, or `None` to leave the group out entirely.
///
/// The group stays hidden while signed out and nothing is wrong, so readers who never
/// sign in are not shown an empty panel. It appears when there is something to say: the
/// balances once signed in, or which step failed when it did not take. Those balances
/// are the only visible proof that signing in actually worked, so a failure names itself
/// rather than going blank.
pub fn account_footer() -> Option<String> {
	if !is_logged_in() {
		// Signed out: nothing to fetch, so report what the device actually holds. Reads
		// defaults only, so this path makes no request at all - it renders even when
		// every CCC host is unreachable, and cannot itself be what freezes anything.
		return Some(diagnostics());
	}

	let member = match fetch_member() {
		Some(member) => Some(member),
		// A stale access token is the usual cause, so try the refresh token once.
		None if refresh() => fetch_member(),
		None => None,
	};

	let Some(member) = member else {
		return Some(String::from("登入已失效，請重新登入。"));
	};

	let name = member
		.nickname
		.filter(|value| !value.is_empty())
		.or(member.name)
		.unwrap_or_else(|| String::from("已登入"));

	let mut parts: Vec<String> = Vec::new();
	parts.push(name);
	parts.push(format!("金幣 {}", member.coin.unwrap_or(0)));
	parts.push(format!("點數 {}", member.point.unwrap_or(0)));
	Some(parts.join("・"))
}
