//! Identity: the guest uuid, OAuth tokens, and the account summary.
//!
//! Every API call must identify itself, either with a `uuid` header (guests) or a
//! bearer token (signed in). Without one the API answers `403 uuid錯誤`.
//!
//! A *dead* bearer token is worse than none: CCC answers
//! `401 {"code":401,"message":"Unauthenticated."}` on every endpoint, `/book` included,
//! rather than falling back to guest access. So an expired session does not degrade the
//! source, it stops it. Renewal therefore has to be automatic - see `recover_session`.
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
use crate::helper::{api_request, read_envelope, send_api, Outcome, API_URL, BASE_URL, DEVICE, USER_AGENT};

const UUID_KEY: &str = "guestUuid";
const ACCESS_TOKEN_KEY: &str = "accessToken";
const REFRESH_TOKEN_KEY: &str = "refreshToken";
/// When renewal last failed to reach the server, so it is not retried on every request.
const FAILED_AT_KEY: &str = "tokenFailedAt";
/// Set when the session is dead and cannot be renewed, so the settings footer can say
/// so instead of leaving the reader guessing.
const NEEDS_RELOGIN_KEY: &str = "needsRelogin";
/// Set whenever a session is picked up, so the `login` notification that follows can be
/// told apart from a sign-out; the app posts the same name for both.
const JUST_LOGGED_IN_KEY: &str = "justLoggedIn";
/// Names of the entries the web login view actually handed over. Recorded so a sign-in
/// that did not take can be diagnosed from the log - the app's delivery of
/// `localStorageKeys` is undocumented and no shipped source relies on it.
const SEEN_KEYS_KEY: &str = "webLoginKeys";
/// How many times the web login view has called back. Distinguishes "never called" from
/// "called but handed over nothing", which look identical otherwise.
const CALL_COUNT_KEY: &str = "webLoginCalls";
/// What the last storage sync saw, so a failure can be read off the settings screen.
const SYNC_RESULT_KEY: &str = "syncResult";

/// The OAuth client the site embeds in its own web bundle. Both values are served to
/// every visitor of creative-comic.tw, so neither is a secret; they are reproduced here
/// because the token endpoint requires them.
const CLIENT_ID: &str = "2";
const CLIENT_SECRET: &str = "9eAhsCX3VWtyqTmkUo5EEaoH4MNPxrn6ZRwse7tE";

/// How long to wait after renewal could not reach the server before trying again.
const FAIL_BACKOFF: i64 = 600;

/// The localStorage entries CCC keeps its session in.
const SESSION_STORAGE_KEYS: [&str; 3] = ["accessToken", "refreshToken", "userId"];

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

pub fn access_token() -> Option<String> {
	stored(ACCESS_TOKEN_KEY)
}

pub fn is_logged_in() -> bool {
	access_token().is_some()
}

/// Set when the stored session is dead and nothing on the device can renew it.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

/// The credential page images are encrypted against: the access token when signed in,
/// otherwise the site's public guest secret.
pub fn image_secret() -> String {
	access_token().unwrap_or_else(|| String::from(crypto::GUEST_SECRET))
}

// ---------------------------------------------------------------------------
// Token storage
// ---------------------------------------------------------------------------

/// Store an access token.
///
/// No expiry is derived or kept. Renewing ahead of time would mean a blocking call to
/// the token endpoint at the top of every entry point, which is the one thing the app's
/// request limit cannot take; a dead token is caught by the 401 handling in `api_get`
/// instead, which runs only after a request has already come back.
fn set_access_token(access: &str) {
	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(String::from(access)));
}

// ---------------------------------------------------------------------------
// Credentials on the wire
// ---------------------------------------------------------------------------

