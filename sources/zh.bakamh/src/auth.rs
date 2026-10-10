//! Login state for bakamh.
//!
//! The site is WordPress: a successful `wp_manga_signin` sets a `wordpress_logged_in_*`
//! cookie (14 days with `rememberme`), and the app's shared cookie jar stores and replays it.
//! So, like zh.favcomic, this module never sets a `Cookie` header of its own.
//!
//! What we keep is the credentials, so a lapsed cookie can be replaced without the reader
//! noticing. There is no way to tell when the cookie lapses short of a chapter saying it
//! needs a login, so renewal happens there and nowhere else.

use aidoku::{
	alloc::{format, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		html::Html,
		net::Request,
		std::current_date,
	},
	prelude::*,
};

use crate::helper::{
	base_url, clearance_cookie, fetch_html, fetch_text, is_challenge, is_signed_in, is_site_page,
	login_nonce, logout_url, site_get, user_name,
};

const USERNAME_KEY: &str = "auth_username";
const PASSWORD_KEY: &str = "auth_password";
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
/// When the source last tried to sign in on its own, whatever the outcome.
const RENEWED_AT_KEY: &str = "auth_renewed_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";
/// Why the last login typed in settings failed: `rejected` or `unreachable`. The login
/// dialog only ever shows the app's generic message, and a rejected login clears the
/// credentials, so without this the account footer could only say "not signed in".
const LAST_FAILURE_KEY: &str = "auth_last_failure";
/// Every domain a login succeeded on, space separated. Each mirror keeps its own sign-in
/// cookie, so logging out has to visit each of them; visiting only these avoids a Cloudflare
/// challenge on mirrors the reader never used.
const DOMAINS_KEY: &str = "auth_domains";

/// Renewals closer together than this are skipped, successful ones included.
const RENEW_GAP: i64 = 600;
/// A `login` notification this soon after a successful login is that login, not a logout.
const LOGIN_WINDOW: i64 = 60;
/// Logging out runs inside a notification handler, where a request that never finishes
/// takes the app down with it.
const LOGOUT_TIMEOUT: f64 = 10.0;

fn timestamp(key: &str) -> i64 {
	defaults_get::<String>(key)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0)
}

fn set_timestamp(key: &str, value: i64) {
	defaults_set(key, DefaultValue::String(format!("{value}")));
}

fn signed_in_domains() -> Vec<String> {
	defaults_get::<String>(DOMAINS_KEY)
		.unwrap_or_default()
		.split_whitespace()
		.map(String::from)
		.collect()
}

fn remember_domain(domain: &str) {
	let mut domains = signed_in_domains();
	if !domains.iter().any(|d| d == domain) {
		domains.push(String::from(domain));
		defaults_set(DOMAINS_KEY, DefaultValue::String(domains.join(" ")));
	}
}

/// The settings login item's own key. The app marks the item logged in by giving this key a
/// value (`SettingView.swift`: `loggedIn` is "the stored value is not empty"), and the source's
/// defaults share that namespace.
const LOGIN_ITEM_KEY: &str = "login";
/// When `note_site_session` last cleared `login`. The settings screen sends the `login`
/// notification for any change to that key while it is open, the source's own included
/// (`SettingView.swift`: `UserDefaultsObserver` → `handleValueChange`).
const MARK_CLEARED_AT_KEY: &str = "auth_mark_cleared_at";
/// A `login` notification this soon after the source cleared the mark is that clear.
const MARK_WINDOW: i64 = 5;

/// Called with every site page the source reads. A sign-in cookie the source has no record of
/// (it outlives removing the source, see CLAUDE.md) leaves the reader signed in on the site while
/// the settings screen said "not signed in". Marking the login item lets its button read "log
/// out", and logging out then ends that session. Defaults only, no request.
pub fn note_site_session(signed_in: bool) {
	if credentials().is_some() {
		return;
	}
	let marked = defaults_get::<String>(LOGIN_ITEM_KEY).is_some_and(|v| !v.is_empty());
	if signed_in && !marked {
		defaults_set(LOGIN_ITEM_KEY, DefaultValue::String(String::from("logged_in")));
	} else if !signed_in && marked {
		set_timestamp(MARK_CLEARED_AT_KEY, current_date());
		defaults_set(LOGIN_ITEM_KEY, DefaultValue::Null);
	}
}

/// Whether the login item shows as logged in without stored credentials: the site still has
/// a session the source did not make.
pub fn has_foreign_session() -> bool {
	credentials().is_none() && defaults_get::<String>(LOGIN_ITEM_KEY).is_some_and(|v| !v.is_empty())
}

pub fn credentials() -> Option<(String, String)> {
	let username = defaults_get::<String>(USERNAME_KEY).filter(|v| !v.is_empty())?;
	let password = defaults_get::<String>(PASSWORD_KEY).filter(|v| !v.is_empty())?;
	Some((username, password))
}

