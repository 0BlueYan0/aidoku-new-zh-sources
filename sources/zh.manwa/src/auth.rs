//! Login state: token, credentials, account facts and the renewal gate.
//!
//! Shaped after zh.komiic2's `auth.rs`. Only `ensure_session` (top of each Source entry
//! point, behind a time gate) and the login handler send requests; everything else reads
//! defaults.

use aidoku::{
	alloc::{string::ToString, String},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
};
use serde::Deserialize;

use crate::helper::base_url;

const TOKEN_KEY: &str = "auth_token";
const USERNAME_KEY: &str = "auth_username";
const PASSWORD_KEY: &str = "auth_password";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
const NEXT_CHECK_KEY: &str = "auth_next_check_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";
const NICKNAME_KEY: &str = "auth_nickname";
const IS_VIP_KEY: &str = "auth_is_vip";
const VIP_EXPIRES_KEY: &str = "auth_vip_expires";
/// Why the last manual login failed. The login dialog only shows a generic message, so the
/// settings footer is where the reason is read.
const LAST_LOGIN_ERROR_KEY: &str = "auth_last_login_error";

pub const REJECTED_TEXT: &str = "帳號或密碼錯誤";
pub const UNREACHABLE_TEXT: &str = "無法連線到站方，請稍後再試";

/// How long a verified session is trusted before probing again.
const CHECK_INTERVAL: i64 = 1800;
/// How soon to probe again when the probe itself could not reach the server.
const PROBE_RETRY: i64 = 60;
/// How long to wait after the server answered with an error before probing again.
const FAIL_BACKOFF: i64 = 600;
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

fn token() -> Option<String> {
	stored(TOKEN_KEY)
}

/// `Some("Bearer <token>")` when logged in. Reads defaults only, never sends a request.
pub fn bearer() -> Option<String> {
	token().map(|token| format!("Bearer {token}"))
}

/// Whether the logged-in account is a site VIP. Reads defaults only.
pub fn is_vip() -> bool {
	defaults_get::<bool>(IS_VIP_KEY).unwrap_or(false)
}

/// Whether a token is stored.
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
		NEXT_CHECK_KEY,
		NEEDS_RELOGIN_KEY,
		NICKNAME_KEY,
		IS_VIP_KEY,
		VIP_EXPIRES_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
	// An empty string rather than Null: the test runner keeps a key that is set to Null
	// (zh.lightnovel's `sign_in::clear`), and `stored` reads "" as absent.
	store(LAST_LOGIN_ERROR_KEY, "");
}

/// Keep the reason a manual login failed, for the settings footer.
pub fn record_login_failure(reason: &str) {
	store(LAST_LOGIN_ERROR_KEY, reason);
}

/// Set when the stored session is dead and cannot be renewed: the password was rejected,
/// or no credentials were stored. The app's own login row keeps showing "logged in" in
/// this state, so the settings footer uses this flag to tell the user to log out and back
/// in. Nothing retries automatically while it is set.
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
	store(key, &format!("{value}"));
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

/// Record that the stored token was just accepted: trust it for `CHECK_INTERVAL` and
/// forget any earlier failure.
fn mark_session_verified() {
	set_timestamp(NEXT_CHECK_KEY, current_date() + CHECK_INTERVAL);
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
}

// ---------------------------------------------------------------------------
// Account facts
// ---------------------------------------------------------------------------

/// What `/api/user/info` says about the account.
#[derive(Debug, PartialEq)]
pub struct Profile {
	pub nickname: String,
	pub is_vip: bool,
	/// The date part of `vip_expires_at`, empty when the site gives none.
	pub vip_expires: String,
}

#[derive(Deserialize)]
struct InfoEnvelope {
	#[serde(default)]
	code: i32,
	#[serde(default)]
	data: Option<InfoData>,
}

#[derive(Deserialize)]
struct InfoData {
	#[serde(default)]
	nickname: Option<String>,
	#[serde(default)]
	is_vip: bool,
	/// The format of a VIP account's value has not been seen, so any JSON type is accepted.
	#[serde(default)]
	vip_expires_at: serde_json::Value,
}

/// The date part of an expiry string: everything before a `T` or a space. Any other shape
/// is shown as it is.
fn expiry_date(value: &serde_json::Value) -> String {
	let text = value.as_str().unwrap_or("").trim();
	String::from(text.split(['T', ' ']).next().unwrap_or(""))
}

