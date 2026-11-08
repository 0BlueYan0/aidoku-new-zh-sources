//! Daily check-in (打卡) on the site, done for the reader while the `autoPunchIn` switch
//! is on. Shaped after zh.lightnovel's `sign_in.rs`.
//!
//! `punch_if_due` runs at the top of each Source entry point, after `ensure_session`. A
//! gate in defaults keeps it to one attempt per site day, and every branch moves the gate
//! (CLAUDE.md, 請求排程): the gate moves to `now + RETRY` before the request is sent, an
//! answer from the site (checked in now, or already checked in) pushes it to the next
//! site day, anything else leaves the one-hour retry. The site's day is taken to start at
//! midnight UTC+8; if that is wrong, the cost is one "already checked in" answer a day.

use aidoku::prelude::format;
use aidoku::{
	alloc::{string::ToString, String},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::HttpMethod,
		std::current_date,
	},
};
use serde::Deserialize;

use crate::{auth, net};

pub const ENABLED_KEY: &str = "autoPunchIn";
const NEXT_KEY: &str = "punch_next";
const LAST_ERROR_KEY: &str = "punch_last_error";
/// Where the gate moves before an attempt is sent, and what a failed attempt leaves.
const RETRY: i64 = 3600;
const DAY: i64 = 86_400;
const SITE_UTC_OFFSET: i64 = 8 * 3600;

pub const FAILED_TEXT: &str = "自動打卡失敗，一小時後會再試";

fn enabled() -> bool {
	defaults_get::<bool>(ENABLED_KEY).unwrap_or(false)
}

fn timestamp(key: &str) -> i64 {
	defaults_get::<String>(key)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0)
}

fn set_string(key: &str, value: &str) {
	defaults_set(key, DefaultValue::String(String::from(value)));
}

fn set_timestamp(key: &str, value: i64) {
	set_string(key, &value.to_string());
}

/// Days since the epoch, counted in the site's time zone.
fn site_day(now: i64) -> i64 {
	(now + SITE_UTC_OFFSET).div_euclid(DAY)
}

/// First second of the site day after the one holding `now`.
pub fn next_site_day(now: i64) -> i64 {
	(site_day(now) + 1) * DAY - SITE_UTC_OFFSET
}

/// Whether to send the check-in now. When it is due, the gate moves to `now + RETRY`
/// here, before anything is sent, so entry points running together send it once.
fn claim_if_due(now: i64) -> bool {
	if !enabled() || !auth::logged_in() || now < timestamp(NEXT_KEY) {
		return false;
	}
	set_timestamp(NEXT_KEY, now + RETRY);
	true
}

pub enum Outcome {
	/// The site took the check-in (`res.status` "ok").
	Done,
	/// The site answered but did not take it; PicaWeb reads any status other than "ok"
	/// this way, and "fail" is what an already checked-in account gets.
	AlreadyDone,
	/// No usable answer.
	Failed,
}

#[derive(Deserialize)]
struct Envelope {
	data: Option<Data>,
}

#[derive(Deserialize)]
struct Data {
	res: Option<Res>,
}

#[derive(Deserialize)]
struct Res {
	#[serde(default)]
	status: String,
}

/// Read a `users/punch-in` answer. PicaWeb (`useUserProfile-*.js`, 2026-11-08) treats
/// `code` 200 with `data.res.status == "ok"` as checked in.
pub fn parse(status: i32, text: &str) -> Outcome {
	if !(200..300).contains(&status) {
		return Outcome::Failed;
	}
	match serde_json::from_str::<Envelope>(text) {
		Ok(Envelope {
			data: Some(Data { res: Some(res) }),
		}) => {
			if res.status == "ok" {
				Outcome::Done
			} else {
				Outcome::AlreadyDone
			}
		}
		_ => Outcome::Failed,
	}
}

fn record(outcome: &Outcome, now: i64) {
	match outcome {
		Outcome::Done | Outcome::AlreadyDone => {
			set_timestamp(NEXT_KEY, next_site_day(now));
			set_string(LAST_ERROR_KEY, "");
		}
		// The gate `claim_if_due` set tries again in an hour.
		Outcome::Failed => set_string(LAST_ERROR_KEY, FAILED_TEXT),
	}
}

