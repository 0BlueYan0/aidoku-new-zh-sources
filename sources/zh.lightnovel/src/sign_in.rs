//! Daily sign-in on the site, done for the reader while the `autoSignIn` switch is on.
//!
//! The `SignIn` call rides in the first signed-in hub batch of the day (see
//! `invoke_signed_in`), so it opens no connection of its own. A gate in defaults keeps
//! it to one attempt per site day, and every branch moves the gate (CLAUDE.md, 請求排程):
//! the window is claimed before the batch is sent, a success or a refusal pushes it to
//! the next site day, anything else leaves the one-hour retry. The site's day is taken
//! to start at midnight UTC+8; if that is wrong, the cost is one refused call a day.
//!
//! Entry points run at the same time (13 connections in 5 s after a login, 2026-09-30),
//! so two of them can both find the gate open and both send `SignIn`. The second is
//! refused; `signed_on` keeps that refusal from being shown as a failure.

use aidoku::{alloc::String, imports::defaults::defaults_get, prelude::*};
use serde_json::json;

use crate::auth::{get_string, set_string, set_timestamp, timestamp};
use crate::hub::{Call, Reply};

pub const ENABLED_KEY: &str = "autoSignIn";
const NEXT_KEY: &str = "sign_in_next";
const SIGNED_ON_KEY: &str = "sign_in_signed_on";
const LAST_ERROR_KEY: &str = "sign_in_last_error";
/// Claimed before an attempt is sent, and what a hub error leaves in place.
const RETRY: i64 = 3600;
const DAY: i64 = 86_400;
const SITE_UTC_OFFSET: i64 = 8 * 3600;

pub fn enabled() -> bool {
	defaults_get::<bool>(ENABLED_KEY).unwrap_or(false)
}

/// Days since the epoch, counted in the site's time zone.
fn site_day(now: i64) -> i64 {
	(now + SITE_UTC_OFFSET).div_euclid(DAY)
}

/// First second of the site day after the one holding `now`.
pub fn next_site_day(now: i64) -> i64 {
	(site_day(now) + 1) * DAY - SITE_UTC_OFFSET
}

pub fn call() -> Call {
	Call::new("SignIn", json!({}))
}

/// Whether this batch should carry `SignIn`. When it should, the gate moves to
/// `now + RETRY` here, before anything is sent, so a batch that never completes still
/// leaves the gate ahead.
pub fn claim_if_due(now: i64) -> bool {
	if !enabled() || now < timestamp(NEXT_KEY) {
		return false;
	}
	set_timestamp(NEXT_KEY, now + RETRY);
	println!("[lightnovel] sign in: attempting, gate -> {}", now + RETRY);
	true
}

/// What the site answered to `SignIn`. The raw reply is printed because the reference
/// doc gives only the success fields; the shape of "already signed today" is unknown.
pub fn record(reply: &Reply, now: i64) {
	match reply {
		Reply::Ok(value) => {
			println!("[lightnovel] sign in: ok {value}, gate -> {}", next_site_day(now));
			set_timestamp(NEXT_KEY, next_site_day(now));
			set_timestamp(SIGNED_ON_KEY, site_day(now));
			set_string(LAST_ERROR_KEY, "");
		}
		Reply::Refused(status, msg) => {
			println!("[lightnovel] sign in: refused {status}: {msg}, gate -> {}", next_site_day(now));
			set_timestamp(NEXT_KEY, next_site_day(now));
			// The site answers `500: 今日已签到` when the day is already signed (device,
			// 2026-10-01), e.g. after a logout cleared `signed_on`. Nothing to act on.
			if msg.contains("已签到") || msg.contains("已簽到") {
				set_timestamp(SIGNED_ON_KEY, site_day(now));
				set_string(LAST_ERROR_KEY, "");
				return;
			}
			// A refusal after (or racing) today's success is the duplicate a concurrent
			// entry point sent, not something the reader has to act on.
			if timestamp(SIGNED_ON_KEY) != site_day(now) {
				set_string(LAST_ERROR_KEY, msg);
			}
		}
		Reply::Error(msg) => {
			println!("[lightnovel] sign in: hub error {msg}, retrying in an hour");
		}
	}
}

/// The site's message from the last refused attempt, until a later attempt succeeds.
pub fn last_error() -> Option<String> {
	get_string(LAST_ERROR_KEY)
}

/// Empty strings rather than `DefaultValue::Null`: the test runner keeps a key that is
/// set to Null (seen 2026-09-30), and `get_string`/`timestamp` read "" as absent anyway.
pub fn clear() {
	for key in [NEXT_KEY, SIGNED_ON_KEY, LAST_ERROR_KEY] {
		set_string(key, "");
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku::imports::defaults::{defaults_set, DefaultValue};
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn site_day_boundaries() {
		// 2026-09-30T03:40:39Z is 11:40 in UTC+8; that site day ends at 2026-09-30T16:00:00Z.
		assert_eq!(next_site_day(1_790_739_639), 1_790_784_000);
		// 2026-09-30T17:00:00Z is already 2026-10-01 in UTC+8.
		assert_eq!(next_site_day(1_790_787_600), 1_790_870_400);
	}

	#[aidoku_test]
	fn gate_moves_on_every_branch() {
		clear();
		let now = 1_790_739_639;
		defaults_set(ENABLED_KEY, DefaultValue::Bool(false));
		assert!(!claim_if_due(now));
		defaults_set(ENABLED_KEY, DefaultValue::Bool(true));
		assert!(claim_if_due(now));
		assert!(!claim_if_due(now + 10));
		assert_eq!(timestamp(NEXT_KEY), now + RETRY);
		record(&Reply::Error(String::from("no completion")), now);
		assert_eq!(timestamp(NEXT_KEY), now + RETRY);
		// A real refusal is shown until a later success.
		record(&Reply::Refused(403, String::from("账号已封禁")), now);
		assert_eq!(last_error().as_deref(), Some("账号已封禁"));
		record(&Reply::Ok(json!({ "reward": 6, "streak": 2 })), now);
		assert!(last_error().is_none());
		assert_eq!(timestamp(NEXT_KEY), next_site_day(now));
		// A refusal after the success is the duplicate, not a failure.
		record(&Reply::Refused(403, String::from("账号已封禁")), now + 5);
		assert!(last_error().is_none());
		// "Already signed" with no success on record (after a logout) is not a failure.
		clear();
		record(&Reply::Refused(500, String::from("今日已签到")), now);
		assert!(last_error().is_none());
		assert_eq!(timestamp(NEXT_KEY), next_site_day(now));
		assert!(!claim_if_due(now + 3 * 3600));
		assert!(claim_if_due(next_site_day(now)));
		defaults_set(ENABLED_KEY, DefaultValue::Bool(false));
		clear();
	}
}
