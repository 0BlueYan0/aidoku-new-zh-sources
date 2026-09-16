use aidoku::{
	alloc::String,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
};

use crate::helper::{
	extract_token_from_cookie, graphql_once, is_auth_error, json_escape, json_str_value,
	ACCOUNT_QUERY,
};

const LOGIN_URL: &str = "https://komiic.com/api/login";
const ORIGIN: &str = "https://komiic.com";

const TOKEN_KEY: &str = "auth_token";
const EMAIL_KEY: &str = "auth_email";
const PASSWORD_KEY: &str = "auth_password";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
const NEXT_CHECK_KEY: &str = "auth_next_check_at";
const FAILED_AT_KEY: &str = "auth_failed_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";

/// How long a verified session is trusted before probing again.
const CHECK_INTERVAL: i64 = 1800;
/// How soon to probe again when the probe itself could not reach the server.
const PROBE_RETRY: i64 = 60;
/// How long to wait after the login endpoint was unreachable before trying again.
const FAIL_BACKOFF: i64 = 600;
/// How long after a successful login the `login` notification still means
/// "just logged in" rather than "logged out".
const JUST_LOGGED_IN_TTL: i64 = 60;

// ---------------------------------------------------------------------------
// Stored state
// ---------------------------------------------------------------------------

/// The JWT issued by Komiic, if one is stored.
pub fn token() -> Option<String> {
	defaults_get::<String>(TOKEN_KEY).filter(|value| !value.is_empty())
}

pub fn set_token(token: &str) {
	defaults_set(TOKEN_KEY, DefaultValue::String(String::from(token)));
}

/// Email and password, kept so an expired token can be renewed silently.
/// Komiic tokens only live for 24 hours, so without these the user would have
/// to log in again every day.
pub fn credentials() -> Option<(String, String)> {
	let email = defaults_get::<String>(EMAIL_KEY)?;
	let password = defaults_get::<String>(PASSWORD_KEY)?;
	if email.is_empty() || password.is_empty() {
		None
	} else {
		Some((email, password))
	}
}

pub fn set_credentials(email: &str, password: &str) {
	defaults_set(EMAIL_KEY, DefaultValue::String(String::from(email)));
	defaults_set(PASSWORD_KEY, DefaultValue::String(String::from(password)));
}

/// Wipe everything auth related. Used on logout and when the stored session
/// turns out to be unrecoverable.
pub fn clear_auth() {
	defaults_set(TOKEN_KEY, DefaultValue::Null);
	defaults_set(EMAIL_KEY, DefaultValue::Null);
	defaults_set(PASSWORD_KEY, DefaultValue::Null);
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
	defaults_set(NEXT_CHECK_KEY, DefaultValue::Null);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
}

/// Set when the stored session is dead and cannot be renewed: the password was
/// rejected, or no credentials were stored. The app's own login row keeps
/// showing "logged in" in this state, so the settings footer uses this flag to
/// tell the user to log out and back in.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

fn set_needs_relogin() {
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
}

// The app fires the same `login` notification for both logging in and logging
// out. A login timestamp lets the handler tell the two apart. It expires, so a
// notification that never arrived cannot make a later logout look like a login
// and leave the credentials behind.
pub fn mark_just_logged_in() {
	set_timestamp(LOGGED_IN_AT_KEY, current_date());
}

pub fn is_just_logged_in() -> bool {
	current_date() - timestamp(LOGGED_IN_AT_KEY) < JUST_LOGGED_IN_TTL
}

