//! SignalR client for the site's API hub, over HTTP long polling with the JSON protocol.
//!
//! The site talks to `/hub/api` over a WebSocket, which a source cannot open. The hub
//! also accepts long polling (negotiate, then POST messages and GET to receive them),
//! and `{"UseGzip": false}` as the second invocation argument makes it answer in plain
//! JSON instead of gzipped bytes.
//!
//! Measured behaviour (2026-09-29) the code depends on:
//! - A connection that goes about 15 seconds without a poll is dropped, and so is one
//!   now and then for no visible reason; both answer `404 No Connection with that ID`.
//!   Every caller therefore opens one connection, sends all its invocations at once and
//!   polls until they complete, and a 404 reopens the connection once.
//! - Two polls on one connection at the same time cancel the first, so polls are
//!   strictly sequential.
//! - The bearer token has to be on the transport requests (polls and sends), as the
//!   site's SignalR client does. An access token lives 30 seconds, but a connection
//!   opened with a valid one kept working at 48 seconds with the expired token still
//!   on its polls.

use aidoku::{
	alloc::{format, string::ToString, String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
		std::current_date,
	},
	prelude::*,
	Result,
};
use core::sync::atomic::{AtomicU32, Ordering};
use serde_json::Value;

pub const API_LINE_KEY: &str = "api_line";

/// The API host of the line picked in settings. The site's own page offers the same two
/// (`LightNovelShelf_Api_Server_V7`). On 2026-09-30 one of the two addresses behind
/// api.lightnovel.life stopped answering and every request from the phone timed out,
/// while cf-api.lightnovel.life kept working, so Cloudflare is the default.
pub fn api_base() -> &'static str {
	match defaults_get::<String>(API_LINE_KEY).as_deref() {
		Some("hk") => "https://api.lightnovel.life",
		_ => "https://cf-api.lightnovel.life",
	}
}
/// SignalR record separator between messages.
const RS: char = '\u{1e}';
/// Completions arrive within seconds; a poll that waits this long is stuck.
const TIMEOUT: f64 = 15.0;
/// Polls allowed while waiting for the completions, beyond one per invocation.
const EXTRA_POLLS: usize = 6;

/// What one invocation came back with.
pub enum Reply {
	/// The hub method ran and reported success; the `response` field.
	Ok(Value),
	/// The method ran but refused, with the site's status and message
	/// (for example 403 when the account level is too low).
	Refused(i64, String),
	/// The hub rejected the call itself, for example `user is unauthorized`.
	Error(String),
}

impl Reply {
	/// The app shows a generic message for an `Err`, so the site's answer is printed
	/// here too; without it a refusal (402 quota, 403 level) leaves no trace in logcat.
	pub fn into_result(self) -> Result<Value> {
		match self {
			Reply::Ok(value) => Ok(value),
			Reply::Refused(status, msg) => {
				println!("[lightnovel] refused {status}: {msg}");
				bail!("[lightnovel] refused {status}: {msg}")
			}
			Reply::Error(msg) => {
				println!("[lightnovel] hub error: {msg}");
				bail!("[lightnovel] hub error: {msg}")
			}
		}
	}
}

/// One hub call: the method name and its first argument, as a JSON value.
#[derive(Clone)]
pub struct Call {
	pub target: &'static str,
	pub args: Value,
}

impl Call {
	pub fn new(target: &'static str, args: Value) -> Self {
		Self { target, args }
	}
}

enum Failure {
	/// The connection is gone (404) or a poll got no answer; worth one retry on a new
	/// connection.
	Lost,
	Other(String),
}

struct Connection {
	url: String,
	token: Option<String>,
}

static SEQUENCE: AtomicU32 = AtomicU32::new(0);
static BASE: AtomicU32 = AtomicU32::new(0);
const SEQUENCE_KEY: &str = "hub_sequence";
/// Requests one connection may make after `persist_sequence` before a later instance,
/// starting from the persisted number plus this, could repeat one of them.
const SEQUENCE_GAP: u64 = 1000;

/// Where this instance's numbers start: the persisted counter plus the gap, read once.
fn base() -> u64 {
	let base = BASE.load(Ordering::Relaxed);
	if base != 0 {
		return u64::from(base);
	}
	let stored = defaults_get::<String>(SEQUENCE_KEY)
		.and_then(|v: String| v.parse::<u64>().ok())
		.unwrap_or(0);
	let base = (stored.wrapping_add(SEQUENCE_GAP) as u32).max(1);
	BASE.store(base, Ordering::Relaxed);
	u64::from(base)
}

