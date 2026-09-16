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
const JUST_LOGGED_IN_KEY: &str = "justLoggedIn";
const CHECKED_AT_KEY: &str = "auth_checked_at";
const FAILED_AT_KEY: &str = "auth_failed_at";

/// How long a verified session is trusted before probing again.
const CHECK_INTERVAL: i64 = 1800;
/// How long to wait after a failed silent login before trying again.
const FAIL_BACKOFF: i64 = 600;

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

pub fn clear_auth() {
	defaults_set(TOKEN_KEY, DefaultValue::Null);
	defaults_set(EMAIL_KEY, DefaultValue::Null);
	defaults_set(PASSWORD_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	defaults_set(CHECKED_AT_KEY, DefaultValue::Null);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
}

// The app fires the same `login` notification for both logging in and logging
// out, so a flag is used to swallow the one that follows a successful login.
pub fn set_just_logged_in() {
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
}

pub fn is_just_logged_in() -> bool {
	defaults_get::<bool>(JUST_LOGGED_IN_KEY).unwrap_or(false)
}

pub fn clear_just_logged_in() {
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
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

pub fn mark_session_verified() {
	set_timestamp(CHECKED_AT_KEY, current_date());
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

/// POST /api/login and return the JWT Komiic issues for these credentials.
/// The token comes back in the response body; the `set-cookie` header carries
/// the same value and is only used as a fallback.
pub fn login(email: &str, password: &str) -> Option<String> {
	let body = format!(
		r#"{{"email":"{}","password":"{}"}}"#,
		json_escape(email),
		json_escape(password)
	);

	let response = Request::post(LOGIN_URL)
		.ok()?
		.header("Content-Type", "application/json")
		.header("Accept", "application/json")
		.header("Origin", ORIGIN)
		.header("Referer", ORIGIN)
		.body(body.as_bytes())
		.send()
		.ok()?;

	let status = response.status_code();
	if status != 200 {
		println!("[komiic] login failed with status {status}");
		return None;
	}

	// Read the header before the body so both are available regardless of which
	// one carries the token.
	let cookie_token = response
		.get_header("set-cookie")
		.and_then(|header| extract_token_from_cookie(&header).map(String::from));

	if let Ok(text) = response.get_string() {
		if let Some(token) = json_str_value(&text, "token") {
			if !token.is_empty() {
				return Some(String::from(token));
			}
		}
	}

	cookie_token.filter(|token| !token.is_empty())
}

/// Renew an expired token using the stored credentials. Backs off after a
/// failure so a changed password cannot trigger a login on every request.
pub fn refresh_token() -> Option<String> {
	let now = current_date();
	if now - timestamp(FAILED_AT_KEY) < FAIL_BACKOFF {
		return None;
	}

	let (email, password) = credentials()?;
	match login(&email, &password) {
		Some(token) => {
			set_token(&token);
			mark_session_verified();
			println!("[komiic] auth token renewed");
			Some(token)
		}
		None => {
			set_timestamp(FAILED_AT_KEY, now);
			println!("[komiic] silent re-login failed");
			None
		}
	}
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
	if credentials().is_none() {
		return;
	}

	let now = current_date();
	if now - timestamp(CHECKED_AT_KEY) < CHECK_INTERVAL {
		return;
	}

	let Ok((status, body)) = graphql_once(ACCOUNT_QUERY, "{}", Some(&token)) else {
		// Network failure: leave the timestamps alone and probe again next time.
		return;
	};

	if is_auth_error(status, &body) {
		println!("[komiic] session expired, re-logging in");
		refresh_token();
	}

	// Record the attempt either way, so a session that stays broken cannot probe
	// on every single request.
	set_timestamp(CHECKED_AT_KEY, now);
}
