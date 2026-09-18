//! Login state for favcomic.
//!
//! The site authenticates with a `token` cookie that it sets itself on a successful
//! `POST /login` and clears on `GET /logout`. Aidoku's shared cookie jar stores and replays that
//! cookie for us, so this module never sets a `Cookie` header of its own -- doing so would lose
//! to the jar's copy, which `AidokuRunner.modify()` puts first.
//!
//! What we do keep is the credentials, so the seven day token can be renewed without the reader
//! noticing it lapsed.

use aidoku::{
	alloc::{format, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
};

use crate::helper::{base_url, USER_AGENT};

const EMAIL_KEY: &str = "auth_email";
const PASSWORD_KEY: &str = "auth_password";
const EXPIRES_AT_KEY: &str = "auth_expires_at";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
const FAILED_AT_KEY: &str = "auth_failed_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";

/// The site issues its token cookie with `Max-Age=604800`.
const TOKEN_LIFETIME: i64 = 604_800;
/// Renew a day early, so a lapsed token never reaches the reader.
const RENEW_MARGIN: i64 = 86_400;
/// How long to wait before retrying a login that could not reach the server.
const RETRY_BACKOFF: i64 = 600;
/// A `login` notification this soon after a successful login is that login, not a logout.
const LOGIN_WINDOW: i64 = 60;

fn timestamp(key: &str) -> i64 {
	defaults_get::<String>(key)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0)
}

fn set_timestamp(key: &str, value: i64) {
	defaults_set(key, DefaultValue::String(format!("{value}")));
}

pub fn credentials() -> Option<(String, String)> {
	let email = defaults_get::<String>(EMAIL_KEY).filter(|v| !v.is_empty())?;
	let password = defaults_get::<String>(PASSWORD_KEY).filter(|v| !v.is_empty())?;
	Some((email, password))
}

fn store_credentials(email: &str, password: &str) {
	defaults_set(EMAIL_KEY, DefaultValue::String(String::from(email)));
	defaults_set(PASSWORD_KEY, DefaultValue::String(String::from(password)));
}

