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
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;

use crate::crypto;
use crate::helper::{api_request, read_envelope, send_api, Outcome, API_URL, BASE_URL, DEVICE, USER_AGENT};

const UUID_KEY: &str = "guestUuid";
const ACCESS_TOKEN_KEY: &str = "accessToken";
const REFRESH_TOKEN_KEY: &str = "refreshToken";
/// Unix seconds at which the stored access token stops being accepted, when that is
/// knowable. Absent for a token whose lifetime the site never told us.
const EXPIRES_AT_KEY: &str = "tokenExpiresAt";
/// When renewal last failed to reach the server, so it is not retried on every request.
const FAILED_AT_KEY: &str = "tokenFailedAt";
/// Set when the session is dead and cannot be renewed, so the settings footer can say
/// so instead of leaving the reader guessing.
const NEEDS_RELOGIN_KEY: &str = "needsRelogin";
/// When `handle_web_login` last handed over a session. Written *only* there, so its
/// absence is what tells the `login` notification apart from a real sign-out - see
/// `handle_login_notification`.
const JUST_LOGGED_IN_KEY: &str = "justLoggedInAt";
/// Names of the entries the web login view actually handed over. Recorded so the
/// settings footer can say what arrived when no token could be found - the app's
/// delivery of `localStorageKeys` is undocumented and no shipped source relies on it.
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

/// Renew this many seconds before the stored expiry, so a token cannot die in flight.
const EXPIRY_MARGIN: i64 = 60;
/// How long to wait after renewal could not reach the server before trying again.
const FAIL_BACKOFF: i64 = 600;
/// How long after a captured sign-in the `login` notification still means "just signed
/// in" rather than "signed out".
const JUST_LOGGED_IN_TTL: i64 = 60;

/// The localStorage entries CCC keeps its session in.
const SESSION_STORAGE_KEYS: [&str; 3] = ["accessToken", "refreshToken", "userId"];

/// Mirrors "is there a session on this device" as a plain bool, purely so
/// `settings.json` can point `requires` / `requiresFalse` at it and grey out the button
/// that would do nothing. The app offers no busy state for a button, so which of the two
/// is tappable is the clearest signal available that the last press did something.
const LOGGED_IN_FLAG_KEY: &str = "loggedIn";

fn set_logged_in_flag(value: bool) {
	defaults_set(LOGGED_IN_FLAG_KEY, DefaultValue::Bool(value));
}

#[derive(Deserialize)]
struct TokenResponse {
	#[serde(default)]
	access_token: Option<String>,
	#[serde(default)]
	refresh_token: Option<String>,
	/// Seconds the new access token is good for. Absent on some grants, in which case
	/// the token's own `exp` claim is used instead.
	#[serde(default)]
	expires_in: Option<i64>,
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

/// Read the `exp` claim out of a JWT payload without pulling in a JWT crate.
///
/// The tokens CCC issues are opaque to us, so this is best effort: anything that does
/// not parse leaves the expiry unknown and the session falls back to renewing when a
/// request comes back 401.
fn jwt_expiry(token: &str) -> Option<i64> {
	let payload = token.split('.').nth(1)?;
	let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
	let text = String::from_utf8(decoded).ok()?;
	let marker = text.find("\"exp\"")?;
	let digits: String = text[marker + 5..]
		.chars()
		.take(32)
		.skip_while(|character| !character.is_ascii_digit())
		.take_while(|character| character.is_ascii_digit())
		.collect();
	digits.parse::<i64>().ok()
}

/// Store an access token together with whatever is known about when it dies.
fn store_access_token(access: &str, expires_in: Option<i64>) {
	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(String::from(access)));
	set_logged_in_flag(true);
	// Prefer the server's own countdown; fall back to the token's `exp` claim, which is
	// the only thing available for a session picked up out of the web view.
	//
	// An expiry already in the past is discarded rather than stored: a misread claim or
	// a skewed device clock would otherwise make `ensure_session` renew on every single
	// entry point, rotating the refresh token on each request with nothing to stop it.
	// Not knowing when the token dies is safe - that falls back to renewing on the
	// first 401 - while believing it died an hour ago is not.
	let expires_at = expires_in
		.map(|seconds| current_date() + seconds)
		.or_else(|| jwt_expiry(access))
		.filter(|expires_at| *expires_at > current_date());
	match expires_at {
		Some(value) => set_timestamp(EXPIRES_AT_KEY, value),
		None => defaults_set(EXPIRES_AT_KEY, DefaultValue::Null),
	}
}

// ---------------------------------------------------------------------------
// Credentials on the wire
// ---------------------------------------------------------------------------

