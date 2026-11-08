//! Login state: token, credentials, account facts and the sign-in gate.
//!
//! Shaped after zh.komiic2 and zh.manwa. Only `ensure_session` (top of each Source entry
//! point, behind a time gate), the login handler and the settings page send requests;
//! building a request only reads defaults.

use crate::net::{self, Url};
use aidoku::{
	alloc::{string::ToString, String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::HttpMethod,
		std::current_date,
	},
	prelude::*,
};
use serde::Deserialize;

const TOKEN_KEY: &str = "auth_token";
const USERNAME_KEY: &str = "auth_username";
const PASSWORD_KEY: &str = "auth_password";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
const NEXT_LOGIN_KEY: &str = "auth_next_login_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";
/// Why the last manual login failed. The login dialog only shows a generic message, so the
/// settings footer is where the reason is read.
const LAST_LOGIN_ERROR_KEY: &str = "auth_last_login_error";

pub const REJECTED_TEXT: &str = "帳號或密碼錯誤";
pub const UNREACHABLE_TEXT: &str = "無法連線到站方，請稍後再試";

/// The shortest time between two automatic sign-ins. The gate moves before the request is
/// sent, so entry points running together do not each sign in.
const LOGIN_RETRY: i64 = 60;
/// How long to wait after the sign-in request got no usable answer.
const FAIL_BACKOFF: i64 = 600;
/// A token with less than this left (seconds) is renewed before it is used.
const RENEW_BEFORE: i64 = 86400;
/// How long after a successful login the `login` notification still means
/// "just logged in" rather than "logged out".
const JUST_LOGGED_IN_TTL: i64 = 60;

// ---------------------------------------------------------------------------
// Stored state
// ---------------------------------------------------------------------------

fn stored(key: &str) -> Option<String> {
	defaults_get::<String>(key).filter(|value| !value.is_empty())
}

fn store(key: &str, value: &str) {
	defaults_set(key, DefaultValue::String(String::from(value)));
}

/// The stored token, sent as the Authorization header as it is. Reads defaults only.
pub fn token() -> Option<String> {
	stored(TOKEN_KEY)
}

pub fn logged_in() -> bool {
	token().is_some()
}

/// Username and password, kept so an expired token can be renewed silently.
fn credentials() -> Option<(String, String)> {
	Some((stored(USERNAME_KEY)?, stored(PASSWORD_KEY)?))
}

/// Wipe everything auth related. Used on logout and when the stored session turns out to
/// be unrecoverable.
pub fn clear_auth() {
	for key in [
		TOKEN_KEY,
		USERNAME_KEY,
		PASSWORD_KEY,
		LOGGED_IN_AT_KEY,
		NEXT_LOGIN_KEY,
		NEEDS_RELOGIN_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
	// An empty string rather than Null: the test runner keeps a key that is set to Null,
	// and `stored` reads "" as absent.
	store(LAST_LOGIN_ERROR_KEY, "");
	// The check-in gate belongs to the account; the next account checks in on its own.
	crate::punch::clear();
}

/// Keep the reason a manual login failed, for the settings footer.
pub fn record_login_failure(reason: &str) {
	store(LAST_LOGIN_ERROR_KEY, reason);
}

/// Set when the stored session is dead and cannot be renewed: the password was rejected.
/// The app's own login row keeps showing "logged in" in this state, so the settings footer
/// uses this flag to tell the user to log out and back in. Nothing signs in automatically
/// while it is set; a successful manual login clears it.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

/// Unix seconds stored as a string, so the value cannot overflow an i32.
fn timestamp(key: &str) -> i64 {
	defaults_get::<String>(key)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0)
}

fn set_timestamp(key: &str, value: i64) {
	store(key, &value.to_string());
}

// The app fires the same `login` notification for both logging in and logging out. A login
// timestamp lets the handler tell the two apart. It expires, so a notification that never
// arrived cannot make a later logout look like a login and leave the credentials behind.
pub fn mark_just_logged_in() {
	set_timestamp(LOGGED_IN_AT_KEY, current_date());
}