fn store_credentials(username: &str, password: &str) {
	defaults_set(USERNAME_KEY, DefaultValue::String(String::from(username)));
	defaults_set(PASSWORD_KEY, DefaultValue::String(String::from(password)));
}

/// Forget everything we know about the session, credentials included.
pub fn clear_auth() {
	for key in [
		USERNAME_KEY,
		PASSWORD_KEY,
		LOGGED_IN_AT_KEY,
		RENEWED_AT_KEY,
		NEEDS_RELOGIN_KEY,
		LAST_FAILURE_KEY,
		DOMAINS_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
}

/// Whether the stored credentials were rejected and should not be retried on their own.
pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

/// Why the last login typed in settings failed, if it did.
pub fn last_failure() -> Option<LoginOutcome> {
	match defaults_get::<String>(LAST_FAILURE_KEY).as_deref() {
		Some("rejected") => Some(LoginOutcome::Rejected),
		Some("unreachable") => Some(LoginOutcome::Unreachable),
		_ => None,
	}
}

fn set_last_failure(value: &str) {
	defaults_set(LAST_FAILURE_KEY, DefaultValue::String(String::from(value)));
}

pub enum LoginOutcome {
	Success,
	/// The site answered and refused these credentials. Retrying changes nothing.
	Rejected,
	/// No usable answer. Worth retrying later.
	Unreachable,
}

/// The login nonce, from a page that is not signed in.
///
/// WordPress only answers `wp_manga_signin` for visitors who are signed out; for a signed-in
/// cookie it returns HTTP 400 with `0`. That cookie can outlive the source's own state (it sits
/// in the app's shared jar, so removing and reinstalling the source keeps it), and on
/// 2026-10-10 it made every login fail. So a signed-in page is logged out of first, which also
/// checks the password the reader typed instead of trusting the old session.
fn login_page_nonce() -> Option<String> {
	let home = format!("{}/", base_url());
	let page = fetch_text(&home).ok()?;
	let doc = Html::parse_with_url(&page, &home).ok()?;
	if !is_signed_in(&doc) {
		return login_nonce(&page);
	}
	println!("[bakamh] login: the site is already signed in, logging out first");
	let url = logout_url(&doc)?;
	site_get(&url).ok()?.timeout(LOGOUT_TIMEOUT).send().ok()?;
	let page = fetch_text(&home).ok()?;
	login_nonce(&page)
}

/// Signs in the way the site's own login form does.
///
/// The form posts a nonce that every page carries in `wpMangaLogin`, so a page is fetched
/// first. A wrong nonce gets `0`; a wrong account or password gets 200 with
/// `{"success":false,…}`; success is `{"success":true,"data":{"id":…}}` (checked 2026-10-09).
pub fn login(username: &str, password: &str) -> LoginOutcome {
	let Some(nonce) = login_page_nonce() else {
		println!("[bakamh] login: no nonce on the home page");
		return LoginOutcome::Unreachable;
	};

	let body = format!(
		"action=wp_manga_signin&login={}&pass={}&rememberme=forever&nonce={nonce}",
		encode_uri_component(username),
		encode_uri_component(password)
	);
	let ajax = format!("{}/wp-admin/admin-ajax.php", base_url());
	let Ok(mut request) = Request::post(&ajax) else {
		return LoginOutcome::Unreachable;
	};
	if let Some(cookie) = clearance_cookie(&ajax) {
		request = request.header("Cookie", &cookie);
	}
	let Ok(response) = request
		.header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
		.header("Referer", &format!("{}/", base_url()))
		.body(body.as_bytes())
		.send()
	else {
		println!("[bakamh] login request could not be sent");
		return LoginOutcome::Unreachable;
	};
	let status = response.status_code();
	let text = response.get_string().unwrap_or_default();

	if text.contains("\"success\":true") {
		defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
		remember_domain(&base_url());
		return LoginOutcome::Success;
	}
	if text.contains("\"success\":false") {
		println!("[bakamh] login refused by the site");
		return LoginOutcome::Rejected;
	}
	println!(
		"[bakamh] unexpected login response (status {status}, {} bytes)",
		text.len()
	);
	LoginOutcome::Unreachable
}

/// Records a login the reader performed through the settings screen.
pub fn handle_login(username: &str, password: &str) -> bool {
	match login(username, password) {
		LoginOutcome::Success => {
			store_credentials(username, password);
			// Only a login the reader performed may claim the `login` notification that
			// follows. A silent renewal must not, or a logout right after one would be
			// mistaken for a login and leave the credentials behind.
			set_timestamp(LOGGED_IN_AT_KEY, current_date());
			defaults_set(LAST_FAILURE_KEY, DefaultValue::Null);
			true
		}
		LoginOutcome::Rejected => {
			clear_auth();
			set_last_failure("rejected");
			false
		}
		LoginOutcome::Unreachable => {
			set_last_failure("unreachable");
			false
		}
	}
}

/// Called when a chapter says it needs a login although credentials are stored.
///
/// Returns whether the caller should fetch the chapter again. The gate is advanced before
/// any request, so a chapter that keeps asking - or several opened at once - signs in at
/// most once per `RENEW_GAP`.
///
/// Unlike zh.favcomic this does not end the old session first. favcomic does so because its
/// site caps signed-in devices; no such cap has been seen here, and ending a session costs two
/// more requests (a page for the logout nonce, then the logout link).
pub fn retry_after_lock() -> bool {
	if needs_relogin() {
		return false;
	}
	let Some((username, password)) = credentials() else {
		return false;
	};
	let now = current_date();
	if now < timestamp(RENEWED_AT_KEY) + RENEW_GAP {
		return false;
	}
	set_timestamp(RENEWED_AT_KEY, now);

	match login(&username, &password) {
		LoginOutcome::Success => {
			println!("[bakamh] signed in again after a chapter asked for a login");
			true
		}
		LoginOutcome::Rejected => {
			// The password changed or the account is gone: stop retrying until the reader
			// signs in again by hand.
			defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
			false
		}
		LoginOutcome::Unreachable => false,
	}
}

/// Ends the session on one domain, which clears its cookie from the shared jar.
///
/// The logout link carries a nonce tied to the session, so it is read off a page first.
/// `GET` on that link removed the `wordpress_logged_in_*` cookie on 2026-10-09.
fn end_site_session(base: &str) {
	// Read without `fetch_html`, which would mark the login item again through
	// `note_site_session` and, with the settings screen open, send another `login` notification.
	let home = format!("{base}/");
	let page = site_get(&home)
		.ok()
		.and_then(|request| request.timeout(LOGOUT_TIMEOUT).string().ok())
		.and_then(|text| Html::parse_with_url(&text, &home).ok())
		.filter(|doc| !is_challenge(doc));
	let Some(url) = page.as_ref().and_then(logout_url) else {
		println!("[bakamh] logout: no logout link on {base}, its sign-in stays on the site");
		return;
	};
	if let Ok(request) = site_get(&url) {
		let _ = request.timeout(LOGOUT_TIMEOUT).send();
	}
}

/// Logs out of every domain a login succeeded on, and the one in use. On 2026-10-09 a reader
/// logged out on a mirror and was still signed in on bakamh.com afterwards.
pub fn logout() {
	let mut domains = signed_in_domains();
	let current = base_url();
	if !domains.contains(&current) {
		domains.push(current);
	}
	for domain in domains {
		end_site_session(&domain);
	}
	clear_auth();
	// A page read by another entry point while this ran may have marked the login item again
	// through `note_site_session`; the sessions are over now. Cleared only when set, and with
	// the gate advanced first, so the notification this write sends is not taken for a logout.
	if defaults_get::<String>(LOGIN_ITEM_KEY).is_some_and(|v| !v.is_empty()) {
		set_timestamp(MARK_CLEARED_AT_KEY, current_date());
		defaults_set(LOGIN_ITEM_KEY, DefaultValue::Null);
	}
}

/// The account line under the login setting. Reads one page and never signs in: this runs
/// while the settings screen is drawn.
pub fn account_footer() -> String {
	if needs_relogin() {
		return String::from("儲存的帳號密碼已失效，請先登出再重新登入");
	}
	let Some(page) = fetch_html(&format!("{}/", base_url()))
		.ok()
		.filter(is_site_page)
	else {
		return String::from("無法連線到巴卡漫畫，請稍後再打開這一頁");
	};
	if !is_signed_in(&page) {
		return String::from("登入已失效，閱讀需登入的章節時會自動重新登入");
	}
	match user_name(&page) {
		Some(name) => format!("已登入：{name}"),
		None => String::from("已登入"),
	}
}

/// Aidoku sends the same `login` notification for logging in and logging out, so tell them
/// apart by how recently a login succeeded.
pub fn handle_login_notification() {
	let logged_in_at = timestamp(LOGGED_IN_AT_KEY);
	if logged_in_at != 0 && current_date() - logged_in_at <= LOGIN_WINDOW {
		defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
		return;
	}
	// The button's logout removes the key before notifying, and a manual login is caught
	// above, so a value here is the source's own mark from `note_site_session`.
	if defaults_get::<String>(LOGIN_ITEM_KEY).is_some_and(|v| !v.is_empty()) {
		return;
	}
	if current_date() - timestamp(MARK_CLEARED_AT_KEY) <= MARK_WINDOW {
		return;
	}
	logout();
}