/// Fetch a guest uuid once and keep it, mirroring what the website stores in
/// localStorage. A fresh uuid per launch would look like a flood of new visitors.
///
/// This runs even while signed in, so that a session which dies later can fall straight
/// back to guest browsing instead of failing every request.
///
/// This must only be called from the top of a `Source` entry point, never while another
/// request is being assembled: it blocks on a request of its own, and the app runs a
/// limited number of requests at once, so nesting one inside another can wedge them all.
pub fn ensure_guest_uuid() {
	if stored(UUID_KEY).is_some() {
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
/// fetching the uuid here would block that request on another one. `prepare` handles
/// that at the entry points instead.
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

/// Run at the top of every `Source` entry point, before any request is assembled.
pub fn prepare() {
	ensure_session();
	ensure_guest_uuid();
}

/// Renew the session before a request can fail on it.
///
/// Reads only defaults unless the stored token is already past its expiry, so the usual
/// case costs nothing at all. A token whose lifetime is unknown is left to the 401
/// handling in `api_get`.
fn ensure_session() {
	if !is_logged_in() {
		return;
	}
	let expires_at = timestamp(EXPIRES_AT_KEY);
	if expires_at == 0 || current_date() < expires_at - EXPIRY_MARGIN {
		return;
	}
	recover_session();
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

	store_access_token(&access, parsed.expires_in);
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
pub fn recover_session() -> bool {
	let Some(before) = access_token() else {
		return false;
	};

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

	store_access_token(&access, None);
	if let Some(refresh) = values
		.get("refreshToken")
		.filter(|value| !value.is_empty())
		.cloned()
	{
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	set_timestamp(JUST_LOGGED_IN_KEY, current_date());
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
			DefaultValue::String(String::from("無法載入 CCC 網頁")),
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

	store_access_token(&access, None);
	if let Some(refresh) = read_storage(&webview, "refreshToken") {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
	defaults_set(SYNC_RESULT_KEY, DefaultValue::Null);
	true
}

/// Forget the session on this device.
pub fn clear() {
	set_logged_in_flag(false);
	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::Null);
	defaults_set(REFRESH_TOKEN_KEY, DefaultValue::Null);
	defaults_set(EXPIRES_AT_KEY, DefaultValue::Null);
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
/// name alone does not say which.
///
/// A sign-out is only ever inferred when `handle_web_login` has actually handed a
/// session over at some point: that is the one situation in which the app is tracking
/// the login state itself and can genuinely be offering a logout. On CCC today it never
/// does - the site sets no cookies, so the callback never fires and the app's login row
/// never flips to "logged in" - which leaves this a no-op, and deliberately so. The
/// earlier version read every `login` notification as a possible sign-out and so could
/// only ever fire on an event whose meaning has never been observed, throwing away a
/// session the reader had just synced. Signing out runs through the explicit "clear
/// login" button instead.
pub fn handle_login_notification() {
	let marked_at = timestamp(JUST_LOGGED_IN_KEY);
	if marked_at == 0 {
		println!("[ccc] login notification received; stored session left unchanged");
		return;
	}
	if current_date() - marked_at < JUST_LOGGED_IN_TTL {
		// The notification for the sign-in that just happened.
		return;
	}
	println!("[ccc] logged out");
	clear_web_session();
}

// ---------------------------------------------------------------------------
// Account summary
// ---------------------------------------------------------------------------

fn fetch_member() -> Outcome<Member> {
	match api_request("/member") {
		Ok(request) => send_api(request),
		Err(_) => Outcome::Unreachable(String::from("無法建立請求")),
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
			"登入已失效且無法自動續期，請按「清除登入狀態」後重新登入。",
		));
	}

	if !is_logged_in() {
		// If the login view ran but left no token, name what it did hand over; that is
		// the one clue available for why sign-in did not take.
		if let Some(reason) = stored(SYNC_RESULT_KEY) {
			return Some(format!("同步失敗：{reason}"));
		}
		if let Some(seen) = stored(SEEN_KEYS_KEY) {
			let calls = defaults_get::<i32>(CALL_COUNT_KEY).unwrap_or(0);
			return Some(format!(
				"登入未完成：登入頁回呼 {calls} 次，交回「{seen}」，其中沒有 accessToken。請回報這行字。"
			));
		}
		// Signed out with nothing wrong: hide the group. The static footer on the
		// account group already explains the login and sync steps.
		return None;
	}

	match fetch_member() {
		Outcome::Ok(member) => Some(describe(member)),
		// The token is dead. The reader is right here waiting, so renew it now and say
		// plainly which of the two failures happened if it does not work.
		Outcome::Unauthorized => {
			if recover_session() {
				if let Outcome::Ok(member) = fetch_member() {
					return Some(describe(member));
				}
			}
			if needs_relogin() {
				Some(String::from(
					"登入已失效且無法自動續期，請按「清除登入狀態」後重新登入。",
				))
			} else {
				Some(String::from("登入已失效，正在重試續期，稍後再回來看看。"))
			}
		}
		Outcome::Unreachable(_) => Some(String::from(
			"目前無法連線到 CCC，暫時讀不到帳號資訊（登入狀態未變）。",
		)),
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// A Laravel Passport access token, whose payload carries `exp`. Reading it is what
	/// lets a session synced out of the web view - which comes with no `expires_in` -
	/// be renewed before a request fails on it.
	const JWT_WITH_EXP: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJhdWQiOiIyIiwianRpIjoiYWJjIiwiaWF0IjoxNzg5MDAwMDAwLCJuYmYiOjE3ODkwMDAwMDAsImV4cCI6MTc5MDAwMDAwMCwic3ViIjoiNDIiLCJzY29wZXMiOltdfQ.sig";
	const JWT_WITHOUT_EXP: &str =
		"eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJhdWQiOiIyIiwic3ViIjoiNDIifQ.sig";

	#[aidoku_test]
	fn reads_the_expiry_out_of_a_jwt() {
		assert_eq!(jwt_expiry(JWT_WITH_EXP), Some(1790000000));
	}

	/// Anything unreadable has to leave the expiry unknown rather than guess one: an
	/// invented expiry in the past would renew a perfectly good session on every call.
	#[aidoku_test]
	fn an_unreadable_token_has_no_known_expiry() {
		assert_eq!(jwt_expiry(JWT_WITHOUT_EXP), None);
		assert_eq!(jwt_expiry("not-a-jwt"), None);
		assert_eq!(jwt_expiry(""), None);
		assert_eq!(jwt_expiry("a.!!!!.c"), None);
	}
}
