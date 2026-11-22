use aidoku::{
	alloc::{string::ToString, String, Vec},
	imports::{defaults::defaults_get, net::Request},
	prelude::{bail, format},
	Result,
};
use serde::de::DeserializeOwned;

use crate::{auth, USER_AGENT};

const LOGIN_REQUIRED: &[u8] = br#"{"status":"login"}"#;

pub fn format_names(raw: &str) -> Option<String> {
	if raw.is_empty() {
		return None;
	}
	let formatted: String = raw
		.split(' ')
		.filter(|s| !s.is_empty())
		.map(|segment| {
			segment
				.split('-')
				.map(|word| {
					let mut chars = word.chars();
					match chars.next() {
						Some(c) => {
							let upper: String = c.to_uppercase().collect();
							let rest: String = chars.collect();
							let mut out = upper;
							out.push_str(&rest);
							out
						}
						None => String::new(),
					}
				})
				.collect::<Vec<String>>()
				.join("-")
		})
		.collect::<Vec<String>>()
		.join(", ");
	Some(formatted)
}

pub fn split_tags(raw: &str) -> Vec<String> {
	if raw.is_empty() {
		return Vec::new();
	}
	raw.split(' ')
		.filter(|s| !s.is_empty())
		.map(|s| s.trim().into())
		.collect()
}

pub fn parse_chapter_name(name: String) -> (Option<f32>, Option<f32>, Option<String>) {
	let (machine, rest) = match name.strip_prefix("機翻") {
		Some(rest) => (true, rest),
		None => (false, name.as_str()),
	};
	let rest = rest.strip_prefix('第').unwrap_or(rest);
	let digits = rest
		.find(|c: char| !c.is_ascii_digit() && c != '.')
		.unwrap_or(rest.len());
	let (number, rest) = rest.split_at(digits);
	let Ok(number) = number.parse::<f32>() else {
		return (None, None, Some(name));
	};
	let (is_range, rest) = match rest.strip_prefix('-') {
		Some(rest) => (true, rest.trim_start_matches(|c: char| c.is_ascii_digit())),
		None => (false, rest),
	};
	let mut chars = rest.chars();
	let is_volume = match chars.next() {
		Some('話' | '话' | '回') => false,
		Some('卷' | '冊') => true,
		None if name.starts_with('第') => false,
		_ => return (None, None, Some(name)),
	};
	// A range (`第1-3話`) is not one chapter: no number, so it cannot share a number with a
	// real chapter (repo rule from issue #58, after the official zh.jmcomic).
	if is_range {
		return (None, None, Some(name));
	}
	let (chapter, volume) = if is_volume {
		(None, Some(number))
	} else {
		(Some(number), None)
	};
	let suffix = chars.as_str().trim();
	let title = match (machine, suffix.is_empty()) {
		(false, true) => None,
		(false, false) => Some(suffix.into()),
		(true, true) => Some("機翻".into()),
		(true, false) => Some(format!("機翻 {suffix}")),
	};
	(chapter, volume, title)
}

pub fn adult_mode(selected: &[String]) -> &'static str {
	let has = |value: &str| selected.iter().any(|s| s == value);
	match (has("false"), has("true")) {
		(true, true) => "both",
		(false, true) => "true",
		_ => "false",
	}
}

pub fn get_adult_mode() -> &'static str {
	adult_mode(&defaults_get::<Vec<String>>("adult_mode").unwrap_or_default())
}

pub fn extract_manga_id(url: &str) -> Option<String> {
	let marker = "/manga/";
	let (_, tail) = url.split_once(marker)?;
	let id = tail
		.split(['/', '?', '#'])
		.next()
		.filter(|s| !s.is_empty())?;
	Some(id.into())
}

pub fn extract_reader_path(url: &str) -> Option<String> {
	let marker = "/reader/";
	let (_, tail) = url.split_once(marker)?;
	let path = tail.split(['?', '#']).next().filter(|s| !s.is_empty())?;
	Some(path.into())
}

pub fn get_server_domain() -> String {
	defaults_get::<String>("server")
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| "noymanga.com".to_string())
}

fn get_base_url() -> String {
	format!("https://api.{}", get_server_domain())
}

pub fn get_img_base() -> String {
	format!("https://img.{}", get_server_domain())
}

pub fn api_get(path: &str, adult: &str) -> Result<Request> {
	Ok(Request::get(format!("{}{path}", get_base_url()))?
		.header("User-Agent", USER_AGENT)
		.header("allow-adult", adult))
}

pub fn api_post(path: &str, body: &str, adult: &str) -> Result<Request> {
	Ok(Request::post(format!("{}{path}", get_base_url()))?
		.header("User-Agent", USER_AGENT)
		.header("Content-Type", "application/x-www-form-urlencoded")
		.header("allow-adult", adult)
		.body(body))
}

/// One request, one answer. A `{"status":"login"}` answer only marks the session dead; the
/// next entry point's `auth::ensure_session` signs in again. Signing in here would send a
/// second request from inside the content path, once per entry point running at that time.
pub fn fetch_json<T: DeserializeOwned>(request: impl FnOnce() -> Result<Request>) -> Result<T> {
	let data = request()?.data()?;
	if data == LOGIN_REQUIRED {
		auth::mark_expired();
		bail!("[noyacg2] the site asked for a login");
	}
	Ok(serde_json::from_slice(&data)?)
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn chapter_names() {
		assert_eq!(
			parse_chapter_name("第12話 標題".into()),
			(Some(12.0), None, Some("標題".into()))
		);
		assert_eq!(parse_chapter_name("第3卷".into()), (None, Some(3.0), None));
		assert_eq!(
			parse_chapter_name("機翻第01回".into()),
			(Some(1.0), None, Some("機翻".into()))
		);
		// a range is not one chapter
		assert_eq!(
			parse_chapter_name("第1-3話".into()),
			(None, None, Some("第1-3話".into()))
		);
		assert_eq!(
			parse_chapter_name("番外篇".into()),
			(None, None, Some("番外篇".into()))
		);
	}
}
