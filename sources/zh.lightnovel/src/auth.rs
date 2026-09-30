//! Login state, kept in defaults. Same shape as zh.komiic's `auth.rs`.
//!
//! `POST /api/user/login` takes the SHA-256 hex of the password and returns an access
//! token and a refresh token. The access token is a JWT that lives 30 seconds;
//! `POST /api/user/refresh_token` exchanges the refresh token for a new one and does
//! not rotate the refresh token, so concurrent entry points renewing at once cannot
//! invalidate each other. Wrong credentials still answer HTTP 200, with
//! `Success: false` (`Status` 401 wrong password, 404 no such user).

use aidoku::{
	alloc::{format, string::ToString, String},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
};
use crate::hub::api_base;
use serde_json::Value;
use sha2::{Digest, Sha256};


const EMAIL_KEY: &str = "auth_email";
const PASSWORD_HASH_KEY: &str = "auth_password_hash";
const REFRESH_KEY: &str = "auth_refresh_token";
const ACCESS_KEY: &str = "auth_access_token";
const ACCESS_UNTIL_KEY: &str = "auth_access_until";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
const FAILED_AT_KEY: &str = "auth_failed_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";

/// How long a fresh access token is reused. The site issues 30 seconds; the margin
/// covers the negotiate that follows. The JWT's own `exp` is not read: a wrong clock
/// or a misparsed claim would make every entry point renew (CLAUDE.md, zh.creativecomic).
const ACCESS_TTL: i64 = 20;
/// How long to wait after the server was unreachable before trying again.
const FAIL_BACKOFF: i64 = 60;
/// How long after a successful login the `login` notification still means
/// "just logged in" rather than "logged out".
const JUST_LOGGED_IN_TTL: i64 = 60;

// ---------------------------------------------------------------------------
// Stored state
// ---------------------------------------------------------------------------

fn get_string(key: &str) -> Option<String> {
	defaults_get::<String>(key).filter(|value: &String| !value.is_empty())
}

fn set_string(key: &str, value: &str) {
	defaults_set(key, DefaultValue::String(String::from(value)));
}

/// Unix seconds stored as a string, so the value cannot overflow an i32.
fn timestamp(key: &str) -> i64 {
	get_string(key).and_then(|value: String| value.parse::<i64>().ok()).unwrap_or(0)
}

fn set_timestamp(key: &str, value: i64) {
	set_string(key, &format!("{value}"));
}

pub fn is_logged_in() -> bool {
	get_string(REFRESH_KEY).is_some()
}

/// Set when the stored login is dead and cannot be renewed (the password changed).
/// The app's login row keeps saying "logged in", so the settings footer uses this to
/// tell the user to log out and back in. Cleared by the next successful login.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

fn set_needs_relogin() {
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
}

