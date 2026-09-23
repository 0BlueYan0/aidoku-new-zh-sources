//! Self-managed login session.
//!
//! The app's login page writes the site's cookies into a per-source web store, but the
//! cookies it hands `handle_web_login` are filtered to the login page's host
//! (`www.bookwalker.com.tw`), and the site sets its login cookies on the parent domain
//! `.bookwalker.com.tw`, so that callback never sees them (app source, 2026-09-23). The
//! app also does not carry the store into the request jar the source's own `Request`
//! uses (verified on device 2026-09-23). So the source reads the login cookies out of
//! the same per-source store through its own web view, confirms them against the
//! bookcase, and attaches them to every request itself.

use aidoku::{
	alloc::{string::ToString, String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		js::{Cookie, WebView},
		net::Request,
		std::current_date,
	},
	prelude::*,
	Result,
};

use crate::helper::{is_bookcase, parse_bookcase, BASE_URL, USER_AGENT};

/// The `Cookie` header value captured from the per-source web store.
const COOKIE_KEY: &str = "auth_cookie";
/// Purchased series/book count from the last successful capture, shown in the footer.
const COUNT_KEY: &str = "auth_book_count";
/// When the last capture succeeded. The `login` notification arrives for both signing
/// in and signing out, so a fresh timestamp is what marks it as the former.
const LOGGED_IN_AT_KEY: &str = "auth_logged_in_at";
/// The cookie header the last failed verification used. The login page reports every
/// cookie change, most of which leave the login cookies untouched, so an identical
/// header is not sent to the site again.
const LAST_FAILED_KEY: &str = "auth_last_failed";

/// A `login` notification within this many seconds of a capture is the sign-in, not a
/// sign-out. A timestamp rather than a flag (zh.komiic does the same): a late duplicate
/// callback after the login page closed would leave a flag set and turn the next logout
/// into a no-op.
const LOGIN_WINDOW: i64 = 60;

/// The archive shelf lists the volumes the user hid on the site (`封存`). Archived
/// volumes stay in the buy shelf, and neither the buy shelf nor the per-series view
/// marks them, so this shelf is the only way to tell an archived volume from a
/// readable one. The reader answers 991 for an archived volume until it is restored on
/// the website, so this only labels them; it cannot make them open.
const ARCHIVE_PATH: &str = "/bookcase/available_book_list/archive?sd=1&d=0&sort=0";
/// Comma-joined archived product ids, and when they were last read.
const ARCHIVED_IDS_KEY: &str = "auth_archived_ids";
const ARCHIVED_AT_KEY: &str = "auth_archived_at";
/// Refresh the archived set at most this often. Archiving is rare, and this rides on
/// the chapter-list path, so it is gated like zh.komiic's session probe: every branch
/// that would fetch advances the gate first, so parallel updates fetch it only once.
const ARCHIVE_TTL: i64 = 300;

/// Cookies worth sending back to the site. Tracking cookies are dropped so the
/// header stays short and stable.
const WANTED_COOKIE_PREFIXES: [&str; 3] = ["remember_web_", "new_se", "bweternity"];

fn bookcase_url() -> String {
	format!("{BASE_URL}/bookcase/available_book_list/buy?sd=1&d=0&sort=0&c=0")
}

/// The one request shape for every bookcase page, so the verification here and the
/// content path in `lib.rs` cannot drift apart (a different User-Agent could make the
/// site answer one of them differently).
pub fn bookcase_request(url: &str, cookie: &str) -> Result<Request> {
	Ok(Request::get(url)?
		.header("User-Agent", USER_AGENT)
		.header("Cookie", cookie))
}

pub fn is_logged_in() -> bool {
	cookie_header().is_some()
}

/// Attached to every request. `None` before the first successful capture.
pub fn cookie_header() -> Option<String> {
	defaults_get::<String>(COOKIE_KEY).filter(|header| !header.is_empty())
}

pub fn book_count() -> Option<i32> {
	defaults_get::<String>(COUNT_KEY).and_then(|value| value.parse().ok())
}