/// A number no earlier request used. The in-memory counter restarts with every new
/// instance of the source, so it is offset by a counter kept in defaults, which
/// `Connection::open` persists once per connection rather than once per request.
fn next_sequence() -> String {
	let n = base() + u64::from(SEQUENCE.fetch_add(1, Ordering::Relaxed));
	format!("{}-{n}", current_date())
}

fn persist_sequence() {
	let n = base() + u64::from(SEQUENCE.load(Ordering::Relaxed));
	defaults_set(SEQUENCE_KEY, DefaultValue::String(format!("{n}")));
}

fn request(method_post: bool, url: &str, body: Option<&str>, token: Option<&str>) -> core::result::Result<String, Failure> {
	// Polls repeat the same URL, and a repeated URL can be answered from a cache (the
	// test runner handed a second negotiate the first one's connection id; a shared
	// connection would mix signed-in and signed-out callers), so every request gets a
	// unique parameter. The hub ignores it.
	let sep = if url.contains('?') { '&' } else { '?' };
	let url = format!("{url}{sep}_={}", next_sequence());
	let url = url.as_str();
	let req = if method_post { Request::post(url) } else { Request::get(url) };
	let mut req = req.map_err(|_| Failure::Other("bad url".to_string()))?.timeout(TIMEOUT);
	if let Some(token) = token {
		req = req.header("Authorization", format!("Bearer {token}").as_str());
	}
	if let Some(body) = body {
		req = req.header("Content-Type", "text/plain;charset=UTF-8").body(body.as_bytes());
	}
	let response = req.send().map_err(|e| {
		println!("[lightnovel] {} {url} failed: {e:?}", if method_post { "POST" } else { "GET" });
		// A poll now and then never gets its answer; a new connection usually does.
		if method_post {
			Failure::Other("request failed".to_string())
		} else {
			Failure::Lost
		}
	})?;
	let status = response.status_code();
	let text = response.get_string().unwrap_or_default();
	match status {
		200 | 204 => Ok(text),
		404 => Err(Failure::Lost),
		_ => Err(Failure::Other(format!("HTTP {status}: {text}"))),
	}
}