pub fn clear_auth() {
	for key in [
		EMAIL_KEY,
		PASSWORD_HASH_KEY,
		REFRESH_KEY,
		ACCESS_KEY,
		ACCESS_UNTIL_KEY,
		LOGGED_IN_AT_KEY,
		FAILED_AT_KEY,
		NEEDS_RELOGIN_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
}

// The app fires the same `login` notification for logging in and logging out. A login
// timestamp tells the two apart, and expires so a lost notification cannot make a
// later logout look like a login.
pub fn is_just_logged_in() -> bool {
	current_date() - timestamp(LOGGED_IN_AT_KEY) < JUST_LOGGED_IN_TTL
}

pub fn clear_just_logged_in() {
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
}

fn store_access(access: &str) {
	set_string(ACCESS_KEY, access);
	set_timestamp(ACCESS_UNTIL_KEY, current_date() + ACCESS_TTL);
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

pub fn password_hash(password: &str) -> String {
	let digest = Sha256::digest(password.as_bytes());
	let mut hex = String::with_capacity(64);
	for byte in digest {
		hex.push_str(&format!("{byte:02x}"));
	}
	hex
}

enum ApiReply {
	/// `Success: true`, with the whole envelope: Novella accepts the payload both in
	/// `Response` and at the top level.
	Ok(Value),
	/// Refused, with the status: HTTP 401/404, or the envelope's `Status`.
	Refused(i64),
	/// No usable answer (offline, server error).
	Unreachable,
}

fn post_json(url: &str, body: &Value) -> ApiReply {
	let Ok(request) = Request::post(url) else {
		return ApiReply::Unreachable;
	};
	let Ok(response) = request
		.header("Content-Type", "application/json")
		.timeout(30.0)
		.body(body.to_string().as_bytes())
		.send()
	else {
		println!("[lightnovel] {url} could not be sent");
		return ApiReply::Unreachable;
	};
	let status = response.status_code();
	let value = response
		.get_string()
		.ok()
		.and_then(|text: String| serde_json::from_str::<Value>(&text).ok());
	let Some(value) = value else {
		println!("[lightnovel] {url} answered HTTP {status} without JSON");
		return if status == 401 || status == 404 { ApiReply::Refused(i64::from(status)) } else { ApiReply::Unreachable };
	};
	if value.get("Success").and_then(Value::as_bool) == Some(true) {
		return ApiReply::Ok(value);
	}
	let site_status = value.get("Status").and_then(Value::as_i64).unwrap_or(i64::from(status));
	println!("[lightnovel] {url} refused: HTTP {status}, Status {site_status}, {:?}", value.get("Msg"));
	if status >= 500 || site_status >= 500 {
		ApiReply::Unreachable
	} else {
		ApiReply::Refused(site_status)
	}
}

/// `name` from the envelope's `Response`, or from the envelope itself.
fn payload_str(envelope: &Value, name: &str) -> Option<String> {
	envelope
		.get("Response")
		.and_then(|r: &Value| r.get(name))
		.or_else(|| envelope.get(name))
		.and_then(Value::as_str)
		.filter(|s: &&str| !s.is_empty())
		.map(String::from)
}

pub enum LoginOutcome {
	LoggedIn,
	/// The site answered and refused the credentials.
	Rejected,
	Unreachable,
}

/// Log in and store the tokens and credentials on success.
pub fn login(email: &str, password_hash: &str) -> LoginOutcome {
	let body = serde_json::json!({ "email": email, "password": password_hash });
	match post_json(&format!("{}/api/user/login", api_base()), &body) {
		ApiReply::Ok(envelope) => {
			let (Some(access), Some(refresh)) = (payload_str(&envelope, "Token"), payload_str(&envelope, "RefreshToken")) else {
				println!("[lightnovel] login answered without tokens");
				return LoginOutcome::Unreachable;
			};
			set_string(EMAIL_KEY, email);
			set_string(PASSWORD_HASH_KEY, password_hash);
			set_string(REFRESH_KEY, &refresh);
			store_access(&access);
			defaults_set(FAILED_AT_KEY, DefaultValue::Null);
			defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
			LoginOutcome::LoggedIn
		}
		ApiReply::Refused(_) => LoginOutcome::Rejected,
		ApiReply::Unreachable => LoginOutcome::Unreachable,
	}
}

/// Called by the login handler after `login` succeeded.
pub fn mark_just_logged_in() {
	set_timestamp(LOGGED_IN_AT_KEY, current_date());
}

/// A usable access token, renewing it when the stored one is older than `ACCESS_TTL`.
/// `None` when signed out, when the login is dead, or while the server is unreachable.
///
/// Called once at the top of each entry point, never inside a request helper
/// (CLAUDE.md, 請求排程). It sends at most one refresh, plus one login when the refresh
/// token was rejected. Every branch moves a gate: a fresh token, the failure backoff,
/// or the needs-relogin brake that only a manual login clears.
pub fn access_token() -> Option<String> {
	if needs_relogin() {
		return None;
	}
	let refresh = get_string(REFRESH_KEY)?;
	let now = current_date();
	if now < timestamp(ACCESS_UNTIL_KEY) {
		if let Some(access) = get_string(ACCESS_KEY) {
			return Some(access);
		}
	}
	if now - timestamp(FAILED_AT_KEY) < FAIL_BACKOFF {
		return None;
	}

	match post_json(&format!("{}/api/user/refresh_token", api_base()), &serde_json::json!({ "token": refresh })) {
		ApiReply::Ok(envelope) => {
			// A bare string in `Response`, or `Token` there or at the top level.
			let access = envelope
				.get("Response")
				.and_then(Value::as_str)
				.filter(|s: &&str| !s.is_empty())
				.map(String::from)
				.or_else(|| payload_str(&envelope, "Token"));
			if let Some(access) = access {
				store_access(&access);
				return Some(access);
			}
			println!("[lightnovel] refresh answered without a token");
			set_timestamp(FAILED_AT_KEY, now);
			return None;
		}
		// The refresh token is dead only on these (Novella's client-core, and the site's
		// own `ki` table); anything else keeps it for the next try.
		ApiReply::Refused(401 | 404 | -100) => {
			println!("[lightnovel] refresh token rejected, logging in again");
		}
		ApiReply::Refused(_) | ApiReply::Unreachable => {
			set_timestamp(FAILED_AT_KEY, now);
			return None;
		}
	}

	let (Some(email), Some(hash)) = (get_string(EMAIL_KEY), get_string(PASSWORD_HASH_KEY)) else {
		println!("[lightnovel] no stored password, manual re-login required");
		set_needs_relogin();
		return None;
	};
	match login(&email, &hash) {
		LoginOutcome::LoggedIn => {
			println!("[lightnovel] logged in again with the stored password");
			get_string(ACCESS_KEY)
		}
		LoginOutcome::Rejected => {
			println!("[lightnovel] stored password rejected, manual re-login required");
			let email = email.to_string();
			clear_auth();
			// Keep the email so the footer can still say which account it was.
			set_string(EMAIL_KEY, &email);
			set_needs_relogin();
			None
		}
		LoginOutcome::Unreachable => {
			println!("[lightnovel] login again failed, will retry later");
			set_timestamp(FAILED_AT_KEY, now);
			None
		}
	}
}

/// A new access token even when the stored one is still fresh, for a call the hub
/// refused. Same gates as `access_token`.
pub fn renew() -> Option<String> {
	defaults_set(ACCESS_UNTIL_KEY, DefaultValue::Null);
	access_token()
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn hashes_password_like_the_site() {
		// crypto.subtle.digest("SHA-256") as hex, which is what the site's login page sends.
		assert_eq!(
			password_hash("password"),
			"5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8"
		);
	}
}