pub fn is_just_logged_in() -> bool {
	current_date() - timestamp(LOGGED_IN_AT_KEY) < JUST_LOGGED_IN_TTL
}

pub fn clear_just_logged_in() {
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
}

/// A response said the stored token is dead (HTTP 401). The token goes; the credentials stay,
/// so the next entry point signs in again. `ensure_session` has already moved its gate for
/// the sign-in that produced this token, so repeated 401s cannot make it loop.
pub fn on_unauthorized() {
	defaults_set(TOKEN_KEY, DefaultValue::Null);
}

// ---------------------------------------------------------------------------
// Token expiry
// ---------------------------------------------------------------------------

fn base64url_decode(text: &str) -> Option<Vec<u8>> {
	let mut out = Vec::new();
	let mut bits: u32 = 0;
	let mut count = 0;
	for byte in text.bytes() {
		let value = match byte {
			b'A'..=b'Z' => byte - b'A',
			b'a'..=b'z' => byte - b'a' + 26,
			b'0'..=b'9' => byte - b'0' + 52,
			b'-' | b'+' => 62,
			b'_' | b'/' => 63,
			b'=' => continue,
			_ => return None,
		};
		bits = (bits << 6) | u32::from(value);
		count += 6;
		if count >= 8 {
			count -= 8;
			out.push((bits >> count) as u8);
			bits &= (1 << count) - 1;
		}
	}
	Some(out)
}

/// The `exp` claim of a JWT, in unix seconds. `None` when the token is not a JWT or has none.
pub fn token_expiry(token: &str) -> Option<i64> {
	#[derive(Deserialize)]
	struct Claims {
		exp: Option<i64>,
	}
	let payload = base64url_decode(token.split('.').nth(1)?)?;
	serde_json::from_slice::<Claims>(&payload).ok()?.exp
}

