//! Official web-login session. No passwords or unrelated browser cookies are stored.
use aidoku::{
	alloc::String,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		js::{Cookie, WebView},
		net::Request,
		std::current_date,
	},
	prelude::*,
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

const SESSION_KEY: &str = "auth_session";
const NAME_KEY: &str = "auth_nickname";
const NEXT_CHECK_KEY: &str = "auth_next_check_at";
const NEEDS_RELOGIN_KEY: &str = "auth_needs_relogin";
const FAILED_KEY: &str = "auth_failed_cookie_hash";
const FAILED_AT_KEY: &str = "auth_failed_at";

#[derive(Deserialize, Serialize)]
struct SessionCookies {
	session: String,
	check: String,
}

impl SessionCookies {
	fn from_store(cookies: &[Cookie]) -> Option<Self> {
		let value = |name: &str| cookies.iter().find(|cookie| {
			let domain = cookie.domain.trim_start_matches('.');
			(domain == "webtoons.com" || domain == "www.webtoons.com")
				&& cookie.name == name
				&& cookie.expires_date.is_none_or(|expiry| expiry > current_date())
		}).map(|cookie| cookie.value.trim_matches('"'));
		let session = value("NEO_SES")?;
		let check = value("NEO_CHK")?;
		let candidate = Self { session: session.into(), check: check.into() };
		candidate.valid().then_some(candidate)
	}

	fn valid(&self) -> bool {
		[&self.session, &self.check].iter().all(|value| {
			!value.is_empty() && !value.eq_ignore_ascii_case("expired")
				&& !value.chars().any(|ch| ch.is_control() || matches!(ch, ';' | '"'))
		})
	}

	fn api_header(&self) -> String {
		// Android's PersistentCookieHandlerImpl quotes NEO_SES for .apis.naver.com.
		format!("NEO_SES=\"{}\"; NEO_CHK={}", self.session, self.check)
	}

	fn web_header(&self) -> String {
		format!("NEO_SES={}; NEO_CHK={}", self.session, self.check)
	}
}

fn stored_session() -> Option<SessionCookies> {
	let raw = defaults_get::<String>(SESSION_KEY)?;
	let session: SessionCookies = serde_json::from_str(&raw).ok()?;
	session.valid().then_some(session)
}

pub fn needs_relogin() -> bool {
	defaults_get::<bool>(NEEDS_RELOGIN_KEY).unwrap_or(false)
}

pub fn is_logged_in() -> bool {
	!needs_relogin() && stored_session().is_some()
}

pub fn api_cookie() -> Option<String> {
	if needs_relogin() { return None; }
	stored_session().map(|session| session.api_header())
}

pub fn web_cookie() -> Option<String> {
	if needs_relogin() { return None; }
	stored_session().map(|session| session.web_header())
}

pub fn with_web_session(request: Request) -> Request {
	if !request.url().is_some_and(|url| trusted_web_url(url)) { return request; }
	match web_cookie() {
		Some(cookie) => request.header("Cookie", cookie.as_str()),
		None => request,
	}
}

fn trusted_web_url(url: &str) -> bool {
	url.starts_with("https://www.webtoons.com/") || url.starts_with("https://m.webtoons.com/")
}

pub fn account_footer() -> String {
	if needs_relogin() {
		String::from("登入已失效，請先登出再重新登入。")
	} else if is_logged_in() {
		let nickname = defaults_get::<String>(NAME_KEY).unwrap_or_default();
		format!("已登入：{nickname}\n已購買的章節可直接閱讀。若剛在官方 App 購買，請重新整理作品章節清單。")
	} else {
		String::from("尚未登入。免費與試閱內容可直接閱讀；已購買內容請登入同一個 WEBTOON 帳號。")
	}
}

fn timestamp(key: &str) -> i64 {
	defaults_get::<String>(key).and_then(|value| value.parse().ok()).unwrap_or(0)
}

fn set_timestamp(key: &str, value: i64) {
	defaults_set(key, DefaultValue::String(format!("{value}")));
}

fn accept_session(session: SessionCookies, nickname: &str) {
	let Ok(raw) = serde_json::to_string(&session) else { return; };
	defaults_set(SESSION_KEY, DefaultValue::String(raw));
	defaults_set(NAME_KEY, DefaultValue::String(nickname.into()));
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(false));
	set_timestamp(NEXT_CHECK_KEY, current_date() + 300);
	defaults_set(FAILED_KEY, DefaultValue::Null);
	defaults_set(FAILED_AT_KEY, DefaultValue::Null);
	// SettingView notifies on this value change for both native and manual login.
	// Do not rewrite an unchanged key and trigger redundant setting notifications.
	if defaults_get::<String>("login").as_deref() != Some("logged_in") {
		defaults_set("login", DefaultValue::String("logged_in".into()));
	}
}