/// Fetch a guest uuid once and keep it, mirroring what the website stores in
/// localStorage. A fresh uuid per launch would look like a flood of new visitors.
///
/// Signed in, this does nothing at all: the bearer token is the credential and no
/// request is made. That matters more than having a guest uuid ready in reserve - this
/// runs at the top of every entry point, and a blocking request there is multiplied by
/// however many entry points the app is running at once. A session that later dies is
/// dropped by `recover_session`, and the entry point after that fetches the uuid.
///
/// This must only be called from the top of a `Source` entry point, never while another
/// request is being assembled: it blocks on a request of its own, and the app runs a
/// limited number of requests at once, so nesting one inside another can wedge them all.
pub fn ensure_guest_uuid() {
	if is_logged_in() || stored(UUID_KEY).is_some() {
		return;
	}
	if fetch_guest_uuid().is_none() {
		println!("[ccc] ERROR could not obtain a guest uuid; requests may be refused");
	}
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

// ---------------------------------------------------------------------------
// Renewal
// ---------------------------------------------------------------------------

enum TokenOutcome {
	/// New tokens were issued and stored.
	Renewed,
	/// The server answered and refused. CCC replies `410 Cannot decrypt the refresh
	/// token` for one that is invalid or already spent.
	Rejected,
	/// No usable answer; worth retrying later.
	Unreachable,
}

/// POST the OAuth token endpoint.
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

	// The endpoint answers a successful grant with a bare OAuth2 payload rather than the
	// usual envelope; failures come back as `{code, message}` with a 4xx status.
	let Ok(parsed) = response.get_json_owned::<TokenResponse>() else {
		return refused(status);
	};
	let Some(access) = parsed.access_token.clone().filter(|value| !value.is_empty()) else {
		return refused(status);
	};

	set_access_token(&access);
	if let Some(refresh) = parsed.refresh_token.filter(|value| !value.is_empty()) {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	TokenOutcome::Renewed
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

/// Swap the refresh token for a fresh pair.
fn refresh() -> TokenOutcome {
	let Some(refresh_token) = stored(REFRESH_TOKEN_KEY) else {
		// Nothing to renew with: a session captured before refresh tokens were kept,
		// or one the web view handed over without one.
		return TokenOutcome::Rejected;
	};
	request_token(&[
		("grant_type", "refresh_token"),
		("client_id", CLIENT_ID),
		("client_secret", CLIENT_SECRET),
		("refresh_token", refresh_token.as_str()),
	])
}

/// React to a session that is no longer accepted: renew it, or drop it so the source
/// keeps working as a guest. Returns true when a fresh token is now stored.
///
/// Safe to call from several requests at once. CCC rotates the refresh token, so when a
/// library refresh has five requests fail together the first renewal succeeds and the
/// other four are refused with a token that is merely superseded - dropping the session
/// on that would undo the renewal that just worked. The stored token is therefore
/// compared before and after, and only a session that nobody replaced is cleared.
pub fn recover_session(used: Option<&str>) -> bool {
	let Some(before) = access_token() else {
		return false;
	};

	// Another request already renewed while this one was in flight, so the retry only
	// needs to go out with the token that is stored now. Without this, one expiry during
	// a library refresh becomes five simultaneous calls to the token endpoint.
	if used.is_some_and(|used| used != before) {
		return true;
	}

	let now = current_date();
	if now - timestamp(FAILED_AT_KEY) < FAIL_BACKOFF {
		return false;
	}

	match refresh() {
		TokenOutcome::Renewed => {
			defaults_set(FAILED_AT_KEY, DefaultValue::Null);
			defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
			println!("[ccc] session renewed");
			true
		}
		TokenOutcome::Rejected => {
			if access_token().as_deref() == Some(before.as_str()) {
				println!("[ccc] session cannot be renewed, falling back to guest");
				clear();
				defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
			}
			false
		}
		TokenOutcome::Unreachable => {
			set_timestamp(FAILED_AT_KEY, now);
			println!("[ccc] session renewal could not reach CCC, will retry later");
			false
		}
	}
}

// ---------------------------------------------------------------------------
// Signing in and out
// ---------------------------------------------------------------------------

/// Take the tokens out of whatever the web login view handed over.
///
/// The site keeps its session in `localStorage` (`accessToken` / `refreshToken`) rather
/// than in a cookie, which is why `settings.json` asks for those keys. The app merges
/// what it collected into this one map, so both are looked up here by name.
///
/// In practice this is never reached: the callback is driven by cookie updates and CCC
/// sets no cookies at all. `sync_from_web_view` is what actually picks the session up.
/// It stays because it costs nothing and would work if the app ever does deliver the
/// requested `localStorageKeys`.
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

	set_access_token(&access);
	if let Some(refresh) = values
		.get("refreshToken")
		.filter(|value| !value.is_empty())
		.cloned()
	{
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
	true
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

/// Open the site in a background web view, ready to read or write its storage.
fn open_site() -> Option<WebView> {
	let webview = WebView::new();
	let request = Request::get(BASE_URL).ok()?.header("User-Agent", USER_AGENT);
	webview.load_blocking(request).ok()?;
	webview.wait_for_load();
	Some(webview)
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
		// most likely the login view and this one do not share a data store. That
		// belongs in the log; what the reader gets is the step to take next.
		println!("[ccc] ERROR the site loaded but its storage held no accessToken");
		defaults_set(
			SYNC_RESULT_KEY,
			DefaultValue::String(String::from(
				"讀不到登入資料。請先按「登入」完成登入，再按一次「同步登入狀態」。",
			)),
		);
		return false;
	};

	set_access_token(&access);
	if let Some(refresh) = read_storage(&webview, "refreshToken") {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
	defaults_set(SYNC_RESULT_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
	true
}

/// Forget the session on this device.
pub fn clear() {
	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::Null);
	defaults_set(REFRESH_TOKEN_KEY, DefaultValue::Null);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	defaults_set(SEEN_KEYS_KEY, DefaultValue::Null);
	defaults_set(CALL_COUNT_KEY, DefaultValue::Null);
	defaults_set(SYNC_RESULT_KEY, DefaultValue::Null);
}

/// Sign out of CCC as well as out of Aidoku.
///
/// `clearCookiesOnLogOut` only clears cookies, and CCC keeps its session in
/// `localStorage`, so on its own it leaves the site signed in: the login page would come
/// back already authenticated and the sync button would restore the very same account.
/// Clearing the site's own storage is what makes "clear login" mean what it says.
pub fn clear_web_session() {
	match open_site() {
		Some(webview) => {
			// `eval` hands back whatever the script evaluates to, and that has to be a
			// string. `localStorage.removeItem(...)` evaluates to `undefined`, which
			// comes back as an error even though the removal itself worked - reporting
			// a failure that never happened. So the script ends by reading the keys
			// back, which gives `eval` its string *and* makes the log honest: empty
			// means the session really is gone.
			let names = SESSION_STORAGE_KEYS
				.iter()
				.map(|key| format!("'{key}'"))
				.collect::<Vec<String>>()
				.join(",");
			// Keys are literals defined in this file, so the quoting is safe.
			let script = format!(
				"var k=[{names}];\
				 k.forEach(function(n){{try{{localStorage.removeItem(n)}}catch(e){{}}}});\
				 k.filter(function(n){{return localStorage.getItem(n)}}).join(',')"
			);
			match webview.eval(&script) {
				Ok(left) if left.trim().is_empty() => println!("[ccc] site session cleared"),
				Ok(left) => {
					println!("[ccc] ERROR the site's storage still holds {left}")
				}
				Err(_) => println!("[ccc] ERROR could not run the storage clearing script"),
			}
		}
		None => println!("[ccc] ERROR could not open the web view to clear the site session"),
	}
	clear();
}

/// Aidoku posts `login` both when a login finishes and when the user logs out, and the
/// name alone does not say which; the flag set when a session was picked up is what
/// tells them apart.
///
/// Treating an unmarked notification as a sign-out is what clears the session, and that
/// matters more than it looks: closing the login page fires this *and* a full refresh of
/// settings, listings and content. Leaving a dead token in place there sends every one
/// of those refreshes out with a credential the server rejects.
pub fn handle_login_notification() {
	if defaults_get::<bool>(JUST_LOGGED_IN_KEY).unwrap_or(false) {
		defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	} else {
		clear();
	}
}

// ---------------------------------------------------------------------------
// Account summary
// ---------------------------------------------------------------------------

fn fetch_member() -> Outcome<Member> {
	match api_request("/member") {
		Ok(request) => send_api(request),
		Err(_) => Outcome::Unreachable,
	}
}

fn describe(member: Member) -> String {
	let name = member
		.nickname
		.filter(|value| !value.is_empty())
		.or(member.name)
		.unwrap_or_else(|| String::from("已登入"));
	format!(
		"{name}・金幣 {}・點數 {}",
		member.coin.unwrap_or(0),
		member.point.unwrap_or(0)
	)
}

/// The account summary shown in settings, or `None` to leave the group out entirely.
///
/// The group stays hidden while signed out and nothing is wrong, so readers who never
/// sign in are not shown an empty panel. It appears when there is something to say: the
/// balances once signed in, or which step failed when it did not take. Those balances
/// are the only visible proof that signing in actually worked, so a failure names itself
/// rather than going blank - and names the *right* failure: being offline is not the
/// same as being signed out, and telling a reader on a train to log in again invites
/// them to throw away a session that is perfectly good.
pub fn account_footer() -> Option<String> {
	if needs_relogin() {
		return Some(String::from(
			"登入已失效，無法自動恢復。請按「清除登入狀態」後重新登入。",
		));
	}

	if !is_logged_in() {
		// The stored reason is already written as something the reader can act on, so
		// it stands on its own.
		if let Some(reason) = stored(SYNC_RESULT_KEY) {
			return Some(reason);
		}
		// The login view ran but left no token. What it did hand over is a clue for us,
		// not for the reader, so it goes to the log and they get the next step.
		if let Some(seen) = stored(SEEN_KEYS_KEY) {
			let calls = defaults_get::<i32>(CALL_COUNT_KEY).unwrap_or(0);
			println!("[ccc] web login called back {calls} time(s) handing over: {seen}");
			return Some(String::from(
				"登入尚未完成。請按「登入」登入後，再按「同步登入狀態」。",
			));
		}
		// Signed out with nothing wrong: hide the group. The static footer on the
		// account group already explains the login and sync steps.
		return None;
	}

	let used = access_token();
	match fetch_member() {
		Outcome::Ok(member) => Some(describe(member)),
		// The token is dead. The reader is right here waiting, so renew it now and say
		// plainly which of the two failures happened if it does not work.
		Outcome::Unauthorized => {
			if recover_session(used.as_deref()) {
				if let Outcome::Ok(member) = fetch_member() {
					return Some(describe(member));
				}
			}
			if needs_relogin() {
				Some(String::from(
					"登入已失效，無法自動恢復。請按「清除登入狀態」後重新登入。",
				))
			} else {
				Some(String::from("登入暫時失效，稍後會自動重試，請稍後再回來看看。"))
			}
		}
		Outcome::Unreachable => Some(String::from(
			"目前無法連線到 CCC，暫時讀不到帳號資訊（登入狀態未變）。",
		)),
	}
}

