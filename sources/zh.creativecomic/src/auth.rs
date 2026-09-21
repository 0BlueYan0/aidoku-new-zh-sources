//! Identity: the guest uuid, OAuth tokens, and the account summary.
//!
//! Every API call must identify itself, either with a `uuid` header (guests) or a
//! bearer token (signed in). Without one the API answers `403 uuid錯誤`.
//!
//! Only the tokens are persisted, never the password: CCC issues a refresh token, so
//! there is no reason to keep credentials on the device.

use aidoku::{
	alloc::{String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
	},
	prelude::*,
	HashMap,
};
use serde::Deserialize;

use crate::crypto;
use crate::helper::{read_envelope, API_URL, DEVICE, USER_AGENT};

const UUID_KEY: &str = "guestUuid";
const ACCESS_TOKEN_KEY: &str = "accessToken";
const REFRESH_TOKEN_KEY: &str = "refreshToken";
/// Set by `handle_basic_login` so the follow-up notification can tell a sign-in from a
/// sign-out; Aidoku posts the same notification name for both.
const JUST_LOGGED_IN_KEY: &str = "justLoggedIn";
/// Names of the entries the web login view actually handed over. Recorded so the
/// settings footer can say what arrived when no token could be found - the app's
/// delivery of `localStorageKeys` is undocumented and no shipped source relies on it.
const SEEN_KEYS_KEY: &str = "webLoginKeys";

/// The site embeds this OAuth client in its web bundle.
const CLIENT_ID: &str = "2";
const CLIENT_SECRET: &str = "9eAhsCX3VWtyqTmkUo5EEaoH4MNPxrn6ZRwse7tE";

#[derive(Deserialize)]
struct TokenResponse {
	#[serde(default)]
	access_token: Option<String>,
	#[serde(default)]
	refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct Member {
	#[serde(default)]
	nickname: Option<String>,
	#[serde(default)]
	name: Option<String>,
	#[serde(default)]
	coin: Option<i64>,
	#[serde(default)]
	point: Option<i64>,
}

fn stored(key: &str) -> Option<String> {
	defaults_get::<String>(key).filter(|value| !value.is_empty())
}

pub fn access_token() -> Option<String> {
	stored(ACCESS_TOKEN_KEY)
}

pub fn is_logged_in() -> bool {
	access_token().is_some()
}

/// The credential page images are encrypted against: the access token when signed in,
/// otherwise the site's public guest secret.
pub fn image_secret() -> String {
	access_token().unwrap_or_else(|| String::from(crypto::GUEST_SECRET))
}

/// Fetch a guest uuid once and keep it, mirroring what the website stores in
/// localStorage. A fresh uuid per launch would look like a flood of new visitors.
fn guest_uuid() -> Option<String> {
	if let Some(existing) = stored(UUID_KEY) {
		return Some(existing);
	}

	let url = format!("{API_URL}/guest");
	let request = Request::get(&url)
		.ok()?
		.header("User-Agent", USER_AGENT)
		.header("device", DEVICE)
		.header("Accept-Language", "zh");
	let uuid: String = read_envelope(request).ok()?;
	if uuid.is_empty() {
		return None;
	}
	defaults_set(UUID_KEY, DefaultValue::String(uuid.clone()));
	Some(uuid)
}

/// Attach whichever credential this install has.
pub fn authorize(request: Request) -> Request {
	let mut request = request;
	if let Some(token) = access_token() {
		let header = format!("Bearer {token}");
		request.set_header("Authorization", header.as_str());
	} else if let Some(uuid) = guest_uuid() {
		request.set_header("uuid", uuid.as_str());
	}
	request
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
	let mut body = String::new();
	for (index, (key, value)) in pairs.iter().enumerate() {
		if index > 0 {
			body.push('&');
		}
		body.push_str(&crate::helper::encode_component(key));
		body.push('=');
		body.push_str(&crate::helper::encode_component(value));
	}
	body
}

/// POST the OAuth token endpoint. Returns true when new tokens were stored.
fn request_token(pairs: &[(&str, &str)]) -> bool {
	let url = format!("{API_URL}/token");
	let body = form_encode(pairs);
	let request = match Request::post(&url) {
		Ok(request) => request
			.header("User-Agent", USER_AGENT)
			.header("device", DEVICE)
			.header("Accept-Language", "zh")
			.header("Content-Type", "application/x-www-form-urlencoded")
			.body(body),
		Err(_) => {
			println!("[ccc] ERROR building the token request");
			return false;
		}
	};

	// The endpoint answers with a bare OAuth2 payload rather than the usual envelope.
	let response: TokenResponse = match request.json_owned() {
		Ok(response) => response,
		Err(_) => {
			println!("[ccc] ERROR the token endpoint did not return a usable payload");
			return false;
		}
	};

	let Some(access) = response.access_token.filter(|value| !value.is_empty()) else {
		println!("[ccc] ERROR the token endpoint returned no access token");
		return false;
	};

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access));
	if let Some(refresh) = response.refresh_token.filter(|value| !value.is_empty()) {
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	true
}

