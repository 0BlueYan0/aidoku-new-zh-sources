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
		std::current_date,
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
/// Unix seconds before which `ensure_session` does nothing at all. This gate is what
/// keeps automatic renewal affordable - see that function.
const NEXT_CHECK_KEY: &str = "sessionNextCheck";
/// Set when the stored session is dead and the refresh token cannot revive it.
const NEEDS_RELOGIN_KEY: &str = "needsRelogin";

/// How long a session that just answered is trusted before being probed again.
const CHECK_INTERVAL: i64 = 1800;
/// How soon to probe again when the probe itself could not reach the server.
const PROBE_RETRY: i64 = 60;
/// How long to wait after the token endpoint refused to renew.
const FAIL_BACKOFF: i64 = 600;

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

/// Unix seconds stored as a string, so the value cannot overflow an i32.
fn timestamp(key: &str) -> i64 {
	defaults_get::<String>(key)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0)
}

fn set_timestamp(key: &str, value: i64) {
	defaults_set(key, DefaultValue::String(format!("{value}")));
}

/// Set when the stored session is dead and nothing on the device can revive it.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
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

/// What a probe of the stored session found.
enum Probe {
	/// The session is still accepted.
	Live,
	/// CCC refused it. A dead token is refused on every endpoint, not just this one.
	Dead,
	/// No usable answer, which says nothing about the session.
	Unknown,
}

/// Ask CCC whether the stored token is still accepted.
fn probe_session() -> Probe {
	let Ok(request) = crate::helper::api_request("/member") else {
		return Probe::Unknown;
	};
	let Ok(response) = request.send() else {
		return Probe::Unknown;
	};
	match response.status_code() {
		200 => Probe::Live,
		401 => Probe::Dead,
		_ => Probe::Unknown,
	}
}

