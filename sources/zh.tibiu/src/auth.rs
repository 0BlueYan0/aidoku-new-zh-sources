//! Session state for the web login.
//!
//! Deliberately much lighter than `zh.komiic`'s auth module. Komiic needs an active
//! session probe because a dead token still returns HTTP 200 with plausible data, so
//! the reader never notices. Here an expired session only makes purchased chapters go
//! back to showing a lock — a visible, harmless degradation that is not worth spending
//! extra requests to pre-empt.

use aidoku::{
	alloc::{String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		std::current_date,
	},
	prelude::*,
};

use crate::helper::{
	fetch_json, json_data_field, json_id, json_num_value, json_string_array, json_text,
	json_top_level_objects, API_URL,
};

const LOGGED_IN_KEY: &str = "auth_logged_in";
const NICKNAME_KEY: &str = "auth_nickname";
const VIP_KEY: &str = "auth_vip";
const VIP_TIME_KEY: &str = "auth_vip_time";
const CION_KEY: &str = "auth_cion";
const TICKET_KEY: &str = "auth_ticket";

pub struct UserInfo {
	pub logged_in: bool,
	pub nickname: String,
	pub vip: i32,
	pub vip_time: i32,
	pub cion: i32,
	pub ticket: i32,
}

/// Ask the server who we are. A guest answers `{"log":0,"nichen":"游客",...}`.
///
/// This is how login is detected, rather than looking for a named cookie: the site only
/// sets its session cookie on a successful login, so the name cannot be known upfront.
pub fn fetch_user_info() -> Option<UserInfo> {
	let url = format!("{API_URL}/user/info?t={}", current_date());
	let body = fetch_json(&url).ok()?;
	let data = json_data_field(&body, "data")?;

	Some(UserInfo {
		logged_in: json_num_value(data, "log").unwrap_or(0) != 0,
		nickname: json_text(data, "nichen").unwrap_or_default(),
		vip: json_num_value(data, "vip").unwrap_or(0) as i32,
		vip_time: json_num_value(data, "viptime").unwrap_or(0) as i32,
		cion: json_num_value(data, "cion").unwrap_or(0) as i32,
		ticket: json_num_value(data, "ticket").unwrap_or(0) as i32,
	})
}

pub fn is_logged_in() -> bool {
	defaults_get::<bool>(LOGGED_IN_KEY).unwrap_or(false)
}

pub fn cached_user_info() -> Option<UserInfo> {
	if !is_logged_in() {
		return None;
	}

	Some(UserInfo {
		logged_in: true,
		nickname: defaults_get::<String>(NICKNAME_KEY).unwrap_or_default(),
		vip: defaults_get::<i32>(VIP_KEY).unwrap_or(0),
		vip_time: defaults_get::<i32>(VIP_TIME_KEY).unwrap_or(0),
		cion: defaults_get::<i32>(CION_KEY).unwrap_or(0),
		ticket: defaults_get::<i32>(TICKET_KEY).unwrap_or(0),
	})
}

fn cache_user_info(info: &UserInfo) {
	defaults_set(LOGGED_IN_KEY, DefaultValue::Bool(info.logged_in));
	defaults_set(NICKNAME_KEY, DefaultValue::String(info.nickname.clone()));
	defaults_set(VIP_KEY, DefaultValue::Int(info.vip));
	defaults_set(VIP_TIME_KEY, DefaultValue::Int(info.vip_time));
	defaults_set(CION_KEY, DefaultValue::Int(info.cion));
	defaults_set(TICKET_KEY, DefaultValue::Int(info.ticket));
}

pub fn clear_auth() {
	for key in [
		LOGGED_IN_KEY,
		NICKNAME_KEY,
		VIP_KEY,
		VIP_TIME_KEY,
		CION_KEY,
		TICKET_KEY,
	] {
		defaults_set(key, DefaultValue::Null);
	}
}

/// Re-probe the server and update the cached account state. Returns whether we are
/// logged in. A failed request leaves the previous state alone, so a flaky network
/// does not silently log the reader out.
pub fn refresh_session() -> bool {
	match fetch_user_info() {
		Some(info) => {
			if info.logged_in {
				cache_user_info(&info);
			} else {
				clear_auth();
			}
			info.logged_in
		}
		None => is_logged_in(),
	}
}

/// Chapter ids the logged-in reader already owns, used to clear the lock flag.
///
/// The exact shape of `buy[]` could not be confirmed without an account, so this tries
/// the two plausible object keys and also copes with a bare array of ids. Anything it
/// cannot read yields an empty list, which simply leaves the chapters locked.
pub fn purchased_chapter_ids(manga_key: &str) -> Vec<String> {
	let mut ids: Vec<String> = Vec::new();

	let url = format!("{API_URL}/comic/buyread?mid={manga_key}");
	let body = match fetch_json(&url) {
		Ok(body) => body,
		Err(_) => return ids,
	};

	if let Some(array) = json_data_field(&body, "buy") {
		for obj in json_top_level_objects(array) {
			// The site's own reader reads `cid` off these entries.
			if let Some(id) = json_id(obj, "cid").or_else(|| json_id(obj, "id")) {
				ids.push(id);
			}
		}
	}

	if ids.is_empty() {
		ids = json_string_array(&body, "buy");
	}

	ids
}
