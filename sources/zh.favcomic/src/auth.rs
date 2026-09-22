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
/// Set when the site refused the login because too many devices are already signed in.
/// Kept apart from `NEEDS_RELOGIN_KEY`: there the password is wrong, here it is right.
const DEVICE_LIMIT_KEY: &str = "auth_device_limit";

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
		DEVICE_LIMIT_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
}

/// Whether the stored credentials were rejected and should not be retried on their own.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

/// Whether the last login attempt failed because the account has too many devices
/// signed in. Reported in settings, because the reader cannot tell this apart from a
/// wrong password otherwise - the app shows the same generic failure for both.
pub fn hit_device_limit() -> bool {
	defaults_get::<bool>(DEVICE_LIMIT_KEY).unwrap_or(false)
}

/// True when the login response is the site's "too many devices" answer.
///
/// `common.js` on the site branches on exactly this: `result.code === 4` makes its login
/// page reveal a verification-code field, set a hidden `force` flag to 1, and relabel the
/// button to clearing the other devices. The code is emailed, and the login form here has
/// nowhere to type one, so this cannot be resolved from inside the app.
fn is_device_limit(text: &str) -> bool {
	let Some(rest) = text.split("\"code\"").nth(1) else {
		return false;
	};
	let digits: String = rest
		.trim_start()
		.strip_prefix(':')
		.unwrap_or("")
		.trim_start()
		.chars()
		.take_while(|character| character.is_ascii_digit())
		.collect();
	digits == "4"
}

pub enum LoginOutcome {
	/// The site accepted the credentials and set a fresh token cookie.
	Success,
	/// The credentials are right, but the account already has as many devices signed in
	/// as the site allows. Nothing here can resolve it - see `is_device_limit`.
	DeviceLimit,
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
		defaults_set(DEVICE_LIMIT_KEY, DefaultValue::Null);
		return LoginOutcome::Success;
	}

	// Checked before the generic refusal: this answer is also `result: fail`, and taking
	// it for a wrong password would wipe credentials that are perfectly good.
	if is_device_limit(&text) {
		println!("[favcomic] login refused: too many devices signed in");
		return LoginOutcome::DeviceLimit;
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
		// The password is right, so the credentials are not what is wrong and must not be
		// wiped. Nothing here can clear the other devices, so this is recorded for the
		// settings screen to explain and the attempt ends.
		LoginOutcome::DeviceLimit => {
			defaults_set(DEVICE_LIMIT_KEY, DefaultValue::Bool(true));
			false
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
	// Too many devices signed in cannot be resolved from here - it needs a code the site
	// emails - so retrying is pure cost. And the cost is not one request: `fetch_html`
	// calls `ensure_session` before every fetch the source makes, and the home screen
	// alone builds eight of them in a row, so a retrying renewal puts a login attempt in
	// front of each one and the source stops keeping up. The reader clears the block on
	// the website and signs in again, which is what lifts this flag.
	if hit_device_limit() {
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
		// Not a credential problem, so the stored password stays; back off like any other
		// failure the site may recover from once the reader frees a device slot.
		LoginOutcome::DeviceLimit => {
			defaults_set(DEVICE_LIMIT_KEY, DefaultValue::Bool(true));
			set_timestamp(FAILED_AT_KEY, now);
			false
		}
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
/// Fetched on every settings draw, with no caching, exactly as `zh.komiic` and
/// `zh.creativecomic` do - a reader who just spent coins on the site expects the figure
/// here to have moved. A 60 second cache lived here briefly and was the reason it did not.
///
/// Everything here is scraped from the site's own account page, which only answers with real
/// numbers while the token cookie is valid -- so it doubles as a visible signal that the session
/// is actually working.
pub fn account_footer() -> String {
	if needs_relogin() {
		return String::from("儲存的帳號密碼已失效，請先登出再重新登入");
	}

	// Deliberately not `fetch_html`: that renews the session first, and this runs while
	// the settings screen is being drawn. komiic and creativecomic both read their account
	// figures without touching the session, and a login attempt has no business being in
	// the way of a screen redraw. Renewal happens on the content paths instead.
	let document = crate::helper::request(&format!("{}/menu", base_url())).and_then(|request| {
		request
			.html()
			.map_err(|_| aidoku::error!("could not read the account page"))
	});
	let Ok(document) = document else {
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

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// `common.js` on the site branches on `result.code === 4` to mean "too many devices
	/// signed in". It arrives as `result: fail` like a wrong password does, so telling the
	/// two apart is the whole point - mistaking it for a wrong password wipes credentials
	/// that are correct.
	#[aidoku_test]
	fn spots_the_too_many_devices_answer() {
		assert!(is_device_limit(
			r#"{"result":"fail","code":4,"msg":"登入裝置過多"}"#
		));
		assert!(is_device_limit(r#"{"code": 4,"result":"fail"}"#));
	}

	/// A wrong password must stay a wrong password, and a longer code starting with 4
	/// must not be read as 4.
	#[aidoku_test]
	fn leaves_every_other_answer_alone() {
		assert!(!is_device_limit(r#"{"result":"fail","code":1}"#));
		assert!(!is_device_limit(r#"{"result":"fail","code":40}"#));
		assert!(!is_device_limit(r#"{"result":"fail"}"#));
		assert!(!is_device_limit(r#"{"result":"success","code":0}"#));
		assert!(!is_device_limit(""));
	}
}