/// Keep the session alive while the reader browses, so an expired token does not simply
/// stop the source until someone opens the settings screen.
///
/// Called from the top of every `Source` entry point, which is only affordable because of
/// the gate: while the session was verified recently this reads two defaults and returns.
/// **Every branch below moves the gate forward.** An earlier attempt renewed from inside
/// `api_get` with no gate at all, so a dead token - which CCC refuses on *every* endpoint -
/// meant one token request per content request, and the home screen alone issues eight.
/// The cost of an ungated renewal is multiplied by the number of entry points, which is
/// what turns it from an optimisation into a freeze.
///
/// Same shape as `zh.komiic`'s `ensure_session`, deliberately.
pub fn ensure_session() {
	if !is_logged_in() || needs_relogin() {
		return;
	}

	let now = current_date();
	if now < timestamp(NEXT_CHECK_KEY) {
		return;
	}
	// Moved before the probe so that entry points running at the same time do not each
	// send one; whichever gets here first speaks for all of them.
	set_timestamp(NEXT_CHECK_KEY, now + PROBE_RETRY);

	match probe_session() {
		Probe::Live => set_timestamp(NEXT_CHECK_KEY, now + CHECK_INTERVAL),
		Probe::Dead => match refresh() {
			TokenOutcome::Renewed => {
				println!("[ccc] session renewed");
				set_timestamp(NEXT_CHECK_KEY, now + CHECK_INTERVAL);
			}
			TokenOutcome::Rejected => {
				// Nothing on the device can revive this. Drop it so browsing carries on
				// as a guest, and let the settings footer explain.
				println!("[ccc] session cannot be renewed, falling back to guest");
				clear();
				defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
			}
			TokenOutcome::Unreachable => set_timestamp(NEXT_CHECK_KEY, now + FAIL_BACKOFF),
		},
		// Says nothing about the session, so try again soon rather than acting on it.
		Probe::Unknown => set_timestamp(NEXT_CHECK_KEY, now + PROBE_RETRY),
	}
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
enum TokenOutcome {
	/// New tokens were issued and stored.
	Renewed,
	/// The server answered and refused. CCC replies `410 Cannot decrypt the refresh
	/// token` for one that is invalid or already spent.
	Rejected,
	/// No usable answer; worth retrying later.
	Unreachable,
}

fn request_token(pairs: &[(&str, &str)]) -> TokenOutcome {
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
			return TokenOutcome::Unreachable;
		}
	};

	let response = match request.send() {
		Ok(response) => response,
		Err(_) => {
			println!("[ccc] ERROR the token endpoint could not be reached");
			return TokenOutcome::Unreachable;
		}
	};
	let status = response.status_code();

	// A 4xx is CCC saying no - a spent refresh token comes back `410 Cannot decrypt the
	// refresh token`. Anything else is worth retrying, so the two are told apart by the
	// status rather than by the absence of a token.
	let refused = |status: i32| {
		if (400..500).contains(&status) {
			println!("[ccc] the token endpoint refused the grant (HTTP {status})");
			TokenOutcome::Rejected
		} else {
			println!("[ccc] ERROR the token endpoint returned no access token (HTTP {status})");
			TokenOutcome::Unreachable
		}
	};

	// A successful grant answers with a bare OAuth2 payload rather than the usual envelope.
	let Ok(parsed) = response.get_json_owned::<TokenResponse>() else {
		return refused(status);
	};
	let Some(access) = parsed.access_token.clone().filter(|value| !value.is_empty()) else {
		return refused(status);
	};

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access));
	if let Some(refresh) = parsed.refresh_token.filter(|value| !value.is_empty()) {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	TokenOutcome::Renewed
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
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	defaults_set(NEXT_CHECK_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
	true
}

/// Swap the refresh token for a fresh pair. Returns true when it worked.
fn refresh() -> TokenOutcome {
	let Some(refresh_token) = stored(REFRESH_TOKEN_KEY) else {
		// Nothing to renew with, and nothing will change that on its own.
		return TokenOutcome::Rejected;
	};
	request_token(&[
		("grant_type", "refresh_token"),
		("client_id", CLIENT_ID),
		("client_secret", CLIENT_SECRET),
		("refresh_token", refresh_token.as_str()),
	])
}

/// The localStorage entries CCC keeps its session in.
const SESSION_STORAGE_KEYS: [&str; 3] = ["accessToken", "refreshToken", "userId"];

/// Open the site in a background web view, on the same origin as the login page.
///
/// Two details matter and both have bitten us:
///
/// * `BASE_URL` redirects to `/zh/`, so the language-prefixed url is loaded directly and
///   the web view performs one navigation instead of two.
/// * `load_blocking` already returns once the page has loaded. Calling `wait_for_load`
///   after it was waiting for a *second* load that, with no redirect left to follow,
///   never comes - and that call has no timeout and returns no error, so it simply never
///   came back and the app froze with it.
fn open_site() -> Option<WebView> {
	let webview = WebView::new();
	let url = format!("{BASE_URL}/zh/");
	let request = Request::get(&url).ok()?.header("User-Agent", USER_AGENT);
	webview.load_blocking(request).ok()?;
	Some(webview)
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
	let Some(webview) = open_site() else {
		defaults_set(
			SYNC_RESULT_KEY,
			DefaultValue::String(String::from("連不上 CCC，請確認網路後再試一次。")),
		);
		return false;
	};

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
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	defaults_set(NEXT_CHECK_KEY, DefaultValue::Null);
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

/// Sign out of CCC as well as out of Aidoku.
///
/// `clearCookiesOnLogOut` only clears cookies, and CCC keeps its session in
/// `localStorage`, so on its own it leaves the site signed in: the login page comes back
/// already authenticated and the sync button restores the very same account.
pub fn clear_web_session() {
	match open_site() {
		Some(webview) => {
			// `eval` hands back whatever the script evaluates to, and that has to be a
			// string. `localStorage.removeItem(...)` evaluates to `undefined`, which comes
			// back as an error even though the removal worked - reporting a failure that
			// never happened. So the script ends by reading the keys back: that gives
			// `eval` its string and makes the log honest, because empty means the session
			// really is gone.
			let names = SESSION_STORAGE_KEYS
				.iter()
				.map(|key| format!("'{key}'"))
				.collect::<Vec<String>>()
				.join(",");
			// Keys are literals defined in this file, so the quoting is safe.
			let script = format!(
				"var k=[{names}];				 k.forEach(function(n){{try{{localStorage.removeItem(n)}}catch(e){{}}}});				 k.filter(function(n){{return localStorage.getItem(n)}}).join(',')"
			);
			match webview.eval(&script) {
				Ok(left) if left.trim().is_empty() => println!("[ccc] site session cleared"),
				Ok(left) => println!("[ccc] ERROR the site's storage still holds {left}"),
				Err(_) => println!("[ccc] ERROR could not run the storage clearing script"),
			}
		}
		None => println!("[ccc] ERROR could not open the web view to clear the site session"),
	}
	clear();
}

pub fn clear() {
	defaults_set(NEXT_CHECK_KEY, DefaultValue::Null);
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
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

fn fetch_member() -> Option<Member> {
	let request = crate::helper::api_request("/member").ok()?;
	read_envelope::<Member>(request).ok()
}

/// The account summary shown in settings. Always says something.
///
/// Returning nothing here is not an option: an empty answer leaves `get_dynamic_settings`
/// with an empty list, and a source that hands the app an empty list of dynamic settings
/// gets a settings screen where the buttons stop responding - which looks exactly like
/// the source has frozen. Signed out with nothing wrong is therefore a sentence about
/// being signed out, not silence.
///
/// The balances, once signed in, are also the only visible proof that signing in worked,
/// so a failure names the next step rather than going blank.
pub fn account_footer() -> Option<String> {
	if needs_relogin() {
		return Some(String::from(
			"登入已失效且無法自動恢復，請按「清除登入狀態」後重新登入。",
		));
	}
	if !is_logged_in() {
		// Signed out: nothing to fetch, and defaults are all this needs to read.
		if let Some(reason) = stored(SYNC_RESULT_KEY) {
			return Some(reason);
		}
		if stored(SEEN_KEYS_KEY).is_some() {
			let calls = defaults_get::<i32>(CALL_COUNT_KEY).unwrap_or(0);
			let seen = stored(SEEN_KEYS_KEY).unwrap_or_default();
			println!("[ccc] web login called back {calls} time(s) handing over: {seen}");
			return Some(String::from(
				"登入尚未完成。請按「登入」登入後，再按「同步登入狀態」。",
			));
		}
		return Some(String::from(
			"尚未登入。登入後才能讀取已購買的章節：按「登入」完成登入，再按「同步登入狀態」。",
		));
	}

	let member = match fetch_member() {
		Some(member) => Some(member),
		// A stale token is the usual cause. The gate is cleared first so the check is not
		// skipped just because one ran recently - the reader is here now, looking at it.
		None => {
			defaults_set(NEXT_CHECK_KEY, DefaultValue::Null);
			ensure_session();
			if is_logged_in() {
				fetch_member()
			} else {
				None
			}
		}
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