/// Forget everything we know about the session, credentials included.
pub fn clear_auth() {
	for key in [
		EMAIL_KEY,
		PASSWORD_KEY,
		EXPIRES_AT_KEY,
		LOGGED_IN_AT_KEY,
		FAILED_AT_KEY,
		NEEDS_RELOGIN_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
}

/// Whether the stored credentials were rejected and should not be retried on their own.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

pub enum LoginOutcome {
	/// The site accepted the credentials and set a fresh token cookie.
	Success,
	/// The site answered, but refused these credentials. Retrying changes nothing.
	Rejected,
	/// No usable answer. Worth retrying after a backoff.
	Unreachable,
}

/// Posts credentials to `/login`.
///
/// A wrong password is not an error status: the site answers 200 with
/// `{"result":"fail",...}` and no `Set-Cookie`, so the body has to be inspected to tell a
/// refusal apart from a network failure.
pub fn login(email: &str, password: &str) -> LoginOutcome {
	let base = base_url();
	let body = format!(
		"loginName={}&password={}&checkCode=&force=",
		encode_uri_component(email),
		encode_uri_component(password)
	);

	let Ok(request) = Request::post(format!("{base}/login")) else {
		return LoginOutcome::Unreachable;
	};
	let Ok(response) = request
		.header("Content-Type", "application/x-www-form-urlencoded")
		.header("User-Agent", USER_AGENT)
		.header("Referer", &format!("{base}/login"))
		.body(body.as_bytes())
		.send()
	else {
		println!("[favcomic] login request could not be sent");
		return LoginOutcome::Unreachable;
	};

	let status = response.status_code();
	let Ok(text) = response.get_string() else {
		return LoginOutcome::Unreachable;
	};

	if text.contains("\"result\":\"success\"") {
		let now = current_date();
		set_timestamp(EXPIRES_AT_KEY, now + TOKEN_LIFETIME);
		defaults_set(FAILED_AT_KEY, DefaultValue::Null);
		defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
		return LoginOutcome::Success;
	}

	if text.contains("\"result\":\"fail\"") {
		println!("[favcomic] login refused by the site");
		return LoginOutcome::Rejected;
	}

	println!("[favcomic] unexpected login response (status {status})");
	LoginOutcome::Unreachable
}

/// Records a login the reader performed through the settings screen.
pub fn handle_login(email: &str, password: &str) -> bool {
	match login(email, password) {
		LoginOutcome::Success => {
			store_credentials(email, password);
			// Only a login the reader performed may claim the `login` notification that follows.
			// A silent renewal must not, or a logout right after one would be mistaken for a login
			// and leave the credentials behind.
			set_timestamp(LOGGED_IN_AT_KEY, current_date());
			true
		}
		LoginOutcome::Rejected => {
			clear_auth();
			false
		}
		LoginOutcome::Unreachable => false,
	}
}

fn renew_with_stored_credentials() -> bool {
	if needs_relogin() {
		return false;
	}
	let Some((email, password)) = credentials() else {
		return false;
	};
	let now = current_date();
	if now < timestamp(FAILED_AT_KEY) + RETRY_BACKOFF {
		return false;
	}

	match login(&email, &password) {
		LoginOutcome::Success => true,
		LoginOutcome::Rejected => {
			// The password changed or the account is gone: stop retrying and tell the reader.
			defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
			false
		}
		LoginOutcome::Unreachable => {
			set_timestamp(FAILED_AT_KEY, now);
			false
		}
	}
}

/// Renews the token before it lapses. Cheap when there is nothing to do.
pub fn ensure_session() {
	if credentials().is_none() {
		return;
	}
	let expires_at = timestamp(EXPIRES_AT_KEY);
	if expires_at == 0 || current_date() < expires_at - RENEW_MARGIN {
		return;
	}
	renew_with_stored_credentials();
}

/// Called when a chapter reports that it needs a login although we believe we have one.
///
/// Returns whether the caller should fetch the chapter again.
pub fn retry_after_lock() -> bool {
	renew_with_stored_credentials()
}

/// Ends the session on the site so it clears the token cookie from the shared jar, then forgets
/// the credentials.
pub fn logout() {
	if let Ok(request) = Request::get(format!("{}/logout", base_url())) {
		let _ = request.header("User-Agent", USER_AGENT).send();
	}
	clear_auth();
}

/// Builds the account summary shown under the login setting.
///
/// Everything here is scraped from the site's own account page, which only answers with real
/// numbers while the token cookie is valid -- so it doubles as a visible signal that the session
/// is actually working.
pub fn account_footer() -> String {
	if needs_relogin() {
		return String::from("儲存的帳號密碼已失效，請先登出再重新登入");
	}

	let Ok(document) = crate::helper::fetch_html(&format!("{}/menu", base_url())) else {
		return String::from("無法連線到喜漫漫畫，請檢查網路");
	};

	let text_of = |selector: &str| {
		document
			.select_first(selector)
			.and_then(|el| el.text())
			.map(|text| String::from(text.trim()))
			.filter(|text| !text.is_empty())
	};

	let Some(nickname) = text_of(".brief .nick_name") else {
		return String::from("登入已失效，稍後會自動重新登入；若持續出現請登出後重新登入");
	};

	let mut footer = match text_of(".brief .role") {
		Some(role) => format!("已登入：{nickname}（{role}）"),
		None => format!("已登入：{nickname}"),
	};

	// The coin balance is the first link in the recharge box; the points box nested inside it
	// carries the coupon and point counts.
	if let Some(coins) = text_of(".recharge_box a") {
		footer = format!("{footer}\n金幣 {coins}");
	}

	let extras = document
		.select(".points_box a")
		.map(|links| {
			links
				.into_iter()
				.filter_map(|link| {
					let label = link.own_text().map(|text| String::from(text.trim()))?;
					let value = link.select_first("span").and_then(|el| el.text())?;
					if label.is_empty() {
						None
					} else {
						Some(format!("{label} {}", value.trim()))
					}
				})
				.collect::<Vec<String>>()
		})
		.unwrap_or_default();
	for extra in extras {
		footer = format!("{footer} ・ {extra}");
	}

	if let Some(progress) = text_of(".progress_txt_box span") {
		footer = format!("{footer}\n會員進度 {progress}");
	}

	footer
}

/// Aidoku sends the same `login` notification for logging in and logging out, so tell them apart
/// by how recently a login succeeded.
pub fn handle_login_notification() {
	let logged_in_at = timestamp(LOGGED_IN_AT_KEY);
	if logged_in_at != 0 && current_date() - logged_in_at <= LOGIN_WINDOW {
		defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
		return;
	}
	logout();
}
