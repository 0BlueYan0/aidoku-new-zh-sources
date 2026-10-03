//! The login token. vomic's tokens are issued until 2043 and stay valid when the same
//! account logs in again elsewhere, so nothing is renewed and the password is not kept
//! (zh.komiic and zh.favcomic keep it to renew short-lived tokens).

use aidoku::{
	alloc::String,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
	Result,
};

use crate::helper::{json_field, json_string};

const API_URL: &str = "https://api.vomicmh.com/pics";

const TOKEN_KEY: &str = "auth_token";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";

/// How long after a successful login the `login` notification still means
/// "just logged in" rather than "logged out".
const JUST_LOGGED_IN_TTL: i64 = 60;

pub fn token() -> Option<String> {
	defaults_get::<String>(TOKEN_KEY).filter(|value: &String| !value.is_empty())
}

/// Wipe the session. Used on logout.
pub fn clear_auth() {
	defaults_set(TOKEN_KEY, DefaultValue::Null);
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
}

// The app fires the same `login` notification for both logging in and logging out. A
// login timestamp tells the two apart; it expires, so a notification that never arrived
// cannot make a later logout look like a login.
pub fn is_just_logged_in() -> bool {
	let at = defaults_get::<String>(LOGGED_IN_AT_KEY)
		.and_then(|value: String| value.parse::<i64>().ok())
		.unwrap_or(0);
	current_date() - at < JUST_LOGGED_IN_TTL
}

pub fn clear_just_logged_in() {
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
}

fn json_escape(value: &str) -> String {
	let mut out = String::with_capacity(value.len());
	for ch in value.chars() {
		match ch {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
			c => out.push(c),
		}
	}
	out
}

/// The token in a login answer. The site answers HTTP 200 either way; a refusal is
/// `{"code":400,"message":"用户名或密码错误"}`.
pub fn token_in(body: &str) -> Option<String> {
	(json_field(body, "code") == Some("200"))
		.then(|| json_string(body, "token"))
		.flatten()
}

/// Log in with an email or phone number. `Ok(false)` when the site refuses the
/// credentials; `Err` when it could not be asked.
pub fn login(account: &str, password: &str) -> Result<bool> {
	let body = format!(
		r#"{{"email":"{}","password":"{}"}}"#,
		json_escape(account.trim()),
		json_escape(password)
	);
	let answer = Request::post(format!("{API_URL}/login"))?
		.header("Content-Type", "application/json")
		.header("Accept", "application/json")
		.body(body)
		.string()?;
	match token_in(&answer) {
		Some(token) => {
			defaults_set(TOKEN_KEY, DefaultValue::String(token));
			defaults_set(LOGGED_IN_AT_KEY, DefaultValue::String(format!("{}", current_date())));
			println!("[vomicmh] login successful");
			Ok(true)
		}
		None => {
			println!("[vomicmh] login refused: {}", answer.chars().take(120).collect::<String>());
			Ok(false)
		}
	}
}

pub enum Account {
	/// The account's display name.
	Name(String),
	/// The site rejected the token.
	Rejected,
	/// No usable answer.
	Unreachable,
}

/// The logged-in account. Only the settings page asks; a 401 here is how a token the
/// site stopped accepting shows (the reader page just comes back empty).
pub fn account(token: &str) -> Account {
	let Ok(response) = Request::get(format!("{API_URL}/me"))
		.map(|r: Request| r.header("Authorization", &format!("Bearer {token}")).header("Accept", "application/json"))
		.and_then(|r: Request| r.send())
	else {
		return Account::Unreachable;
	};
	if response.status_code() == 401 {
		return Account::Rejected;
	}
	let Ok(body) = response.get_string() else {
		return Account::Unreachable;
	};
	match json_field(&body, "data").and_then(|data: &str| json_string(data, "name")) {
		Some(name) => Account::Name(name),
		None => Account::Unreachable,
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn reads_login_answers() {
		assert_eq!(
			token_in(r#"{"code":200,"data":{"id":1,"name":"a","token":"nested"},"token":"eyJ.x.y","ttl":8640000}"#).as_deref(),
			Some("eyJ.x.y")
		);
		assert_eq!(token_in(r#"{"code":400,"message":"用户名或密码错误"}"#), None);
		assert_eq!(token_in(""), None);
		assert_eq!(json_escape("a\"b\\c"), "a\\\"b\\\\c");
	}
}