/// Take the tokens out of whatever the web login view handed over.
///
/// The site keeps its session in `localStorage` (`accessToken` / `refreshToken`) rather
/// than in a cookie, which is why `settings.json` asks for those keys. The app merges
/// what it collected into this one map, so both are looked up here by name.
pub fn capture_web_login(values: &HashMap<String, String>) -> bool {
	// Record the key names (never the values) so a failed sign-in can be diagnosed from
	// the settings screen instead of needing a log server.
	let mut seen: Vec<String> = values.keys().cloned().collect();
	seen.sort();
	defaults_set(SEEN_KEYS_KEY, DefaultValue::String(seen.join(", ")));

	let access = values
		.get("accessToken")
		.filter(|value| !value.is_empty())
		.cloned();
	let Some(access) = access else {
		return false;
	};

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access));
	if let Some(refresh) = values
		.get("refreshToken")
		.filter(|value| !value.is_empty())
		.cloned()
	{
		defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String(refresh));
	}
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Bool(true));
	true
}

/// Swap the refresh token for a fresh pair. Returns true when it worked.
pub fn refresh() -> bool {
	let Some(refresh_token) = stored(REFRESH_TOKEN_KEY) else {
		return false;
	};
	request_token(&[
		("grant_type", "refresh_token"),
		("client_id", CLIENT_ID),
		("client_secret", CLIENT_SECRET),
		("refresh_token", refresh_token.as_str()),
	])
}

pub fn clear() {
	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::Null);
	defaults_set(REFRESH_TOKEN_KEY, DefaultValue::Null);
	defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	defaults_set(SEEN_KEYS_KEY, DefaultValue::Null);
}

/// Aidoku posts the same notification for signing in and signing out, so the flag set
/// during `handle_basic_login` is what tells them apart.
pub fn handle_login_notification() {
	let just_logged_in = defaults_get::<bool>(JUST_LOGGED_IN_KEY).unwrap_or(false);
	if just_logged_in {
		defaults_set(JUST_LOGGED_IN_KEY, DefaultValue::Null);
	} else {
		clear();
	}
}

fn fetch_member() -> Option<Member> {
	let request = crate::helper::api_request("/member").ok()?;
	read_envelope::<Member>(request).ok()
}

/// The account summary shown in settings.
///
/// Those balances are also the only visible proof that signing in actually took, so
/// when they cannot be read this says which failure it was instead of going blank.
pub fn account_footer() -> Option<String> {
	if !is_logged_in() {
		// If the login view ran but left no token, name what it did hand over; that is
		// the one clue available for why sign-in did not take.
		if let Some(seen) = stored(SEEN_KEYS_KEY) {
			return Some(format!(
				"登入未完成：登入頁只交回了 {seen}，其中沒有 accessToken。請回報這行字。"
			));
		}
		return Some(String::from("尚未登入。登入後可閱讀已購買的付費章節。"));
	}

	let member = match fetch_member() {
		Some(member) => Some(member),
		// A stale access token is the usual cause, so try the refresh token once.
		None if refresh() => fetch_member(),
		None => None,
	};

	let Some(member) = member else {
		return Some(String::from("登入已失效，請重新登入。"));
	};

	let name = member
		.nickname
		.filter(|value| !value.is_empty())
		.or(member.name)
		.unwrap_or_else(|| String::from("已登入"));

	let mut parts: Vec<String> = Vec::new();
	parts.push(name);
	parts.push(format!("金幣 {}", member.coin.unwrap_or(0)));
	parts.push(format!("點數 {}", member.point.unwrap_or(0)));
	Some(parts.join("・"))
}