fn read_web_session() -> Option<SessionCookies> {
	let cookies = WebView::new().get_cookies().ok()?;
	let has_session = cookies.iter().any(|cookie| cookie.name == "NEO_SES");
	let has_check = cookies.iter().any(|cookie| cookie.name == "NEO_CHK");
	println!("[webtoons] login store: {} cookies, NEO_SES={}, NEO_CHK={}",
		cookies.len(), has_session, has_check);
	SessionCookies::from_store(&cookies)
}

/// A callback may carry an empty cookie map because login redirects to another
/// host and the App filters parent-domain cookies. Read the shared source store.
pub fn capture_web_login(force_verify: bool) -> bool {
	let Some(session) = read_web_session() else { return false; };
	let header = session.api_header();
	if !force_verify && api_cookie().as_deref() == Some(header.as_str()) {
		return true;
	}
	let fingerprint: String = Sha1::digest(header.as_bytes()).iter()
		.map(|byte| format!("{byte:02x}")).collect();
	if defaults_get::<String>(FAILED_KEY).as_deref() == Some(fingerprint.as_str())
		&& current_date() - timestamp(FAILED_AT_KEY) < 15 {
		return false;
	}
	// Gate before I/O so cookie-change callbacks do not issue duplicate probes.
	defaults_set(FAILED_KEY, DefaultValue::String(fingerprint));
	set_timestamp(FAILED_AT_KEY, current_date());
	match crate::api::member_with_cookie(&header) {
		Ok(Some(member)) => {
			accept_session(session, &member.nickname);
			println!("[webtoons] official login verified by App API");
			true
		}
		Ok(None) => {
			mark_relogin(&header);
			println!("[webtoons] login not verified: App API reports not logged in");
			false
		}
		Err(_) => {
			println!("[webtoons] login verification unavailable; session unchanged");
			false
		}
	}
}

/// Expired credentials stop automatically retrying. A response from an old
/// session cannot invalidate a newer login which completed while it was in flight.
pub fn mark_relogin(request_cookie: &str) {
	if api_cookie().as_deref() == Some(request_cookie) {
		defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(true));
		println!("[webtoons] login expired; automatic probes stopped");
	}
}

pub fn ensure_session() {
	let Some(header) = api_cookie() else { return; };
	if current_date() < timestamp(NEXT_CHECK_KEY) { return; }
	set_timestamp(NEXT_CHECK_KEY, current_date() + 300);
	match crate::api::member_with_cookie(&header) {
		Ok(Some(member)) => {
			if api_cookie().as_deref() == Some(header.as_str()) {
				defaults_set(NAME_KEY, DefaultValue::String(member.nickname));
			}
		}
		Ok(None) => {
			// The login page can have refreshed its cookies since the last capture.
			if let Some(new_session) = read_web_session() {
				if new_session.api_header() != header {
					if let Ok(Some(member)) = crate::api::member_with_cookie(&new_session.api_header()) {
						if api_cookie().as_deref() == Some(header.as_str()) {
							accept_session(new_session, &member.nickname);
						}
						return;
					}
				}
			}
			mark_relogin(&header);
		}
		Err(_) => {
			// Transport errors do not mean logout. Back off without clearing credentials.
			set_timestamp(NEXT_CHECK_KEY, current_date() + 60);
		}
	}
}

pub fn handle_login_notification() {
	if !login_notification_is_logout() {
		println!("[webtoons] login notification: no logout requested, session preserved");
		return;
	}
	let session = stored_session();
	// Clear first so concurrent refreshes cannot use the account during logout.
	clear_local();
	if let Some(session) = session {
		let result = Request::get("https://www.webtoons.com/member/logout?returnUrl=https%3A%2F%2Fwww.webtoons.com%2Fzh-hant%2F")
			.map(|request| request.timeout(8.0).header("Cookie", session.web_header().as_str()))
			.and_then(|request| request.send());
		println!("[webtoons] official web logout request completed={}", result.is_ok());
	}
	let cleared = WebView::new().delete_all_cookies().is_ok();
	println!("[webtoons] logout: source web store cleared={cleared}");
}

fn login_notification_is_logout() -> bool {
	// Aidoku writes a nonempty login setting on successful login and removes it
	// BEFORE posting the notification for an explicit user logout. Notifications
	// can be delayed or repeated; elapsed time cannot identify their meaning.
	stored_session().is_some()
		&& defaults_get::<String>("login").is_none_or(|value| value.is_empty())
}