/// Called from `handle_web_login` on every cookie change of the login page. Reads the
/// login cookies out of the per-source web store, confirms them by reading the bookcase
/// back, and stores them. Returns whether the source is now logged in, which the app
/// takes as "close the login page".
pub fn capture_from_web_login() -> bool {
	let Some(header) = read_cookie_from_store() else {
		println!("[bookwalker] login: store has no login cookie yet");
		return false;
	};
	if defaults_get::<String>(LAST_FAILED_KEY).as_deref() == Some(header.as_str()) {
		println!("[bookwalker] login: same cookies as the last failed check, skipped");
		return false;
	}
	match verify(&header) {
		Some(count) => {
			defaults_set(COOKIE_KEY, DefaultValue::String(header));
			defaults_set(COUNT_KEY, DefaultValue::String(count.to_string()));
			defaults_set(
				LOGGED_IN_AT_KEY,
				DefaultValue::String(current_date().to_string()),
			);
			defaults_set(LAST_FAILED_KEY, DefaultValue::Null);
			println!("[bookwalker] login ok, {count} series/books");
			true
		}
		None => {
			defaults_set(LAST_FAILED_KEY, DefaultValue::String(header));
			println!("[bookwalker] login: cookies did not read the bookcase back");
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
		println!("[bookwalker] login notification taken as sign-in, state kept");
		return;
	}
	// The app clears the per-source web store in a detached task; clearing it here as
	// well makes sure an immediate re-login cannot pick the old cookies back up.
	match WebView::new().delete_all_cookies() {
		Ok(()) => println!("[bookwalker] logout: web store cookies deleted"),
		Err(_) => println!("[bookwalker] logout: could not delete web store cookies"),
	}
	clear_all();
}

/// Drops everything that only exists after a login, reader sessions included.
pub fn clear_all() {
	defaults_set(COOKIE_KEY, DefaultValue::Null);
	defaults_set(COUNT_KEY, DefaultValue::Null);
	defaults_set(LOGGED_IN_AT_KEY, DefaultValue::Null);
	defaults_set(LAST_FAILED_KEY, DefaultValue::Null);
	defaults_set(ARCHIVED_IDS_KEY, DefaultValue::Null);
	defaults_set(ARCHIVED_AT_KEY, DefaultValue::Null);
	crate::reader::clear_sessions();
}

/// Product ids the user has archived on the site. The chapter list marks any volume in
/// this set so the reader's 991 does not look like a broken book. The set is cached and
/// refreshed from the archive shelf at most every `ARCHIVE_TTL` seconds. Empty when
/// signed out or when the shelf has not been read yet.
pub fn archived_ids() -> Vec<String> {
	refresh_archived_if_stale();
	defaults_get::<String>(ARCHIVED_IDS_KEY)
		.unwrap_or_default()
		.split(',')
		.filter(|id| !id.is_empty())
		.map(String::from)
		.collect()
}

fn refresh_archived_if_stale() {
	let Some(cookie) = cookie_header() else {
		return;
	};
	let at = defaults_get::<String>(ARCHIVED_AT_KEY)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0);
	if current_date() - at < ARCHIVE_TTL {
		return;
	}
	// Advance the gate before the request, so several manga updating at once fetch the
	// shelf only once (the zh.komiic rule).
	defaults_set(ARCHIVED_AT_KEY, DefaultValue::String(current_date().to_string()));

	let url = format!("{BASE_URL}{ARCHIVE_PATH}");
	let Some(html) = bookcase_request(&url, &cookie)
		.ok()
		.and_then(|request| request.html().ok())
	else {
		println!("[bookwalker] archive shelf fetch failed, keeping the cached set");
		return;
	};
	if !is_bookcase(&html) {
		// An expired cookie redirects to the login page; leave the cache untouched.
		return;
	}
	let ids: Vec<String> = parse_bookcase(&html)
		.into_iter()
		.flat_map(|card| card.product_ids)
		.collect();
	println!("[bookwalker] archive shelf has {} archived volumes", ids.len());
	defaults_set(ARCHIVED_IDS_KEY, DefaultValue::String(ids.join(",")));
}

/// Reads the bookcase with `header`; `Some(count)` when the page really is the bookcase.
fn verify(header: &str) -> Option<i32> {
	let html = bookcase_request(&bookcase_url(), header).ok()?.html().ok()?;
	is_bookcase(&html).then(|| parse_bookcase(&html).len() as i32)
}

/// Reads the login cookies straight out of the per-source web store, without loading
/// a page. A load would rotate the site's session cookies, the login page observes that
/// store, and it would call `handle_web_login` again for every rotation. The cookie
/// names (never the values) are logged: whether this store shows the login page's
/// cookies while that page is still open is only known from the device.
fn read_cookie_from_store() -> Option<String> {
	let cookies = WebView::new().get_cookies().ok()?;
	let names: Vec<&str> = cookies.iter().map(|cookie| cookie.name.as_str()).collect();
	println!(
		"[bookwalker] login: store has {} cookies: {}",
		cookies.len(),
		names.join(",")
	);
	cookie_header_from(&cookies)
}

fn cookie_header_from(cookies: &[Cookie]) -> Option<String> {
	let pairs: Vec<String> = cookies
		.iter()
		.filter(|cookie| {
			cookie.domain.trim_start_matches('.') == "bookwalker.com.tw"
				&& WANTED_COOKIE_PREFIXES
					.iter()
					.any(|prefix| cookie.name.starts_with(prefix))
		})
		.map(|cookie| format!("{}={}", cookie.name, cookie.value))
		.collect();
	(!pairs.is_empty()).then(|| pairs.join("; "))
}
