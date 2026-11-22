//! Login state: credentials, the renewal gate, account facts and the daily sign-in.
//!
//! Shaped after zh.picacomic2. Only `ensure_session` (top of each Source entry point, behind
//! a time gate), the login handler, the logout notification, the settings page and the
//! upstream daily sign-in send requests; `fetch_json` only marks a dead session.
//!
//! The site keeps the session in a cookie (`NOY_SESSION`, about 30 days) bound to the host
//! that answered the login, so switching the server domain leaves the new host without a
//! session. `ensure_session` signs in again when the stored domain differs.

use aidoku::{
	alloc::{string::ToString, String, Vec},
	helpers::uri::QueryParameters,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		std::current_date,
	},
	prelude::*,
};
use serde::Deserialize;

use crate::helpers::{api_post, fetch_json, get_adult_mode, get_server_domain};
use crate::models::{SigninRecordResp, UserInfoResp};

const USERNAME_KEY: &str = "auth_username";
const PASSWORD_KEY: &str = "auth_password";
/// The server domain the stored session belongs to.
const DOMAIN_KEY: &str = "auth_domain";
/// Set by `fetch_json` when the site answered `{"status":"login"}`.
const EXPIRED_KEY: &str = "auth_session_expired";
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
/// How long after a successful login the `login` notification still means
/// "just logged in" rather than "logged out".
const JUST_LOGGED_IN_TTL: i64 = 60;

// Upstream daily sign-in (kept as in the official source).
const LAST_SIGNIN_DAY_KEY: &str = "lastSigninDay";
const UTC8_OFFSET: i64 = 28800;
const SECS_PER_DAY: i64 = 86400;

// ---------------------------------------------------------------------------
// Stored state
// ---------------------------------------------------------------------------

fn stored(key: &str) -> Option<String> {
	defaults_get::<String>(key).filter(|value| !value.is_empty())
}

fn store(key: &str, value: &str) {
	defaults_set(key, DefaultValue::String(value.into()));
}

fn flag(key: &str) -> bool {
	defaults_get::<bool>(key).unwrap_or(false)
}