impl Connection {
	fn open(token: Option<&str>) -> core::result::Result<Self, Failure> {
		persist_sequence();
		let negotiated = request(true, &format!("{}/hub/api/negotiate?negotiateVersion=1", api_base()), None, token)?;
		let id = serde_json::from_str::<Value>(&negotiated)
			.ok()
			.and_then(|v: Value| v.get("connectionToken").and_then(Value::as_str).map(String::from))
			.ok_or_else(|| Failure::Other("negotiate without connectionToken".to_string()))?;
		println!("[lightnovel] hub connection {id} ({})", if token.is_some() { "signed in" } else { "anonymous" });
		let conn = Self {
			url: format!("{}/hub/api?id={id}", api_base()),
			token: token.map(String::from),
		};
		// The first poll returns at once and starts the transport.
		conn.poll()?;
		conn.send(&format!(r#"{{"protocol":"json","version":1}}{RS}"#))?;
		// Handshake reply `{}`, often followed by an `OnMessage` announcement.
		conn.poll()?;
		Ok(conn)
	}

	// The hub takes the user from the transport requests, not from negotiate: with the
	// token on negotiate only, calls came back unauthorized (device and Python,
	// 2026-09-29). The site's own SignalR client sends it on every request.
	fn poll(&self) -> core::result::Result<String, Failure> {
		request(false, &self.url, None, self.token.as_deref())
	}

	fn send(&self, body: &str) -> core::result::Result<String, Failure> {
		request(true, &self.url, Some(body), self.token.as_deref())
	}

	fn invoke(&self, calls: &[Call]) -> core::result::Result<Vec<Reply>, Failure> {
		let mut body = String::new();
		for (i, call) in calls.iter().enumerate() {
			let message = serde_json::json!({
				"type": 1,
				"invocationId": format!("{i}"),
				"target": call.target,
				"arguments": [call.args, {"UseGzip": false}],
			});
			body.push_str(&message.to_string());
			body.push(RS);
		}
		self.send(&body)?;

		let mut replies: Vec<Option<Reply>> = (0..calls.len()).map(|_| None).collect();
		let mut pending = calls.len();
		let mut buffer = String::new();
		for _ in 0..calls.len() + EXTRA_POLLS {
			if pending == 0 {
				break;
			}
			buffer.push_str(&self.poll()?);
			// Keep a trailing partial message for the next poll.
			let complete_end = buffer.rfind(RS).map(|i| i + RS.len_utf8()).unwrap_or(0);
			let rest = buffer.split_off(complete_end);
			for frame in buffer.split(RS).filter(|f: &&str| !f.is_empty()) {
				if let Some((index, reply)) = parse_completion(frame) {
					if let Some(slot) = replies.get_mut(index) {
						if slot.is_none() {
							pending -= 1;
						}
						*slot = Some(reply);
					}
				}
			}
			buffer = rest;
		}
		if pending > 0 {
			return Err(Failure::Other(format!("{pending} invocations never completed")));
		}
		Ok(replies.into_iter().map(|r: Option<Reply>| r.unwrap_or(Reply::Error(String::new()))).collect())
	}
}

/// A completion message (`type` 3) as (invocation index, reply). Other messages, such
/// as the `OnMessage` announcements the hub pushes, are ignored.
fn parse_completion(frame: &str) -> Option<(usize, Reply)> {
	let value: Value = serde_json::from_str(frame).ok()?;
	if value.get("type").and_then(Value::as_i64) != Some(3) {
		return None;
	}
	let index = value.get("invocationId")?.as_str()?.parse::<usize>().ok()?;
	if let Some(error) = value.get("error").and_then(Value::as_str) {
		return Some((index, Reply::Error(error.to_string())));
	}
	let result = value.get("result")?;
	let success = result.get("success").and_then(Value::as_bool).unwrap_or(false);
	if success {
		let response = result.get("response").cloned().unwrap_or(Value::Null);
		Some((index, Reply::Ok(response)))
	} else {
		let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
		let msg = result.get("msg").and_then(Value::as_str).unwrap_or("").to_string();
		Some((index, Reply::Refused(status, msg)))
	}
}

/// Run `calls` on one fresh connection and return their replies in order.
/// The connection is used for this batch only.
pub fn invoke_all(token: Option<&str>, calls: &[Call]) -> Result<Vec<Reply>> {
	let mut last = String::new();
	for attempt in 0..2 {
		let outcome = Connection::open(token).and_then(|conn: Connection| conn.invoke(calls));
		match outcome {
			Ok(replies) => return Ok(replies),
			Err(Failure::Lost) => {
				println!("[lightnovel] hub connection lost (attempt {attempt})");
				last = "connection lost".to_string();
			}
			Err(Failure::Other(msg)) => bail!("[lightnovel] hub: {msg}"),
		}
	}
	bail!("[lightnovel] hub: {last}")
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn parses_completion_frames() {
		let ok = r#"{"type":3,"invocationId":"2","result":{"response":{"a":1},"status":200,"success":true,"msg":""}}"#;
		match parse_completion(ok) {
			Some((2, Reply::Ok(v))) => assert_eq!(v["a"], 1),
			_ => panic!("ok frame"),
		}
		let refused = r#"{"type":3,"invocationId":"0","result":{"response":null,"status":403,"success":false,"msg":"x"}}"#;
		assert!(matches!(parse_completion(refused), Some((0, Reply::Refused(403, _)))));
		let error = r#"{"type":3,"invocationId":"1","error":"Failed to invoke 'GetBookInfo' because user is unauthorized"}"#;
		assert!(matches!(parse_completion(error), Some((1, Reply::Error(_)))));
		let push = r#"{"type":1,"target":"OnMessage","arguments":["hi"]}"#;
		assert!(parse_completion(push).is_none());
	}

	#[aidoku_test]
	fn invokes_over_long_polling() {
		// The one method that works signed out.
		let replies = invoke_all(
			None,
			&[
				Call::new("GetLatestBookList", serde_json::json!({ "Page": 1, "Size": 6 })),
				Call::new("GetBookInfo", serde_json::json!({ "Id": 19367 })),
			],
		)
		.expect("hub");
		match &replies[0] {
			Reply::Ok(value) => assert!(value["data"].as_array().is_some_and(|a: &Vec<Value>| !a.is_empty())),
			_ => panic!("latest list"),
		}
		assert!(matches!(&replies[1], Reply::Error(msg) if msg.contains("unauthorized")));
	}
}
