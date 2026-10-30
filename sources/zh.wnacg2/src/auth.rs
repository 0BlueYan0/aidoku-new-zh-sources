//! Web login session.
//!
//! The site puts a slider puzzle in front of `users-check_login.html` (2026-10-23), so a
//! plain username/password POST is refused and the login has to happen in the app's
//! web view. The login cookie is host-only on `www.wnacg.com`, the same host as the
//! login page, so the cookies the app hands `handle_web_login` already include it. The
//! source stores it and attaches it to every request itself (zh.bookwalker does the
//! same, see its `auth.rs`).
//!
//! The site keeps no server-side session: `users-logout.html` only expires the cookie
//! in the browser and the old value keeps working, so signing out only has to forget it
//! here.

use aidoku::{
	alloc::{string::ToString, String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		html::Document,
		std::current_date,
	},
	prelude::*,
	HashMap,
};

use crate::helper::{base_url, request_with_cookie};

/// The `Cookie` header value captured at login.
const COOKIE_KEY: &str = "auth_cookie";
/// The display name read off the account page, shown in the settings footer.
const ACCOUNT_KEY: &str = "auth_account";
/// When the last capture succeeded. The `login` notification arrives for both signing
/// in and signing out, so a fresh timestamp is what marks it as the former.
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
/// The cookie header the last failed verification used. The login page reports every
/// cookie change, so an identical header is not sent to the site again.
const LAST_FAILED_KEY: &str = "auth_last_failed";

/// A `login` notification within this many seconds of a capture is the sign-in, not a
/// sign-out (zh.bookwalker and zh.komiic2 use the same window).
const LOGIN_WINDOW: i64 = 60;

/// The login cookie was `MPIC_bnS5` on 2026-10-30. The suffix looks like a per-site
/// random value, so only the prefix is matched; this is an assumption, not observed.
const LOGIN_COOKIE_PREFIX: &str = "MPIC_";

pub fn is_logged_in() -> bool {
	cookie_header().is_some()
}

/// Attached to every page request. `None` before the first successful capture.
pub fn cookie_header() -> Option<String> {
	defaults_get::<String>(COOKIE_KEY).filter(|header| !header.is_empty())
}

pub fn account() -> Option<String> {
	defaults_get::<String>(ACCOUNT_KEY).filter(|name| !name.is_empty())
}

/// Called from `handle_web_login` on every cookie change of the login page. Returns
/// whether the source is now logged in, which the app takes as "close the login page".
pub fn capture_web_login(cookies: &HashMap<String, String>) -> bool {
	let Some(header) = login_cookie_header(cookies) else {
		return false;
	};
	if defaults_get::<String>(LAST_FAILED_KEY).as_deref() == Some(header.as_str()) {
		return false;
	}
	match verify(&header) {
		Some(name) => {
			defaults_set(COOKIE_KEY, DefaultValue::String(header));
			defaults_set(ACCOUNT_KEY, DefaultValue::String(name));
			defaults_set(
				LOGGED_IN_AT_KEY,
				DefaultValue::String(current_date().to_string()),
			);
			defaults_set(LAST_FAILED_KEY, DefaultValue::Null);
			true
		}
		None => {
			defaults_set(LAST_FAILED_KEY, DefaultValue::String(header));
			false
		}
	}
}

/// The app posts the same `login` notification for signing in and signing out.
pub fn handle_login_notification() {
	let logged_in_at = defaults_get::<String>(LOGGED_IN_AT_KEY)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0);
	if current_date() - logged_in_at < LOGIN_WINDOW {
		defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
		return;
	}
	clear_all();
}

fn clear_all() {
	defaults_set(COOKIE_KEY, DefaultValue::Null);
	defaults_set(ACCOUNT_KEY, DefaultValue::Null);
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
	defaults_set(LAST_FAILED_KEY, DefaultValue::Null);
}

/// Sorted so the same cookies always give the same header, which the
/// `LAST_FAILED_KEY` comparison relies on.
fn login_cookie_header(cookies: &HashMap<String, String>) -> Option<String> {
	let mut pairs: Vec<String> = cookies
		.iter()
		.filter(|(name, value)| name.starts_with(LOGIN_COOKIE_PREFIX) && !value.is_empty())
		.map(|(name, value)| format!("{name}={value}"))
		.collect();
	pairs.sort();
	(!pairs.is_empty()).then(|| pairs.join("; "))
}

/// Reads the account page with `header`; `Some(display name)` when it is the signed-in
/// page. Signed out, the same URL serves the login form instead.
fn verify(header: &str) -> Option<String> {
	let url = format!("{}/users.html", base_url());
	let response = request_with_cookie(&url, Some(header)).ok()?.send().ok()?;
	if !(200..300).contains(&response.status_code()) {
		return None;
	}
	account_name(&response.get_html().ok()?)
}

/// The desktop account page links to `users-logout.html` only when signed in, and shows
/// the display name next to the avatar in `.top_utab.ui`. The `.top_utab` before it is
/// the notice menu, whose 「關注更新」 also links to `/users.html`. A blank name still
/// counts as signed in; the footer then just leaves the name out.
fn account_name(html: &Document) -> Option<String> {
	html.select_first("a[href*=users-logout]")?;
	Some(
		html.select_first(".top_utab.ui > a[href$=\"users.html\"]")
			.and_then(|el| el.text())
			.map(|text| text.trim().to_string())
			.unwrap_or_default(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn cookies(pairs: &[(&str, &str)]) -> HashMap<String, String> {
		pairs
			.iter()
			.map(|(name, value)| (String::from(*name), String::from(*value)))
			.collect()
	}

	#[aidoku_test]
	fn keeps_only_the_login_cookie() {
		let jar = cookies(&[
			("_cfuvid", "x"),
			("MPIC_bnS5", "abc%2B"),
			("PHPSESSID", "y"),
		]);
		assert_eq!(
			login_cookie_header(&jar).as_deref(),
			Some("MPIC_bnS5=abc%2B")
		);
	}

	#[aidoku_test]
	fn no_login_cookie_means_not_logged_in() {
		let jar = cookies(&[("_cfuvid", "x"), ("MPIC_bnS5", "")]);
		assert_eq!(login_cookie_header(&jar), None);
	}

	#[aidoku_test]
	fn reads_the_display_name_from_the_account_page() {
		let page = r#"<div class="top_utab ui"><a title="查看/修改我的資料" href="/users.html"><img src="/a.jpg">Reader01</a>
			<a title="退出管理" href="/users-logout.html">退出管理</a></div>"#;
		let html = Html::parse_with_url(page, "https://www.wnacg.com/users.html").expect("parse");
		assert_eq!(account_name(&html).as_deref(), Some("Reader01"));
	}

	#[aidoku_test]
	fn the_logout_link_is_never_taken_for_the_name() {
		let page = r#"<div class="top_utab ui"><a href="/users-logout.html">退出管理</a>
			<a href="/users.html">Reader01</a></div>"#;
		let html = Html::parse_with_url(page, "https://www.wnacg.com/users.html").expect("parse");
		assert_eq!(account_name(&html).as_deref(), Some("Reader01"));
	}

	#[aidoku_test]
	fn the_login_form_is_not_a_signed_in_page() {
		let page = r#"<form id="login_form" action="/users-check_login.html"><input name="login_name"></form>"#;
		let html = Html::parse_with_url(page, "https://www.wnacg.com/users.html").expect("parse");
		assert_eq!(account_name(&html), None);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[path = "fixture_tests/auth.rs"]
mod fixture_tests;