fn set_flag(key: &str, value: bool) {
	defaults_set(
		key,
		if value {
			DefaultValue::Bool(true)
		} else {
			DefaultValue::Null
		},
	);
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

fn credentials() -> Option<(String, String)> {
	Some((stored(USERNAME_KEY)?, stored(PASSWORD_KEY)?))
}

pub fn is_logged_in() -> bool {
	credentials().is_some()
}

/// Set when the password was rejected. Nothing signs in automatically while it is set; a
/// successful manual login clears it.
pub fn needs_relogin() -> bool {
	flag(NEEDS_RELOGIN_KEY)
}

/// A response said the session is gone. Only a flag: the next entry point signs in again.
pub fn mark_expired() {
	set_flag(EXPIRED_KEY, true);
}

fn session_expired() -> bool {
	flag(EXPIRED_KEY)
}

/// Wipe everything auth related. Used on logout.
pub fn clear_auth() {
	for key in [
		USERNAME_KEY,
		PASSWORD_KEY,
		DOMAIN_KEY,
		EXPIRED_KEY,
		LOGGED_IN_AT_KEY,
		NEXT_LOGIN_KEY,
		NEEDS_RELOGIN_KEY,
		LAST_SIGNIN_DAY_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
	// An empty string rather than Null: the test runner keeps a key that is set to Null,
	// and `stored` reads "" as absent.
	store(LAST_LOGIN_ERROR_KEY, "");
}

pub fn record_login_failure(reason: &str) {
	store(LAST_LOGIN_ERROR_KEY, reason);
}

// The app fires the same `login` notification for both logging in and logging out. A login
// timestamp lets the handler tell the two apart. It expires, so a notification that never
// arrived cannot make a later logout look like a login and leave the credentials behind.
pub fn is_just_logged_in() -> bool {
	current_date() - timestamp(LOGGED_IN_AT_KEY) < JUST_LOGGED_IN_TTL
}

pub fn clear_just_logged_in() {
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum LoginOutcome {
	Ok,
	/// `error` (wrong account or password) or `danger` (characters the site refuses), the
	/// two failures the web client names (`auth-*.js`).
	Rejected,
	Unreachable,
}

#[derive(Deserialize)]
struct LoginResp {
	status: Option<String>,
}

pub fn parse_login(body: &[u8]) -> LoginOutcome {
	match serde_json::from_slice::<LoginResp>(body)
		.ok()
		.and_then(|resp| resp.status)
		.as_deref()
	{
		Some("ok") => LoginOutcome::Ok,
		Some("error" | "danger") => LoginOutcome::Rejected,
		_ => LoginOutcome::Unreachable,
	}
}

/// One `POST /api/login`. The session cookie lands in the app's cookie store.
pub fn login(username: &str, password: &str) -> LoginOutcome {
	let mut body = QueryParameters::new();
	body.push("user", Some(username));
	body.push("pass", Some(password));
	let Ok(request) = api_post("/api/login", &body.to_string(), get_adult_mode()) else {
		return LoginOutcome::Unreachable;
	};
	match request.data() {
		Ok(data) => parse_login(&data),
		Err(_) => LoginOutcome::Unreachable,
	}
}

/// Keep a fresh manual login and forget any earlier failure.
pub fn save_login(username: &str, password: &str) {
	store(USERNAME_KEY, username);
	store(PASSWORD_KEY, password);
	store(DOMAIN_KEY, &get_server_domain());
	store(LAST_LOGIN_ERROR_KEY, "");
	set_flag(EXPIRED_KEY, false);
	set_flag(NEEDS_RELOGIN_KEY, false);
	// Open, so switching the server right after logging in signs in on the new domain.
	set_timestamp(NEXT_LOGIN_KEY, 0);
	// Last, so the window in which the app's `login` notification still counts as a login
	// starts after everything above is stored.
	set_timestamp(LOGGED_IN_AT_KEY, current_date());
}

/// Called once at the top of each Source entry point. Never from request helpers.
///
/// Signs in again when a response marked the session dead, or when the server domain is not
/// the one the session belongs to, at most once per `LOGIN_RETRY`. The gate moves before the
/// request is sent, and every outcome moves it again: success clears the reason, a rejected
/// password sets `needs_relogin` (which stops this function until a manual login), and an
/// unreachable site waits `FAIL_BACKOFF`.
pub fn ensure_session() {
	if needs_relogin() {
		return;
	}
	let Some((username, password)) = credentials() else {
		return;
	};
	let domain = get_server_domain();
	let same_domain = stored(DOMAIN_KEY).as_deref() == Some(domain.as_str());
	if same_domain && !session_expired() {
		return;
	}
	let now = current_date();
	if now < timestamp(NEXT_LOGIN_KEY) {
		return;
	}
	set_timestamp(NEXT_LOGIN_KEY, now + LOGIN_RETRY);

	match login(&username, &password) {
		LoginOutcome::Ok => {
			store(DOMAIN_KEY, &domain);
			set_flag(EXPIRED_KEY, false);
		}
		LoginOutcome::Rejected => set_flag(NEEDS_RELOGIN_KEY, true),
		LoginOutcome::Unreachable => set_timestamp(NEXT_LOGIN_KEY, now + FAIL_BACKOFF),
	}
}

/// Ends the site session. Called from the logout notification, after the local state is gone.
pub fn logout() {
	if let Ok(request) = api_post("/api/logout", "", get_adult_mode()) {
		_ = request.send();
	}
}

// ---------------------------------------------------------------------------
// Daily sign-in (upstream behaviour, unchanged apart from the credential check)
// ---------------------------------------------------------------------------

fn today_utc8() -> String {
	let day = (current_date() + UTC8_OFFSET) / SECS_PER_DAY;
	format!("{day}")
}

pub fn try_daily_signin() {
	if !is_logged_in() {
		return;
	}
	if !defaults_get::<bool>("auto_signin").unwrap_or(false) {
		return;
	}
	let today = today_utc8();
	if defaults_get::<String>(LAST_SIGNIN_DAY_KEY).as_deref() == Some(today.as_str()) {
		return;
	}
	let adult = get_adult_mode();
	let Ok(record) =
		fetch_json::<SigninRecordResp>(|| api_post("/api/v4/signin/record", "", adult))
	else {
		return;
	};
	if record.today == Some(true) {
		defaults_set(LAST_SIGNIN_DAY_KEY, DefaultValue::String(today));
		return;
	}
	let Ok(request) = api_post("/api/v4/signin/sign", "", adult) else {
		return;
	};
	if request.send().is_ok() {
		defaults_set(LAST_SIGNIN_DAY_KEY, DefaultValue::String(today));
	}
}

// ---------------------------------------------------------------------------
// Settings page text
// ---------------------------------------------------------------------------

/// Shown when the password was rejected. The app's own login row still says "logged in",
/// so the user has to be told to log out first.
pub const RELOGIN_HINT: &str = "登入已失效，請登出再重新登入";
pub const RENEW_HINT: &str = "登入已過期，重新載入內容時會自動重新登入";
/// Shown while signed out. The group cannot simply be left out: a source that hands the app
/// an empty list of dynamic settings gets a settings screen whose buttons stop responding.
pub const SIGNED_OUT_HINT: &str =
	"尚未登入。這個網站的所有內容都要登入才看得到，登入後會在這裡顯示帳號資訊。";
pub const PROFILE_FAILED_HINT: &str = "暫時讀不到帳號資訊，請稍後重新打開這一頁";

pub struct Account {
	pub name: String,
	pub points: i64,
	pub signed_today: Option<bool>,
	pub continuous: Option<i64>,
}

/// The account summary under the account group. Signed out and every failure state read
/// defaults only; only stored credentials make it ask the site.
pub fn account_footer() -> String {
	if needs_relogin() {
		return String::from(RELOGIN_HINT);
	}
	if !is_logged_in() {
		return signed_out_text(&stored(LAST_LOGIN_ERROR_KEY).unwrap_or_default());
	}
	if session_expired() {
		return String::from(RENEW_HINT);
	}
	let adult = get_adult_mode();
	let info = match fetch_json::<UserInfoResp>(|| api_post("/api/v3/userinfo", "", adult)) {
		Ok(UserInfoResp {
			userinfo: Some(info),
		}) => info,
		Ok(_) => return String::from(PROFILE_FAILED_HINT),
		Err(_) if session_expired() => return String::from(RENEW_HINT),
		Err(_) => return String::from(PROFILE_FAILED_HINT),
	};
	// The sign-in record is extra; the account lines stand without it.
	let record =
		fetch_json::<SigninRecordResp>(|| api_post("/api/v4/signin/record", "", adult)).ok();
	describe_account(&Account {
		name: info.username.unwrap_or_default(),
		points: info.integral.unwrap_or(0),
		signed_today: record.as_ref().and_then(|r| r.today),
		continuous: record.and_then(|r| r.continuous),
	})
}

fn signed_out_text(last_error: &str) -> String {
	if last_error.is_empty() {
		String::from(SIGNED_OUT_HINT)
	} else {
		format!("上次登入失敗：{last_error}\n{SIGNED_OUT_HINT}")
	}
}

pub fn describe_account(account: &Account) -> String {
	let mut lines = Vec::new();
	lines.push(if account.name.is_empty() {
		String::from("已登入")
	} else {
		format!("已登入：{}", account.name)
	});
	lines.push(format!("積分：{}", account.points));
	match account.signed_today {
		Some(true) => lines.push(String::from("今日簽到：已簽到")),
		Some(false) => lines.push(String::from("今日簽到：尚未簽到")),
		None => {}
	}
	if let Some(days) = account.continuous.filter(|days| *days > 0) {
		lines.push(format!("連續簽到：{days} 天"));
	}
	lines.join("\n")
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn login_statuses() {
		assert_eq!(parse_login(br#"{"status":"ok"}"#), LoginOutcome::Ok);
		assert_eq!(
			parse_login(br#"{"status":"error"}"#),
			LoginOutcome::Rejected
		);
		assert_eq!(
			parse_login(br#"{"status":"danger"}"#),
			LoginOutcome::Rejected
		);
		assert_eq!(parse_login(b"<html>"), LoginOutcome::Unreachable);
		assert_eq!(
			parse_login(br#"{"status":"login"}"#),
			LoginOutcome::Unreachable
		);
	}

	#[aidoku_test]
	fn signed_out_footer_is_never_empty() {
		assert!(!signed_out_text("").is_empty());
		assert!(signed_out_text(REJECTED_TEXT).starts_with("上次登入失敗"));
	}

	#[aidoku_test]
	fn account_text() {
		let text = describe_account(&Account {
			name: "abc".into(),
			points: 120,
			signed_today: Some(true),
			continuous: Some(3),
		});
		assert_eq!(
			text,
			"已登入：abc\n積分：120\n今日簽到：已簽到\n連續簽到：3 天"
		);
		let text = describe_account(&Account {
			name: String::new(),
			points: 0,
			signed_today: None,
			continuous: Some(0),
		});
		assert_eq!(text, "已登入\n積分：0");
	}
}