/// Read a `/api/user/info` body. `None` when it is not a successful answer.
pub fn parse_profile(text: &str) -> Option<Profile> {
	let envelope = serde_json::from_str::<InfoEnvelope>(text).ok()?;
	if envelope.code != 200 {
		return None;
	}
	let data = envelope.data?;
	Some(Profile {
		nickname: data.nickname.unwrap_or_default().trim().into(),
		is_vip: data.is_vip,
		vip_expires: expiry_date(&data.vip_expires_at),
	})
}

fn store_profile(profile: &Profile) {
	store(NICKNAME_KEY, &profile.nickname);
	defaults_set(IS_VIP_KEY, DefaultValue::Bool(profile.is_vip));
	store(VIP_EXPIRES_KEY, &profile.vip_expires);
}

enum Probe {
	Profile(Profile),
	/// The site says the token is dead (HTTP 401).
	Unauthorized,
	/// The site answered, but not with the account.
	ServerError,
	/// No answer at all.
	Unreachable,
}

/// GET /api/user/info with this token.
fn probe(token: &str) -> Probe {
	let Ok(request) = Request::get(format!("{}/api/user/info", base_url())) else {
		return Probe::Unreachable;
	};
	let Ok(response) = request
		.header("Authorization", &format!("Bearer {token}"))
		.send()
	else {
		return Probe::Unreachable;
	};
	classify_probe(
		response.status_code(),
		response.get_string().ok().as_deref(),
	)
}