/// POST users/punch-in once. Sent at most once per site day, see the module comment.
fn send() -> Outcome {
	let base = net::api_base();
	let Ok(request) = net::create_request(
		format!("{base}/users/punch-in"),
		&base,
		HttpMethod::Post,
		None,
		true,
	) else {
		return Outcome::Failed;
	};
	let Ok(response) = request.send() else {
		return Outcome::Failed;
	};
	let status = response.status_code();
	if status == 401 {
		auth::on_unauthorized();
		return Outcome::Failed;
	}
	match response.get_string() {
		Ok(text) => parse(status, &text),
		Err(_) => Outcome::Failed,
	}
}

/// Called once at the top of each Source entry point, right after `auth::ensure_session`.
pub fn punch_if_due() {
	let now = current_date();
	if !claim_if_due(now) {
		return;
	}
	let outcome = send();
	record(&outcome, now);
}

/// Why the last attempt failed, until a later attempt gets an answer. Nothing while the
/// switch is off: the text says it will try again, which only holds while it is on.
pub fn last_error() -> Option<String> {
	if !enabled() {
		return None;
	}
	defaults_get::<String>(LAST_ERROR_KEY).filter(|value| !value.is_empty())
}

/// Empty strings rather than `DefaultValue::Null`: the test runner keeps a key that is set
/// to Null, and `timestamp`/`last_error` read "" as absent.
pub fn clear() {
	for key in [NEXT_KEY, LAST_ERROR_KEY] {
		set_string(key, "");
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn site_day_boundaries() {
		// 2026-09-30T03:40:39Z is 11:40 in UTC+8; that site day ends at 2026-09-30T16:00:00Z.
		assert_eq!(next_site_day(1_790_739_639), 1_790_784_000);
		// 2026-09-30T17:00:00Z is already 2026-10-01 in UTC+8.
		assert_eq!(next_site_day(1_790_787_600), 1_790_870_400);
	}

	#[aidoku_test]
	fn answers() {
		let ok = r#"{"code":200,"message":"success","data":{"res":{"status":"ok","punchInLastDay":"2026-11-08"}}}"#;
		assert!(matches!(parse(200, ok), Outcome::Done));
		let again = r#"{"code":200,"message":"success","data":{"res":{"status":"fail"}}}"#;
		assert!(matches!(parse(200, again), Outcome::AlreadyDone));
		assert!(matches!(parse(200, "{}"), Outcome::Failed));
		assert!(matches!(parse(200, "not json"), Outcome::Failed));
		assert!(matches!(parse(500, ok), Outcome::Failed));
	}

	#[aidoku_test]
	fn gate_moves_on_every_branch() {
		clear();
		auth::clear_auth();
		let now = 1_790_739_639;
		defaults_set(ENABLED_KEY, DefaultValue::Bool(true));
		// Signed out: nothing is due.
		assert!(!claim_if_due(now));
		defaults_set("auth_token", DefaultValue::String("t".into()));
		defaults_set(ENABLED_KEY, DefaultValue::Bool(false));
		assert!(!claim_if_due(now));
		defaults_set(ENABLED_KEY, DefaultValue::Bool(true));
		assert!(claim_if_due(now));
		// A second entry point at the same time finds the gate moved.
		assert!(!claim_if_due(now + 10));
		assert_eq!(timestamp(NEXT_KEY), now + RETRY);
		record(&Outcome::Failed, now);
		assert_eq!(timestamp(NEXT_KEY), now + RETRY);
		assert_eq!(last_error().as_deref(), Some(FAILED_TEXT));
		assert!(claim_if_due(now + RETRY));
		record(&Outcome::AlreadyDone, now + RETRY);
		assert_eq!(timestamp(NEXT_KEY), next_site_day(now + RETRY));
		assert!(last_error().is_none());
		record(&Outcome::Done, now);
		assert_eq!(timestamp(NEXT_KEY), next_site_day(now));
		clear();
		auth::clear_auth();
	}
}