fn clear_local() {
	crate::unlock::clear_session_state();
	for key in [SESSION_KEY, NAME_KEY, NEXT_CHECK_KEY, "auth_logged_in_at",
		FAILED_KEY, FAILED_AT_KEY, "login"] {
		// Overwrite first: even a failed deletion must not leave usable credentials.
		// The current test runner also rejects Null's zero pointer before removing it.
		defaults_set(key, DefaultValue::String(String::new()));
		defaults_set(key, DefaultValue::Null);
	}
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Bool(false));
	defaults_set(NEEDS_RELOGIN_KEY, DefaultValue::Null);
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	fn cookie(name: &str, value: &str, domain: &str) -> Cookie {
		Cookie {
			name: name.into(), value: value.into(), domain: domain.into(),
			path: "/".into(), expires_date: None, is_secure: true, is_http_only: true,
		}
	}

	#[aidoku_test]
	fn manual_sync_and_repeated_login_notifications_keep_the_verified_session() {
		clear_local();
		accept_session(SessionCookies { session: "sync".into(), check: "sync".into() }, "測試帳號");
		assert!(!login_notification_is_logout());
		assert!(!login_notification_is_logout());
		assert!(is_logged_in());
	}

	#[aidoku_test]
	fn delayed_login_notification_keeps_session_but_immediate_real_logout_is_honored() {
		clear_local();
		accept_session(SessionCookies { session: "auto".into(), check: "auto".into() }, "測試帳號");
		set_timestamp("auth_logged_in_at", current_date() - 600);
		assert!(!login_notification_is_logout());
		// Aidoku clears its login setting before posting the user's logout notification.
		defaults_set("login", DefaultValue::String(String::new()));
		assert!(login_notification_is_logout());
		clear_local();
		assert!(!login_notification_is_logout());
	}

	#[aidoku_test]
	fn web_session_never_follows_foreign_or_insecure_urls() {
		assert!(trusted_web_url("https://www.webtoons.com/zh-hant/"));
		assert!(trusted_web_url("https://m.webtoons.com/api/v1/webtoon/10568/episodes"));
		assert!(!trusted_web_url("https://www.webtoons.com.evil.test/"));
		assert!(!trusted_web_url("http://www.webtoons.com/zh-hant/"));
	}

	#[aidoku_test]
	fn only_webtoon_session_cookies_are_forwarded_and_api_session_is_quoted() {
		let cookies = [
			cookie("NEO_SES", "session", ".webtoons.com"),
			cookie("NEO_CHK", "check", "www.webtoons.com"),
			cookie("tracking", "private", ".webtoons.com"),
			cookie("NEO_SES", "foreign", "webtoons.com.evil.test"),
		];
		let session = SessionCookies::from_store(&cookies).unwrap();
		assert_eq!(session.web_header(), "NEO_SES=session; NEO_CHK=check");
		assert_eq!(session.api_header(), "NEO_SES=\"session\"; NEO_CHK=check");
	}

	#[aidoku_test]
	fn incomplete_expired_or_injected_cookies_are_not_a_login() {
		assert!(SessionCookies::from_store(&[cookie("NEO_SES", "session", ".webtoons.com")]).is_none());
		assert!(SessionCookies::from_store(&[
			cookie("NEO_SES", "expired", ".webtoons.com"),
			cookie("NEO_CHK", "check", ".webtoons.com"),
		]).is_none());
		assert!(SessionCookies::from_store(&[
			cookie("NEO_SES", "x\r\nInjected: y", ".webtoons.com"),
			cookie("NEO_CHK", "check", ".webtoons.com"),
		]).is_none());
	}

	#[aidoku_test]
	fn real_notification_handler_preserves_session_on_repeated_sync_notifications() {
		clear_local();
		accept_session(SessionCookies { session: "sync".into(), check: "sync".into() }, "測試帳號");
		handle_login_notification();
		handle_login_notification();
		assert!(is_logged_in());
		assert!(api_cookie().is_some());
		clear_local();
		assert!(!is_logged_in());
		assert!(api_cookie().is_none());
	}

	#[aidoku_test]
	fn immediate_real_logout_is_not_hidden_by_a_recent_login() {
		clear_local();
		accept_session(SessionCookies { session: "recent".into(), check: "recent".into() }, "測試帳號");
		defaults_set("login", DefaultValue::String(String::new()));
		assert!(login_notification_is_logout());
	}

	#[aidoku_test]
	fn a_stale_request_cannot_expire_a_new_session() {
		clear_local();
		accept_session(SessionCookies { session: "new".into(), check: "new".into() }, "測試帳號");
		mark_relogin("NEO_SES=\"old\"; NEO_CHK=old");
		assert!(is_logged_in());
		let current = api_cookie().unwrap();
		mark_relogin(&current);
		assert!(!is_logged_in());
		assert!(needs_relogin());
		assert!(api_cookie().is_none());
		clear_local();
	}
}
