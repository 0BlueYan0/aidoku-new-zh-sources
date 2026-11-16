//! Proxy lines, taken from PicaWeb instead of being written into the source.
//!
//! PicaWeb (`picawang.com`) keeps its proxy domains in a constant of its front-end bundle
//! `shared-core-<hash>.js` (`const Ie=["go2778.com","acbbb.com"]` on 2026-11-16), and the
//! domains are temporary (issue #56). There is no API for the list, so `fetch_lines` reads
//! the home page, follows it to the bundle and takes the array of domains that is followed
//! by the `picaapi` host prefix.
//!
//! Each line is then tested the way PicaWeb does (`GET https://picaapi.<line>/status.json`,
//! HTTP 2xx with `code` 200). PicaWeb sorts the answering lines by speed and uses the
//! fastest; the source has no clock finer than a second, so it keeps the bundle's order
//! among the lines that answered.
//!
//! Two places refresh the list: the settings page every time it opens (the reader asked for
//! it), and `refresh_if_due` at the top of each Source entry point. That one is gated
//! (CLAUDE.md, 請求排程): the gate moves to `now + RETRY` before anything is sent, a refresh
//! that got the list or a line to answer moves it a day ahead, and any other outcome leaves
//! the one-hour retry.

use aidoku::{
	alloc::{string::ToString, String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
};
use serde::Deserialize;

use crate::{home::HOME_BATCH, settings};

const HOME_URL: &str = "https://picawang.com/";
/// PicaWeb's own list on 2026-11-16, used until a fetch has worked once.
pub const BUILT_IN: [&str; 2] = ["go2778.com", "acbbb.com"];

const LINES_KEY: &str = "proxy_lines";
const ANSWERED_KEY: &str = "proxy_lines_answered";
const NEXT_KEY: &str = "proxy_lines_next";
const RETRY: i64 = 3600;
const DAY: i64 = 86_400;
/// PicaWeb gives each status test 3 seconds.
const STATUS_TIMEOUT: f64 = 3.0;
const PAGE_TIMEOUT: f64 = 10.0;
/// How far after an array the `picaapi` prefix may appear for the array to count as the
/// line list. In the 2026-11-16 bundle it is 151 characters away.
const PREFIX_WINDOW: usize = 300;

/// A bare host name such as `go2778.com`: lowercase letters, digits, `-` and `.`, with at
/// least one dot and no empty label.
pub fn is_domain(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 253
		&& value.contains('.')
		&& value.split('.').all(|label| {
			!label.is_empty()
				&& !label.starts_with('-')
				&& !label.ends_with('-')
				&& label
					.bytes()
					.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		})
}

/// Path of the bundle that holds the line list, from the home page.
pub fn find_bundle(html: &str) -> Option<String> {
	const PREFIX: &str = "/assets/js/shared-core-";
	let start = html.find(PREFIX)?;
	let rest = &html[start + PREFIX.len()..];
	let hash_len = rest
		.bytes()
		.take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
		.count();
	if hash_len == 0 || !rest[hash_len..].starts_with(".js") {
		return None;
	}
	Some(format!("{PREFIX}{}.js", &rest[..hash_len]))
}

/// The array of quoted domains starting at `text[0]` (`[`), and the length it spans.
fn domain_array(text: &str) -> Option<(Vec<String>, usize)> {
	let bytes = text.as_bytes();
	let mut index = 1;
	let mut domains = Vec::new();
	loop {
		if bytes.get(index) != Some(&b'"') {
			return None;
		}
		let end = index + 1 + text[index + 1..].find('"')?;
		let domain = &text[index + 1..end];
		if !is_domain(domain) {
			return None;
		}
		domains.push(String::from(domain));
		index = end + 1;
		match bytes.get(index) {
			Some(b',') => index += 1,
			Some(b']') => return Some((domains, index + 1)),
			_ => return None,
		}
	}
}

/// The line list in the bundle: the first array made only of domains that has `picaapi`
/// within `PREFIX_WINDOW` characters after it. Other domain arrays in the bundle (the
/// `macapi` hosts) are not followed by it.
pub fn parse_lines(js: &str) -> Option<Vec<String>> {
	let mut from = 0;
	while let Some(offset) = js[from..].find('[') {
		let start = from + offset;
		if let Some((domains, len)) = domain_array(&js[start..]) {
			let after = &js[start + len..];
			let window = after
				.char_indices()
				.nth(PREFIX_WINDOW)
				.map_or(after, |(end, _)| &after[..end]);
			if window.contains("picaapi") {
				return Some(domains);
			}
		}
		from = start + 1;
	}
	None
}

#[derive(Deserialize)]
struct Status {
	code: i64,
}

/// Whether a `status.json` answer means the line works, as PicaWeb reads it.
pub fn status_ok(status: i32, text: &str) -> bool {
	(200..300).contains(&status)
		&& serde_json::from_str::<Status>(text).is_ok_and(|body| body.code == 200)
}

fn get_text(url: &str, timeout: f64) -> Option<String> {
	let response = Request::get(url).ok()?.timeout(timeout).send().ok()?;
	if !(200..300).contains(&response.status_code()) {
		return None;
	}
	response.get_string().ok()
}

/// Read the line list from PicaWeb. Two requests: the home page and the bundle.
fn fetch_lines() -> Option<Vec<String>> {
	let html = get_text(HOME_URL, PAGE_TIMEOUT)?;
	let bundle = find_bundle(&html)?;
	let js = get_text(&format!("https://picawang.com{bundle}"), PAGE_TIMEOUT)?;
	parse_lines(&js).filter(|lines| !lines.is_empty())
}

/// Test every line at once; `true` where `status.json` answered.
fn test_lines(lines: &[String]) -> Vec<bool> {
	let requests: Vec<Request> = lines
		.iter()
		.filter_map(|line| {
			Request::get(format!("https://picaapi.{line}/status.json"))
				.ok()
				.map(|request| request.timeout(STATUS_TIMEOUT))
		})
		.collect();
	if requests.len() != lines.len() {
		return lines.iter().map(|_| false).collect();
	}
	// The list comes from the site, so its length is not ours to cap; send it in batches.
	let mut responses = Vec::new();
	let mut requests = requests.into_iter().peekable();
	while requests.peek().is_some() {
		let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
		responses.extend(Request::send_all(batch));
	}
	responses
		.into_iter()
		.map(|result| {
			result.ok().is_some_and(|response| {
				let status = response.status_code();
				response
					.get_string()
					.is_ok_and(|text| status_ok(status, &text))
			})
		})
		.collect()
}

fn read_list(key: &str) -> Option<Vec<String>> {
	defaults_get::<String>(key)
		.filter(|value| !value.is_empty())
		.map(|value| value.split(',').map(String::from).collect())
}

fn write_list(key: &str, lines: &[String]) {
	defaults_set(key, DefaultValue::String(lines.join(",")));
}

/// The last list read from PicaWeb, or the built-in one.
pub fn known_lines() -> Vec<String> {
	read_list(LINES_KEY)
		.unwrap_or_else(|| BUILT_IN.iter().map(|line| String::from(*line)).collect())
}

/// Lines that answered the last test that reached any line, in list order.
fn answered_lines() -> Option<Vec<String>> {
	read_list(ANSWERED_KEY)
}

/// Whether a line the reader picked can still be used: it answered the last test, or no
/// test has reached a line yet and it is on the known list.
pub fn usable(line: &str) -> bool {
	match answered_lines() {
		Some(answered) => answered.iter().any(|l| l == line),
		None => known_lines().iter().any(|l| l == line),
	}
}

/// The line used when the reader has not picked one, or picked one that cannot be used:
/// the first line that answered, else the first known line.
pub fn best_line() -> String {
	answered_lines()
		.and_then(|answered| answered.into_iter().next())
		.or_else(|| known_lines().into_iter().next())
		.unwrap_or_else(|| String::from(settings::DEFAULT_LINE))
}

pub struct LineState {
	pub line: String,
	pub answered: bool,
}

pub struct Refresh {
	/// The list was read from PicaWeb (not the stored or built-in one).
	pub fetched: bool,
	pub lines: Vec<LineState>,
}

/// Fetch the list and test it, keeping what was learnt. A fetch that fails keeps the
/// stored list; a test where no line answered (offline) keeps the last answers.
pub fn refresh() -> Refresh {
	let fetched = fetch_lines();
	let is_fetched = fetched.is_some();
	let lines = fetched.unwrap_or_else(known_lines);
	if is_fetched {
		write_list(LINES_KEY, &lines);
	}
	let results = test_lines(&lines);
	let answered: Vec<String> = lines
		.iter()
		.zip(&results)
		.filter(|(_, ok)| **ok)
		.map(|(line, _)| line.clone())
		.collect();
	if !answered.is_empty() {
		write_list(ANSWERED_KEY, &answered);
	}
	Refresh {
		fetched: is_fetched,
		lines: lines
			.into_iter()
			.zip(results)
			.map(|(line, answered)| LineState { line, answered })
			.collect(),
	}
}

fn next_due() -> i64 {
	defaults_get::<String>(NEXT_KEY)
		.and_then(|value| value.parse::<i64>().ok())
		.unwrap_or(0)
}

fn set_next_due(value: i64) {
	defaults_set(NEXT_KEY, DefaultValue::String(value.to_string()));
}

/// Move the gate after a refresh: a day ahead when it learnt something, else leave the
/// retry that `claim_if_due` set.
pub fn record(refresh: &Refresh, now: i64) {
	if refresh.fetched || refresh.lines.iter().any(|state| state.answered) {
		set_next_due(now + DAY);
	}
}

/// Whether the daily refresh should run now. Moves the gate to `now + RETRY` first so
/// entry points running together send it once. Readers on the official line or on their
/// own domain do not use the list, so nothing is due for them.
fn claim_if_due(now: i64) -> bool {
	if !settings::uses_proxy_list() || now < next_due() {
		return false;
	}
	set_next_due(now + RETRY);
	true
}

/// Called once at the top of each Source entry point, before `auth::ensure_session` so the
/// sign-in goes to the refreshed line.
pub fn refresh_if_due() {
	let now = current_date();
	if !claim_if_due(now) {
		return;
	}
	let result = refresh();
	record(&result, now);
}

/// Called after the settings page refreshed the list itself.
pub fn record_now(refresh: &Refresh) {
	set_next_due(current_date() + RETRY);
	record(refresh, current_date());
}

/// Empty strings rather than `DefaultValue::Null`, see `punch::clear`.
#[cfg(test)]
pub fn clear() {
	for key in [LINES_KEY, ANSWERED_KEY, NEXT_KEY] {
		defaults_set(key, DefaultValue::String(String::new()));
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::alloc::vec;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn domains() {
		assert!(is_domain("go2778.com"));
		assert!(is_domain("macapi3.top"));
		assert!(!is_domain("localhost"));
		assert!(!is_domain("Go2778.com"));
		assert!(!is_domain("a..com"));
		assert!(!is_domain("-a.com"));
		assert!(!is_domain("a.com/x"));
		assert!(!is_domain("${t}.${a}"));
	}

	#[aidoku_test]
	fn bundle_path() {
		let html = r#"<script type="module" src="/assets/js/index-ea_4Germ.js"></script><link rel="modulepreload" crossorigin href="/assets/js/shared-core-D9Sg6H3u.js">"#;
		assert_eq!(
			find_bundle(html).as_deref(),
			Some("/assets/js/shared-core-D9Sg6H3u.js")
		);
		assert_eq!(find_bundle("<html></html>"), None);
		assert_eq!(find_bundle("/assets/js/shared-core-.js"), None);
		assert_eq!(find_bundle("/assets/js/shared-core-abc\"x"), None);
	}

	#[aidoku_test]
	fn line_array() {
		// The shape of the 2026-11-16 bundle, with the macapi array placed first and more than
		// `PREFIX_WINDOW` characters away from `picaapi`, as it is in the real bundle.
		let js = format!(
			r#"Zt=Object.freeze(["macapi1.com","macapi2.com","macapi3.top"]),Xt={};const Ie=["go2778.com","acbbb.com"];let xe=!1;const Oe=async(e,t=3e3,a="picaapi")=>{{}}"#,
			"x".repeat(PREFIX_WINDOW)
		);
		let js = js.as_str();
		assert_eq!(
			parse_lines(js),
			Some(vec![String::from("go2778.com"), String::from("acbbb.com")])
		);
		// A single-domain array without the prefix nearby is not the list.
		assert_eq!(parse_lines(r#"x=["yyai.vip"];y=1"#), None);
		// Arrays with anything but domains are skipped.
		assert_eq!(parse_lines(r#"x=["a b","c.com"];"picaapi""#), None);
		assert_eq!(parse_lines(r#"x=[1,2];"picaapi""#), None);
	}

	#[aidoku_test]
	fn status_answers() {
		assert!(status_ok(200, r#"{"code":200,"message":"ok"}"#));
		assert!(!status_ok(200, r#"{"code":500}"#));
		assert!(!status_ok(200, "<html>"));
		assert!(!status_ok(502, r#"{"code":200}"#));
	}

	#[aidoku_test]
	fn picks_answered_line() {
		clear();
		assert_eq!(best_line(), "go2778.com");
		assert!(usable("acbbb.com"));
		assert!(!usable("other.com"));
		write_list(ANSWERED_KEY, &[String::from("acbbb.com")]);
		assert_eq!(best_line(), "acbbb.com");
		assert!(!usable("go2778.com"));
		write_list(LINES_KEY, &[String::from("new.com")]);
		defaults_set(ANSWERED_KEY, DefaultValue::String(String::new()));
		assert_eq!(best_line(), "new.com");
		assert!(usable("new.com"));
		assert!(!usable("go2778.com"));
		clear();
	}

	#[aidoku_test]
	fn gate_moves_on_every_branch() {
		clear();
		settings::clear_line_choice();
		let now = 1_790_739_639;
		assert!(claim_if_due(now));
		// A second entry point at the same time finds the gate moved.
		assert!(!claim_if_due(now + 10));
		// Nothing learnt: the one-hour retry stays.
		record(
			&Refresh {
				fetched: false,
				lines: Vec::new(),
			},
			now,
		);
		assert!(!claim_if_due(now + RETRY - 1));
		assert!(claim_if_due(now + RETRY));
		// A line answered: a day ahead.
		record(
			&Refresh {
				fetched: false,
				lines: vec![LineState {
					line: String::from("go2778.com"),
					answered: true,
				}],
			},
			now + RETRY,
		);
		assert!(!claim_if_due(now + RETRY + DAY - 1));
		assert!(claim_if_due(now + RETRY + DAY));
		// Official line: nothing is due.
		clear();
		defaults_set("api_line", DefaultValue::String(String::from("official")));
		assert!(!claim_if_due(now));
		settings::clear_line_choice();
		clear();
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/lines.rs"]
mod fixture_tests;