fn classify_probe(status: i32, body: Option<&str>) -> Probe {
	if status == 401 {
		return Probe::Unauthorized;
	}
	match body.and_then(parse_profile) {
		Some(profile) if (200..300).contains(&status) => Probe::Profile(profile),
		_ => Probe::ServerError,
	}
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

pub enum LoginOutcome {
	/// The site accepted the credentials and issued this token.
	Token(String),
	/// The site answered that the credentials are wrong.
	Rejected,
	/// No usable answer (network error, server error). Worth retrying later.
	Unreachable,
}

#[derive(Deserialize)]
struct LoginEnvelope {
	#[serde(default)]
	code: i32,
	#[serde(default)]
	data: Option<LoginData>,
}

#[derive(Deserialize)]
struct LoginData {
	#[serde(default)]
	token: String,
}

/// Read a `/api/user/login` answer. Wrong credentials are HTTP 200 with `code` 201, so the
/// code decides, and a failure to reach the site must not be mistaken for it.
pub fn parse_login(status: i32, text: &str) -> LoginOutcome {
	let Ok(envelope) = serde_json::from_str::<LoginEnvelope>(text) else {
		return LoginOutcome::Unreachable;
	};
	if !(200..300).contains(&status) {
		return LoginOutcome::Unreachable;
	}
	match envelope.code {
		200 => match envelope.data {
			Some(data) if !data.token.is_empty() => LoginOutcome::Token(data.token),
			_ => LoginOutcome::Unreachable,
		},
		201 => LoginOutcome::Rejected,
		_ => LoginOutcome::Unreachable,
	}
}

/// POST /api/user/login with these credentials.
pub fn login(username: &str, password: &str) -> LoginOutcome {
	let body = serde_json::json!({ "username": username, "password": password }).to_string();
	let Ok(request) = Request::post(format!("{}/api/user/login", base_url())) else {
		return LoginOutcome::Unreachable;
	};
	let Ok(response) = request
		.header("Content-Type", "application/json")
		.body(body.as_bytes())
		.send()
	else {
		return LoginOutcome::Unreachable;
	};
	match response.get_string() {
		Ok(text) => parse_login(response.status_code(), &text),
		Err(_) => LoginOutcome::Unreachable,
	}
}

/// Keep a fresh login: token, credentials, account facts, and a verified session. The
/// account facts are best effort; a login without them is still a login.
pub fn save_login(token: &str, username: &str, password: &str) {
	store(TOKEN_KEY, token);
	store(USERNAME_KEY, username);
	store(PASSWORD_KEY, password);
	store(LAST_LOGIN_ERROR_KEY, "");
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
	// Only an answer that carries the account makes the session verified. Any other
	// result moves the gate to a short retry instead of trusting the token for
	// `CHECK_INTERVAL`.
	if let Probe::Profile(profile) = probe(token) {
		store_profile(&profile);
		mark_session_verified();
	} else {
		set_timestamp(NEXT_CHECK_KEY, current_date() + PROBE_RETRY);
	}
	// Last, so a slow probe cannot use up the window in which the app's `login`
	// notification still counts as a login rather than a logout.
	mark_just_logged_in();
}

// ---------------------------------------------------------------------------
// Renewal
// ---------------------------------------------------------------------------

/// What the probe's answer means for the next probe, in seconds from now. `None` means the
/// token is dead and needs renewing.
fn next_probe_delay(probe: &Probe) -> Option<i64> {
	match probe {
		Probe::Profile(_) => Some(CHECK_INTERVAL),
		Probe::Unreachable => Some(PROBE_RETRY),
		Probe::ServerError => Some(FAIL_BACKOFF),
		Probe::Unauthorized => None,
	}
}

/// Renew a dead token with the stored credentials.
///
/// A rejected password wipes the stored auth and flags the account for a manual re-login:
/// retrying would only hammer the login endpoint with a password that is known to be wrong.
/// An unreachable site backs off for `FAIL_BACKOFF` instead.
fn recover_session(now: i64) {
	let Some((username, password)) = credentials() else {
		// A token without credentials cannot be renewed.
		clear_auth();
		defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
		return;
	};
	match login(&username, &password) {
		LoginOutcome::Token(token) => {
			store(TOKEN_KEY, &token);
			mark_session_verified();
			if let Probe::Profile(profile) = probe(&token) {
				store_profile(&profile);
			}
		}
		LoginOutcome::Rejected => {
			clear_auth();
			defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
		}
		LoginOutcome::Unreachable => set_timestamp(NEXT_CHECK_KEY, now + FAIL_BACKOFF),
	}
}

/// Called once at the top of each Source entry point (`get_search_manga_list`,
/// `get_manga_update`, `get_page_list`, `get_manga_list`, `get_home`). Never from request
/// helpers.
///
/// Probes the stored token at most once per `CHECK_INTERVAL`. The gate moves forward before
/// the probe is sent, so entry points running at the same time do not each probe, and every
/// branch below moves it again: no branch leaves it where it was. A dead token is renewed
/// with the stored credentials. Signed out, or flagged for a manual re-login, nothing is
/// sent.
pub fn ensure_session() {
	let Some(token) = token() else {
		return;
	};
	if needs_relogin() {
		return;
	}

	let now = current_date();
	if now < timestamp(NEXT_CHECK_KEY) {
		return;
	}
	set_timestamp(NEXT_CHECK_KEY, now + PROBE_RETRY);

	let result = probe(&token);
	if let Some(delay) = next_probe_delay(&result) {
		if let Probe::Profile(profile) = &result {
			// Moves the gate by `CHECK_INTERVAL` and clears an earlier failure.
			store_profile(profile);
			mark_session_verified();
		} else {
			set_timestamp(NEXT_CHECK_KEY, now + delay);
		}
	} else {
		recover_session(now);
	}
}

// ---------------------------------------------------------------------------
// Settings page text
// ---------------------------------------------------------------------------

/// Shown when the stored session is dead and cannot be renewed. The app's own login row
/// still says "logged in" then, so the user has to be told to log out first.
pub const RELOGIN_HINT: &str = "登入已失效，請登出再重新登入";
/// Shown while signed out. The group cannot simply be left out: a source that hands the app
/// an empty list of dynamic settings gets a settings screen whose buttons stop responding.
pub const SIGNED_OUT_HINT: &str =
	"尚未登入。登入後 VIP 會員可以閱讀「VIP 搶先看」章節，並在這裡看到帳號資訊。";

/// The account summary under the login row, from defaults only.
pub fn account_footer() -> String {
	if needs_relogin() {
		return String::from(RELOGIN_HINT);
	}
	if !logged_in() {
		return signed_out_text(&stored(LAST_LOGIN_ERROR_KEY).unwrap_or_default());
	}
	describe_account(
		&stored(NICKNAME_KEY).unwrap_or_default(),
		is_vip(),
		&stored(VIP_EXPIRES_KEY).unwrap_or_default(),
	)
}

/// The signed-out text, led by why the last login failed when there is a reason.
fn signed_out_text(last_error: &str) -> String {
	if last_error.is_empty() {
		String::from(SIGNED_OUT_HINT)
	} else {
		format!("上次登入失敗：{last_error}\n{SIGNED_OUT_HINT}")
	}
}

fn describe_account(nickname: &str, is_vip: bool, vip_expires: &str) -> String {
	let mut footer = if nickname.is_empty() {
		String::from("已登入")
	} else {
		format!("已登入：{nickname}")
	};
	footer.push_str(&match (is_vip, vip_expires.is_empty()) {
		(true, false) => format!("\n會員：VIP（到期日 {vip_expires}）"),
		(true, true) => String::from("\n會員：VIP"),
		(false, _) => String::from("\n會員：一般會員"),
	});
	footer
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn login_outcomes() {
		let ok = r#"{"code":200,"msg":"Login successful","data":{"token":"abc","userId":"1","isPatrol":false}}"#;
		assert!(matches!(parse_login(200, ok), LoginOutcome::Token(t) if t == "abc"));
		let wrong = r#"{"code":201,"msg":"x"}"#;
		assert!(matches!(parse_login(200, wrong), LoginOutcome::Rejected));
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
	fn expiry_keeps_the_date_part_of_any_string() {
		use serde_json::json;
		assert_eq!(expiry_date(&json!("2026-12-01T10:00:00")), "2026-12-01");
		assert_eq!(expiry_date(&json!("2026-12-01 10:00:00")), "2026-12-01");
		assert_eq!(expiry_date(&json!("永久")), "永久");
		assert_eq!(expiry_date(&json!("")), "");
		assert_eq!(expiry_date(&json!(null)), "");
		assert_eq!(expiry_date(&json!(1767225600)), "");
	}

	#[aidoku_test]
	fn probe_answers() {
		assert!(matches!(
			classify_probe(401, Some("{}")),
			Probe::Unauthorized
		));
		assert!(matches!(classify_probe(401, None), Probe::Unauthorized));
		assert!(matches!(
			classify_probe(500, Some("oops")),
			Probe::ServerError
		));
		assert!(matches!(
			classify_probe(200, Some(r#"{"code":500}"#)),
			Probe::ServerError
		));
		assert!(matches!(
			classify_probe(200, Some(r#"{"code":200,"data":{"nickname":"a"}}"#)),
			Probe::Profile(_)
		));
	}

	/// Every answer moves the gate, and only a dead token asks for renewal.
	#[aidoku_test]
	fn every_probe_answer_moves_the_gate() {
		let profile = Probe::Profile(Profile {
			nickname: String::new(),
			is_vip: false,
			vip_expires: String::new(),
		});
		assert_eq!(next_probe_delay(&profile), Some(1800));
		assert_eq!(next_probe_delay(&Probe::Unreachable), Some(60));
		assert_eq!(next_probe_delay(&Probe::ServerError), Some(600));
		assert_eq!(next_probe_delay(&Probe::Unauthorized), None);
	}

	#[aidoku_test]
	fn signed_out_text_leads_with_the_reason() {
		assert_eq!(signed_out_text(""), SIGNED_OUT_HINT);
		let text = signed_out_text(REJECTED_TEXT);
		assert!(text.starts_with("上次登入失敗：帳號或密碼錯誤\n"));
		assert!(text.ends_with(SIGNED_OUT_HINT));
	}

	/// The reason survives until a login or a logout clears it, and `account_footer` shows
	/// it only while signed out.
	#[aidoku_test]
	fn login_failure_reason_is_kept_then_cleared() {
		clear_auth();
		assert_eq!(account_footer(), SIGNED_OUT_HINT);
		record_login_failure(UNREACHABLE_TEXT);
		assert!(account_footer().contains(UNREACHABLE_TEXT));
		clear_auth();
		assert_eq!(account_footer(), SIGNED_OUT_HINT);
	}

	#[aidoku_test]
	fn account_text() {
		assert_eq!(
			describe_account("甲", false, ""),
			"已登入：甲\n會員：一般會員"
		);
		assert_eq!(describe_account("", false, ""), "已登入\n會員：一般會員");
		assert_eq!(
			describe_account("甲", true, "2026-12-01"),
			"已登入：甲\n會員：VIP（到期日 2026-12-01）"
		);
		assert_eq!(describe_account("甲", true, ""), "已登入：甲\n會員：VIP");
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/auth.rs"]
mod fixture_tests;