pub fn clear_just_logged_in() {
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
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

/// Record that the stored token was just accepted: trust it for
/// `CHECK_INTERVAL` and forget any earlier failure.
pub fn mark_session_verified() {
	set_timestamp(NEXT_CHECK_KEY, current_date() + CHECK_INTERVAL);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

pub enum LoginOutcome {
	/// Komiic accepted the credentials and issued this JWT.
	Token(String),
	/// Komiic answered but issued no token: the credentials are wrong.
	Rejected,
	/// No usable answer (network error, server error). Worth retrying later.
	Unreachable,
}

/// POST /api/login with these credentials. The token comes back in the
/// response body; the `set-cookie` header carries the same value and is only
/// used as a fallback.
///
/// Wrong credentials do not produce an error status: Komiic replies HTTP 200
/// with an empty body, a `www-authenticate` header and a `Set-Cookie` that
/// clears the token cookie. That case must be told apart from a network
/// failure, or a changed password would be retried forever.
pub fn login(email: &str, password: &str) -> LoginOutcome {
	let body = format!(
		r#"{{"email":"{}","password":"{}"}}"#,
		json_escape(email),
		json_escape(password)
	);

	let Ok(request) = Request::post(LOGIN_URL) else {
		return LoginOutcome::Unreachable;
	};
	let Ok(response) = request
		.header("Content-Type", "application/json")
		.header("Accept", "application/json")
		.header("Origin", ORIGIN)
		.header("Referer", ORIGIN)
		.body(body.as_bytes())
		.send()
	else {
		println!("[komiic] login request failed to send");
		return LoginOutcome::Unreachable;
	};

	let status = response.status_code();

	let cookie_token = response
		.get_header("set-cookie")
		.and_then(|header| extract_token_from_cookie(&header).map(String::from))
		.filter(|token| !token.is_empty());
	let body_token = response
		.get_string()
		.ok()
		.and_then(|text| json_str_value(&text, "token").map(String::from))
		.filter(|token| !token.is_empty());

	if let Some(token) = body_token.or(cookie_token) {
		return LoginOutcome::Token(token);
	}

	let challenged = response.get_header("www-authenticate").is_some();
	if status == 200 || status == 401 || challenged {
		println!("[komiic] login rejected (HTTP {status})");
		LoginOutcome::Rejected
	} else {
		println!("[komiic] login failed with HTTP {status}");
		LoginOutcome::Unreachable
	}
}

/// Renew a dead token with the stored credentials.
///
/// A rejected password wipes the stored auth and flags the account for a
/// manual re-login: retrying would only hammer the login endpoint with a
/// password that is known to be wrong. An unreachable server backs off for
/// `FAIL_BACKOFF` instead.
pub fn refresh_token() -> Option<String> {
	let now = current_date();
	if now - timestamp(FAILED_AT_KEY) < FAIL_BACKOFF {
		return None;
	}

	let (email, password) = credentials()?;
	match login(&email, &password) {
		LoginOutcome::Token(token) => {
			set_token(&token);
			mark_session_verified();
			println!("[komiic] auth token renewed");
			Some(token)
		}
		LoginOutcome::Rejected => {
			println!("[komiic] stored password rejected, manual re-login required");
			clear_auth();
			set_needs_relogin();
			None
		}
		LoginOutcome::Unreachable => {
			set_timestamp(FAILED_AT_KEY, now);
			println!("[komiic] silent re-login failed, will retry later");
			None
		}
	}
}

/// React to a response that says the stored token is dead: renew it when the
/// credentials are stored, otherwise drop it and ask the user to log in again.
/// Returns the new token if one was obtained.
pub fn recover_session() -> Option<String> {
	if credentials().is_some() {
		return refresh_token();
	}
	// A token without credentials was stored by a version that did not keep
	// them; nothing can renew it.
	println!("[komiic] session expired and no credentials stored, manual re-login required");
	clear_auth();
	set_needs_relogin();
	None
}

/// Make sure the stored token is still accepted, renewing it if it is not.
///
/// Komiic does not reject a dead token: every content query is simply served
/// anonymously with HTTP 200, so waiting for a request to fail would never
/// detect the expiry. `account` is the one query that reports the state, so it
/// is probed here on an interval instead.
pub fn ensure_session() {
	let Some(token) = token() else {
		return;
	};

	let now = current_date();
	if now < timestamp(NEXT_CHECK_KEY) {
		return;
	}

	let Ok((status, body)) = graphql_once(ACCOUNT_QUERY, "{}", Some(&token)) else {
		// The probe itself failed (offline, server down). Retry soon, but not
		// on every single request while it stays that way.
		set_timestamp(NEXT_CHECK_KEY, now + PROBE_RETRY);
		return;
	};

	if is_auth_error(status, &body) {
		println!("[komiic] session expired, recovering");
		if recover_session().is_none() && self::token().is_some() {
			// Renewal is backing off; do not probe on every request meanwhile.
			set_timestamp(NEXT_CHECK_KEY, now + FAIL_BACKOFF);
		}
	} else if status == 200 {
		mark_session_verified();
	} else {
		// Server error: says nothing about the token, so check again soon.
		set_timestamp(NEXT_CHECK_KEY, now + PROBE_RETRY);
	}
}