/// A token that expires within `RENEW_BEFORE` is renewed. One whose expiry cannot be read is
/// used as it is, and a 401 settles it.
fn needs_renewal(token: &str, now: i64) -> bool {
	token_expiry(token).is_some_and(|exp| exp - now < RENEW_BEFORE)
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

pub enum LoginOutcome {
	/// The site accepted the credentials and issued this token.
	Token(String),
	/// The site answered that the credentials are wrong (HTTP 400 or 401).
	Rejected,
	/// No usable answer (network error, server error, unexpected body). Worth retrying later.
	Unreachable,
}

#[derive(Deserialize)]
struct LoginEnvelope {
	data: Option<LoginData>,
}

#[derive(Deserialize)]
struct LoginData {
	#[serde(default)]
	token: String,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
	#[serde(default)]
	error: String,
}

/// Read a `auth/sign-in` answer from its status and body. Only HTTP 400 with error `1004`
/// ("invalid email or password") means the credentials are wrong; any other failure is not
/// a reason to stop signing in.
pub fn parse_login(status: i32, text: &str) -> LoginOutcome {
	match status {
		400 => match serde_json::from_str::<ErrorEnvelope>(text) {
			Ok(envelope) if envelope.error == "1004" => LoginOutcome::Rejected,
			_ => LoginOutcome::Unreachable,
		},
		200..=299 => match serde_json::from_str::<LoginEnvelope>(text) {
			Ok(LoginEnvelope {
				data: Some(LoginData { token }),
			}) if !token.is_empty() => LoginOutcome::Token(token),
			_ => LoginOutcome::Unreachable,
		},
		_ => LoginOutcome::Unreachable,
	}
}

/// POST auth/sign-in with these credentials on the selected line.
pub fn login(username: &str, password: &str) -> LoginOutcome {
	let base = net::api_base();
	let body = serde_json::json!({ "email": username, "password": password }).to_string();
	let Ok(request) = net::create_request(
		format!("{base}/auth/sign-in"),
		&base,
		HttpMethod::Post,
		Some(body),
		false,
	) else {
		return LoginOutcome::Unreachable;
	};
	let Ok(response) = request.send() else {
		return LoginOutcome::Unreachable;
	};
	match response.get_string() {
		Ok(text) => parse_login(response.status_code(), &text),
		Err(_) => LoginOutcome::Unreachable,
	}
}

/// Keep a fresh login: token and credentials, and forget any earlier failure.
pub fn save_login(token: &str, username: &str, password: &str) {
	store(TOKEN_KEY, token);
	store(USERNAME_KEY, username);
	store(PASSWORD_KEY, password);
	store(LAST_LOGIN_ERROR_KEY, "");
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	set_timestamp(NEXT_LOGIN_KEY, current_date() + LOGIN_RETRY);
	// Last, so the window in which the app's `login` notification still counts as a login
	// rather than a logout starts after everything above is stored.
	mark_just_logged_in();
}

/// Called once at the top of each Source entry point (`get_search_manga_list`,
/// `get_manga_update`, `get_page_list`, `get_manga_list`, `get_home`). Never from request
/// helpers.
///
/// Signs in again when the stored token is missing or has less than a day left, at most once
/// per `LOGIN_RETRY`. The gate moves before the request is sent, and every outcome moves it
/// again (success and an unreachable site by their own delay, a rejected password by the
/// `needs_relogin` flag, which stops this function until a manual login). Signed out, or
/// flagged for a manual re-login, nothing is sent. The token's expiry is read from the token
/// itself, so there is no probe request.
pub fn ensure_session() {
	if needs_relogin() {
		return;
	}
	let Some((username, password)) = credentials() else {
		return;
	};
	let now = current_date();
	if token().is_some_and(|token| !needs_renewal(&token, now)) {
		return;
	}
	if now < timestamp(NEXT_LOGIN_KEY) {
		return;
	}
	set_timestamp(NEXT_LOGIN_KEY, now + LOGIN_RETRY);

	match login(&username, &password) {
		LoginOutcome::Token(token) => {
			store(TOKEN_KEY, &token);
			set_timestamp(NEXT_LOGIN_KEY, now + LOGIN_RETRY);
		}
		LoginOutcome::Rejected => {
			clear_auth();
			defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
		}
		LoginOutcome::Unreachable => set_timestamp(NEXT_LOGIN_KEY, now + FAIL_BACKOFF),
	}
}

// ---------------------------------------------------------------------------
// Account facts
// ---------------------------------------------------------------------------

/// What `users/profile` says about the account.
#[derive(Debug, PartialEq)]
pub struct Profile {
	pub name: String,
	pub level: i32,
	pub exp: i64,
	pub title: String,
	pub is_punched: bool,
}

#[derive(Deserialize)]
struct ProfileEnvelope {
	data: Option<ProfileData>,
}

#[derive(Deserialize)]
struct ProfileData {
	user: Option<ProfileUser>,
}

#[derive(Deserialize)]
struct ProfileUser {
	#[serde(default)]
	name: String,
	#[serde(default)]
	level: i32,
	#[serde(default)]
	exp: i64,
	#[serde(default)]
	title: String,
	#[serde(rename = "isPunched", default)]
	is_punched: bool,
}

/// Read a `users/profile` body. `None` when it does not carry the account.
pub fn parse_profile(text: &str) -> Option<Profile> {
	let user = serde_json::from_str::<ProfileEnvelope>(text)
		.ok()?
		.data?
		.user?;
	Some(Profile {
		name: user.name.trim().into(),
		level: user.level,
		exp: user.exp,
		title: user.title.trim().into(),
		is_punched: user.is_punched,
	})
}

pub enum ProfileState {
	Profile(Profile),
	/// The site says the token is dead (HTTP 401).
	Expired,
	/// No usable answer.
	Failed,
}

pub fn classify_profile(status: i32, body: Option<&str>) -> ProfileState {
	if status == 401 {
		return ProfileState::Expired;
	}
	match body.and_then(parse_profile) {
		Some(profile) if (200..300).contains(&status) => ProfileState::Profile(profile),
		_ => ProfileState::Failed,
	}
}

/// GET users/profile with the stored token. Only the settings page calls this.
fn fetch_profile() -> ProfileState {
	let Ok(request) = net::build_request(&Url::Profile) else {
		return ProfileState::Failed;
	};
	let Ok(response) = request.send() else {
		return ProfileState::Failed;
	};
	let state = classify_profile(
		response.status_code(),
		response.get_string().ok().as_deref(),
	);
	if matches!(state, ProfileState::Expired) {
		on_unauthorized();
	}
	state
}

// ---------------------------------------------------------------------------
// Settings page text
// ---------------------------------------------------------------------------

/// Shown when the stored session is dead and cannot be renewed. The app's own login row
/// still says "logged in" then, so the user has to be told to log out first.
pub const RELOGIN_HINT: &str = "登入已失效，請登出再重新登入";
/// Shown while the stored token is gone but the credentials are kept.
pub const RENEW_HINT: &str = "登入已過期，重新載入內容時會自動重新登入";
/// Shown while signed out. The group cannot simply be left out: a source that hands the app
/// an empty list of dynamic settings gets a settings screen whose buttons stop responding.
pub const SIGNED_OUT_HINT: &str = "尚未登入。登入後可以看到全部內容與收藏，並在這裡看到帳號資訊。";
pub const PROFILE_FAILED_HINT: &str = "暫時讀不到帳號資訊，請稍後重新打開這一頁";

/// The account summary under the login row. Signed out and every failure state read
/// defaults only; only a stored token makes it ask the site for the profile.
pub fn account_footer() -> String {
	if needs_relogin() {
		return String::from(RELOGIN_HINT);
	}
	if !logged_in() {
		if credentials().is_some() {
			return String::from(RENEW_HINT);
		}
		return signed_out_text(&stored(LAST_LOGIN_ERROR_KEY).unwrap_or_default());
	}
	match fetch_profile() {
		ProfileState::Profile(profile) => match crate::punch::last_error() {
			Some(error) if !profile.is_punched => {
				format!("{}\n{error}", describe_account(&profile))
			}
			_ => describe_account(&profile),
		},
		ProfileState::Expired => String::from(RENEW_HINT),
		ProfileState::Failed => String::from(PROFILE_FAILED_HINT),
	}
}

/// The signed-out text, led by why the last login failed when there is a reason.
fn signed_out_text(last_error: &str) -> String {
	if last_error.is_empty() {
		String::from(SIGNED_OUT_HINT)
	} else {
		format!("上次登入失敗：{last_error}\n{SIGNED_OUT_HINT}")
	}
}

pub fn describe_account(profile: &Profile) -> String {
	let mut lines = Vec::new();
	lines.push(if profile.name.is_empty() {
		String::from("已登入")
	} else {
		format!("已登入：{}", profile.name)
	});
	lines.push(if profile.title.is_empty() {
		format!("等級：{}", profile.level)
	} else {
		format!("等級：{}（{}）", profile.level, profile.title)
	});
	lines.push(format!("經驗：{}", profile.exp));
	lines.push(String::from(if profile.is_punched {
		"今日打卡：已完成"
	} else {
		"今日打卡：尚未打卡"
	}));
	lines.join("\n")
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// Header `{"alg":"HS256"}`, payload `{"exp":1800000000,"iat":1799395200}`.
	const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjE4MDAwMDAwMDAsImlhdCI6MTc5OTM5NTIwMH0.sig";

	#[aidoku_test]
	fn login_outcomes() {
		let ok = r#"{"code":200,"message":"success","data":{"token":"abc"}}"#;
		assert!(matches!(parse_login(200, ok), LoginOutcome::Token(t) if t == "abc"));
		let wrong = r#"{"code":400,"error":"1004","message":"invalid email or password"}"#;
		assert!(matches!(parse_login(400, wrong), LoginOutcome::Rejected));
		// Another 400, a 401 and a 400 that is not JSON are not "wrong password".
		let other = r#"{"code":400,"error":"1001","message":"bad request"}"#;
		assert!(matches!(parse_login(400, other), LoginOutcome::Unreachable));
		assert!(matches!(parse_login(400, "{}"), LoginOutcome::Unreachable));
		assert!(matches!(
			parse_login(400, "<html>"),
			LoginOutcome::Unreachable
		));
		assert!(matches!(parse_login(401, wrong), LoginOutcome::Unreachable));
		// A 200 without a token, a server error and a page that is not JSON are not "wrong
		// password".
		assert!(matches!(
			parse_login(200, r#"{"code":200,"data":{}}"#),
			LoginOutcome::Unreachable
		));
		assert!(matches!(parse_login(502, wrong), LoginOutcome::Unreachable));
		assert!(matches!(
			parse_login(200, "<html>"),
			LoginOutcome::Unreachable
		));
	}

	#[aidoku_test]
	fn jwt_expiry() {
		assert_eq!(token_expiry(JWT), Some(1800000000));
		assert_eq!(token_expiry("not a token"), None);
		assert_eq!(token_expiry("a.b.c"), None);
		assert_eq!(token_expiry(""), None);
	}

	#[aidoku_test]
	fn renewal_window() {
		let exp = 1800000000;
		// More than a day left: used as it is.
		assert!(!needs_renewal(JWT, exp - 2 * 86400));
		// Less than a day left, or already expired: renewed first.
		assert!(needs_renewal(JWT, exp - 3600));
		assert!(needs_renewal(JWT, exp + 10));
		// An expiry that cannot be read is never a reason to sign in.
		assert!(!needs_renewal("opaque", exp));
	}

	#[aidoku_test]
	fn base64url() {
		assert_eq!(base64url_decode("TWFu"), Some(Vec::from(*b"Man")));
		assert_eq!(base64url_decode("TWE"), Some(Vec::from(*b"Ma")));
		assert_eq!(base64url_decode("TQ=="), Some(Vec::from(*b"M")));
		assert_eq!(base64url_decode("a b"), None);
	}

	#[aidoku_test]
	fn profile_answers() {
		let body = r#"{"code":200,"data":{"user":{"name":"甲","level":3,"exp":120,"title":"萌新","isPunched":true}}}"#;
		assert!(matches!(
			classify_profile(200, Some(body)),
			ProfileState::Profile(p) if p.name == "甲" && p.level == 3 && p.is_punched
		));
		assert!(matches!(
			classify_profile(401, Some("{}")),
			ProfileState::Expired
		));
		assert!(matches!(classify_profile(401, None), ProfileState::Expired));
		assert!(matches!(
			classify_profile(500, Some(body)),
			ProfileState::Failed
		));
		assert!(matches!(
			classify_profile(200, Some(r#"{"code":200,"data":{}}"#)),
			ProfileState::Failed
		));
		assert!(matches!(classify_profile(200, None), ProfileState::Failed));
	}

	#[aidoku_test]
	fn signed_out_text_leads_with_the_reason() {
		assert_eq!(signed_out_text(""), SIGNED_OUT_HINT);
		let text = signed_out_text(REJECTED_TEXT);
		assert!(text.starts_with("上次登入失敗：帳號或密碼錯誤\n"));
		assert!(text.ends_with(SIGNED_OUT_HINT));
	}

	/// Signed out, the footer is a fixed text and sends no request.
	#[aidoku_test]
	fn signed_out_footer_is_never_empty() {
		clear_auth();
		assert_eq!(account_footer(), SIGNED_OUT_HINT);
		record_login_failure(UNREACHABLE_TEXT);
		assert!(account_footer().contains(UNREACHABLE_TEXT));
		clear_auth();
		assert_eq!(account_footer(), SIGNED_OUT_HINT);
	}

	#[aidoku_test]
	fn account_text() {
		let mut profile = Profile {
			name: String::from("甲"),
			level: 3,
			exp: 120,
			title: String::from("萌新"),
			is_punched: false,
		};
		assert_eq!(
			describe_account(&profile),
			"已登入：甲\n等級：3（萌新）\n經驗：120\n今日打卡：尚未打卡"
		);
		profile.name = String::new();
		profile.title = String::new();
		profile.is_punched = true;
		assert_eq!(
			describe_account(&profile),
			"已登入\n等級：3\n經驗：120\n今日打卡：已完成"
		);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/auth.rs"]
mod fixture_tests;
