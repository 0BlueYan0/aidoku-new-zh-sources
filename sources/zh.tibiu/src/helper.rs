use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::parse_date},
	prelude::*,
	Chapter, ContentRating, Manga, MangaStatus, Result, Viewer,
};
use core::cmp::Ordering;

pub const BASE_URL: &str = "https://comic.tibiu.net";
pub const API_URL: &str = "https://comic.tibiu.net/index.php/api";
pub const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------

/// Fetch a JSON endpoint and return the raw response body.
///
/// Every request goes out as a guest. That is enough for everything this source shows:
/// listings, search, details, chapter lists and images are all served without a session.
pub fn fetch_json(url: &str) -> Result<String> {
	Request::get(url)?
		.header("User-Agent", USER_AGENT)
		.header("Referer", BASE_URL)
		.header("Accept", "application/json, text/javascript, */*; q=0.01")
		.header("X-Requested-With", "XMLHttpRequest")
		.string()
}

// ---------------------------------------------------------------------------
// Hand-rolled JSON scanning (no serde: keeps the wasm binary small)
// ---------------------------------------------------------------------------

/// Extract a JSON string value: `"key":"value"` → `value` (still escaped).
pub fn json_str_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
	let search = format!("\"{}\":\"", key);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];
	// Handle escaped quotes
	let mut end = 0;
	let bytes = rest.as_bytes();
	while end < bytes.len() {
		if bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\') {
			break;
		}
		end += 1;
	}
	Some(&rest[..end])
}

/// Extract a JSON number value: `"key":123` → `123`.
///
/// Tolerates the value being quoted (`"key":"123"`), which this API does inconsistently:
/// the listing endpoints return numbers as strings while `comic/detail` returns them raw.
/// The digits have to start where the value starts: a non-numeric value (`""`, `null`,
/// `true`) yields `None` instead of borrowing the next number further along in the body.
pub fn json_num_value(json: &str, key: &str) -> Option<i64> {
	let search = format!("\"{}\":", key);
	let pos = json.find(&search)?;
	let rest = json[pos + search.len()..].trim_start();
	let rest = rest.strip_prefix('"').unwrap_or(rest);

	let end = rest
		.char_indices()
		.find(|&(i, ch): &(usize, char)| !(ch.is_ascii_digit() || (i == 0 && ch == '-')))
		.map_or(rest.len(), |(i, _): (usize, char)| i);

	rest[..end].parse::<i64>().ok()
}

/// Tracks whether a scan position sits inside a JSON string literal.
///
/// Every depth-counting scan below has to ignore brackets and braces that appear inside
/// string values. Synopses on this site open a bracketed pull quote and the truncated
/// `text` field can cut the closing bracket off, so the counts really do go unbalanced.
#[derive(Default)]
struct StringScanner {
	in_string: bool,
	escaped: bool,
}

impl StringScanner {
	/// Feed the next character; returns true only if it is structural punctuation,
	/// meaning it sits outside any string literal.
	fn is_structural(&mut self, ch: char) -> bool {
		if self.in_string {
			if self.escaped {
				self.escaped = false;
			} else if ch == '\\' {
				self.escaped = true;
			} else if ch == '"' {
				self.in_string = false;
			}
			return false;
		}

		if ch == '"' {
			self.in_string = true;
			return false;
		}

		true
	}
}

/// Iterate over JSON array objects.
/// Given `"key":[{...},{...}]`, returns a Vec of the individual `{...}` strings.
pub fn json_array_objects<'a>(json: &'a str, key: &str) -> Vec<&'a str> {
	let mut results: Vec<&str> = Vec::new();
	let search = format!("\"{}\":[", key);
	let pos = match json.find(&search) {
		Some(p) => p + search.len(),
		None => return results,
	};

	let body = &json[pos..];
	let mut scanner = StringScanner::default();
	let mut depth = 0i32;
	let mut obj_start: Option<usize> = None;

	for (i, ch) in body.char_indices() {
		if !scanner.is_structural(ch) {
			continue;
		}
		match ch {
			'{' => {
				if depth == 0 {
					obj_start = Some(i);
				}
				depth += 1;
			}
			'}' => {
				depth -= 1;
				if depth == 0 {
					if let Some(start) = obj_start {
						results.push(&body[start..=i]);
					}
					obj_start = None;
				}
			}
			']' if depth == 0 => break,
			_ => {}
		}
	}

	results
}

/// Find the value of `"field":` and return it including its surrounding brackets.
/// Works for both array (`[...]`) and object (`{...}`) values.
pub fn json_data_field<'a>(json: &'a str, field: &str) -> Option<&'a str> {
	let search = format!("\"{}\":", field);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];

	let first_char = rest.chars().next()?;
	let (open, close) = match first_char {
		'[' => ('[', ']'),
		'{' => ('{', '}'),
		_ => return None,
	};

	let mut scanner = StringScanner::default();
	let mut depth = 0i32;

	for (i, ch) in rest.char_indices() {
		if !scanner.is_structural(ch) {
			continue;
		}
		if ch == open {
			depth += 1;
		} else if ch == close {
			depth -= 1;
			if depth == 0 {
				return Some(&rest[..=i]);
			}
		}
	}

	None
}

/// Extract top-level objects from a JSON array string like `[{...},{...}]`.
pub fn json_top_level_objects(array_str: &str) -> Vec<&str> {
	let mut results: Vec<&str> = Vec::new();
	let mut scanner = StringScanner::default();
	let mut depth = 0i32;
	let mut obj_start: Option<usize> = None;

	for (i, ch) in array_str.char_indices() {
		if !scanner.is_structural(ch) {
			continue;
		}
		match ch {
			'{' => {
				if depth == 0 {
					obj_start = Some(i);
				}
				depth += 1;
			}
			'}' => {
				depth -= 1;
				if depth == 0 {
					if let Some(start) = obj_start {
						results.push(&array_str[start..=i]);
					}
					obj_start = None;
				}
			}
			_ => {}
		}
	}

	results
}

/// Extract an array of plain strings: `"key":["a","b"]` → `["a", "b"]`.
/// Returns an empty Vec if the key is missing or the array holds objects instead.
pub fn json_string_array(json: &str, key: &str) -> Vec<String> {
	let mut results: Vec<String> = Vec::new();
	let search = format!("\"{}\":[", key);
	let pos = match json.find(&search) {
		Some(p) => p + search.len(),
		None => return results,
	};

	let body = &json[pos..];
	let bytes = body.as_bytes();
	let mut i = 0usize;
	let mut start: Option<usize> = None;

	while i < bytes.len() {
		let byte = bytes[i];
		match start {
			Some(s) => {
				if byte == b'"' && bytes[i - 1] != b'\\' {
					results.push(json_unescape(&body[s..i]));
					start = None;
				}
			}
			None => {
				if byte == b'"' {
					start = Some(i + 1);
				} else if byte == b']' {
					break;
				}
			}
		}
		i += 1;
	}

	results
}

/// Decode JSON string escapes. This API is PHP-generated and escapes forward slashes,
/// so every URL comes back as `https:\/\/...` — without this step images never load.
pub fn json_unescape(value: &str) -> String {
	if !value.contains('\\') {
		return String::from(value);
	}

	let mut out = String::with_capacity(value.len());
	let mut chars = value.chars();

	while let Some(ch) = chars.next() {
		if ch != '\\' {
			out.push(ch);
			continue;
		}
		match chars.next() {
			Some('n') => out.push('\n'),
			Some('r') => out.push('\r'),
			Some('t') => out.push('\t'),
			Some('b') => out.push('\u{8}'),
			Some('f') => out.push('\u{c}'),
			Some('u') => {
				let mut code = 0u32;
				let mut valid = true;
				for _ in 0..4 {
					match chars.next().and_then(|c: char| c.to_digit(16)) {
						Some(digit) => code = code * 16 + digit,
						None => {
							valid = false;
							break;
						}
					}
				}
				if valid {
					if let Some(decoded) = char::from_u32(code) {
						out.push(decoded);
					}
				}
			}
			// Covers \" \\ \/ and anything else we do not special-case.
			Some(other) => out.push(other),
			None => break,
		}
	}

	out
}

/// Read a string field and unescape it, treating empty strings as absent.
pub fn json_text(json: &str, key: &str) -> Option<String> {
	let raw = json_str_value(json, key)?;
	if raw.is_empty() {
		return None;
	}
	Some(json_unescape(raw))
}

/// Read an id that the API may return either quoted or bare.
pub fn json_id(json: &str, key: &str) -> Option<String> {
	if let Some(text) = json_text(json, key) {
		return Some(text);
	}
	json_num_value(json, key).map(|n: i64| format!("{n}"))
}

// ---------------------------------------------------------------------------
// Domain parsing
// ---------------------------------------------------------------------------

/// `serialize` is either a bare status (`连载`) or status plus latest chapter
/// (`连载：第115话`), depending on which endpoint returned the item.
pub fn status_from_serialize(serialize: &str) -> MangaStatus {
	if serialize.contains("完结") || serialize.contains("完結") {
		MangaStatus::Completed
	} else if serialize.contains("连载") || serialize.contains("連載") {
		MangaStatus::Ongoing
	} else {
		MangaStatus::Unknown
	}
}

/// Pull the chapter number out of a title like `第63话 第三季無碼` → `63`.
///
/// Scans every `第` because the first one is not always followed by digits
/// (`第三季 第2话`), then falls back to the first digit run anywhere in the title.
pub fn chapter_number_from_name(name: &str) -> Option<f32> {
	for (idx, ch) in name.char_indices() {
		if ch != '第' {
			continue;
		}
		let rest = &name[idx + ch.len_utf8()..];
		if let Some(number) = leading_number(rest) {
			return Some(number);
		}
	}

	// No `第N` marker: take the first standalone number, e.g. `Ch.12` or `12 話`.
	for (idx, ch) in name.char_indices() {
		if ch.is_ascii_digit() {
			return leading_number(&name[idx..]);
		}
	}

	None
}

/// Collect a leading decimal number from `text`, if it starts with one.
fn leading_number(text: &str) -> Option<f32> {
	let mut buf = String::new();
	for ch in text.chars() {
		// A single decimal point is allowed, but only between digits.
		let accepted = ch.is_ascii_digit() || (ch == '.' && !buf.is_empty() && !buf.contains('.'));
		if !accepted {
			break;
		}
		buf.push(ch);
	}
	if buf.is_empty() || buf.ends_with('.') {
		return None;
	}
	buf.parse::<f32>().ok()
}

/// `tags` arrives in two shapes: the listing/search/rank endpoints return plain strings
/// (`["韩国","条漫"]`) while `comic/detail` returns objects (`[{"id":..,"name":".."}]`).
pub fn parse_tags(obj: &str) -> Vec<String> {
	let search = format!("\"{}\":[", "tags");
	let pos = match obj.find(&search) {
		Some(p) => p + search.len(),
		None => return Vec::new(),
	};

	// Decide by the first meaningful character after the opening bracket.
	let is_object_array = obj[pos..]
		.chars()
		.find(|ch: &char| !ch.is_whitespace())
		.map(|ch: char| ch == '{')
		.unwrap_or(false);

	if is_object_array {
		json_array_objects(obj, "tags")
			.into_iter()
			.filter_map(|entry: &str| json_text(entry, "name"))
			.collect()
	} else {
		json_string_array(obj, "tags")
	}
}

/// Parse one comic object from any of the listing, search, ranking or detail endpoints.
pub fn parse_comic(obj: &str) -> Option<Manga> {
	let key = json_id(obj, "id")?;
	let title = json_text(obj, "name")?;

	let cover = json_text(obj, "pic").or_else(|| json_text(obj, "picx"));

	let authors = json_text(obj, "author").map(|author: String| {
		author
			.split('/')
			.map(|part: &str| String::from(part.trim()))
			.filter(|part: &String| !part.is_empty())
			.collect::<Vec<String>>()
	});

	let description = json_text(obj, "content").or_else(|| json_text(obj, "text"));

	let status = json_text(obj, "serialize")
		.map(|s: String| status_from_serialize(&s))
		.unwrap_or(MangaStatus::Unknown);

	// Every title here is BL aimed at adults; `cadult` marks the explicit ones.
	let content_rating = if json_num_value(obj, "cadult").unwrap_or(0) == 1 {
		ContentRating::NSFW
	} else {
		ContentRating::Suggestive
	};

	let tags = parse_tags(obj);

	Some(Manga {
		url: Some(format!("{BASE_URL}/comic/{key}")),
		key,
		title,
		cover,
		authors: authors.filter(|list: &Vec<String>| !list.is_empty()),
		description,
		tags: if tags.is_empty() { None } else { Some(tags) },
		status,
		content_rating,
		// The catalogue is Korean long-strip work almost throughout, and tags cannot tell
		// strips apart: listings tag them `条漫`, but `comic/detail` swaps those for trope
		// tags and its response replaces the listing entry once a title is opened.
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

/// Parse one entry from `comic/chapter`.
pub fn parse_chapter(obj: &str) -> Option<Chapter> {
	let key = json_id(obj, "id")?;
	let title = json_text(obj, "name");

	let chapter_number = title
		.as_ref()
		.and_then(|name: &String| chapter_number_from_name(name));

	let url = json_text(obj, "link").map(|link: String| format!("{BASE_URL}{link}"));

	let date_uploaded =
		json_text(obj, "addtime").and_then(|date: String| parse_date(date, "yyyy-MM-dd"));

	// Any of these being set means the chapter sits behind VIP or the coin wall. This
	// source only browses as a guest, so the flag is final.
	let locked = json_num_value(obj, "vip").unwrap_or(0) > 0
		|| json_num_value(obj, "cion").unwrap_or(0) > 0
		|| json_num_value(obj, "pay").unwrap_or(0) > 0;

	Some(Chapter {
		key,
		title,
		chapter_number,
		date_uploaded,
		url,
		locked,
		..Default::default()
	})
}

/// Order chapters newest first.
///
/// The API order cannot be trusted: for one title it returned the two afterwords, then the
/// five newest uploads, then 第01话 onwards with 第177话 last. Chapter ids are neither
/// ascending nor descending and upload dates are out of sequence too, so the parsed
/// chapter number is the only usable key. Unnumbered extras (後記, 番外) sink to the
/// bottom in reverse API order, which is the site's rough oldest-first order for them.
pub fn sort_chapters_newest_first(chapters: &mut [Chapter]) {
	chapters.reverse();
	chapters.sort_by(|a: &Chapter, b: &Chapter| match (a.chapter_number, b.chapter_number) {
		(Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(Ordering::Equal),
		(Some(_), None) => Ordering::Less,
		(None, Some(_)) => Ordering::Greater,
		(None, None) => Ordering::Equal,
	});
}

/// Explain an empty page list. A bad or missing chapter id comes back as
/// `{"msg":"章节不存在","code":-1}`, so the server's own message is the most useful thing
/// to show; a `code:1` answer with no images is what a VIP or coin-locked chapter looks
/// like to a guest.
pub fn page_list_error(body: &str) -> String {
	let failed = json_num_value(body, "code").unwrap_or(1) != 1;
	match json_text(body, "msg") {
		Some(msg) if failed => msg,
		_ => String::from("此章節需要 VIP 或金幣，本來源不支援登入"),
	}
}

/// Parse the `data` array of a listing response into manga entries.
pub fn parse_comic_list(body: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(array) = json_data_field(body, "data") {
		for obj in json_top_level_objects(array) {
			if let Some(manga) = parse_comic(obj) {
				entries.push(manga);
			}
		}
	}
	entries
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// Synopses on this site sometimes open a bracketed pull quote, and the truncated
	/// `text` field can cut the closing bracket off entirely. Counting brackets without
	/// skipping string contents made the whole listing come back empty — with no error,
	/// so nothing was logged either.
	const UNBALANCED_BRACKET: &str = r#"{"code":1,"msg":"ok","data":[{"id":"1","name":"A","text":"[未闭合的引言"},{"id":"2","name":"B","text":"正常"}]}"#;

	const BRACE_IN_STRING: &str = r#"{"data":[{"id":"1","name":"{A"},{"id":"2","name":"B}"}]}"#;

	#[aidoku_test]
	fn json_data_field_skips_brackets_inside_strings() {
		let array = json_data_field(UNBALANCED_BRACKET, "data").expect("data array");
		assert!(array.starts_with('['));
		assert!(array.ends_with(']'));
		assert_eq!(json_top_level_objects(array).len(), 2);
	}

	#[aidoku_test]
	fn json_top_level_objects_skips_braces_inside_strings() {
		let array = json_data_field(BRACE_IN_STRING, "data").expect("data array");
		let objects = json_top_level_objects(array);
		assert_eq!(objects.len(), 2);
		assert_eq!(json_str_value(objects[1], "name"), Some("B}"));
	}

	#[aidoku_test]
	fn json_array_objects_skips_punctuation_inside_strings() {
		assert_eq!(json_array_objects(UNBALANCED_BRACKET, "data").len(), 2);
		// Braces in a title would otherwise merge or split the objects.
		assert_eq!(json_array_objects(BRACE_IN_STRING, "data").len(), 2);
	}

	#[aidoku_test]
	fn parses_comics_despite_bracketed_synopsis() {
		let array = json_data_field(UNBALANCED_BRACKET, "data").expect("data array");
		let comics: Vec<Manga> = json_top_level_objects(array)
			.into_iter()
			.filter_map(parse_comic)
			.collect();
		assert_eq!(comics.len(), 2);
		assert_eq!(comics[0].key, "1");
		assert_eq!(comics[1].title, "B");
	}

	#[aidoku_test]
	fn json_unescape_restores_php_escaped_slashes() {
		assert_eq!(json_unescape(r#"https:\/\/a.example\/b.webp"#), "https://a.example/b.webp");
		assert_eq!(json_unescape(r#"say \"hi\""#), "say \"hi\"");
	}

	#[aidoku_test]
	fn json_string_array_reads_plain_strings() {
		let obj = r#"{"tags":["韩国","条漫"],"id":"7"}"#;
		assert_eq!(json_string_array(obj, "tags"), ["韩国", "条漫"]);
	}

	#[aidoku_test]
	fn chapter_numbers_come_from_the_title() {
		assert_eq!(chapter_number_from_name("第1话"), Some(1.0));
		// The first 第 is not always followed by digits.
		assert_eq!(chapter_number_from_name("第三季 第2话"), Some(2.0));
		assert_eq!(chapter_number_from_name("第63话 第三季"), Some(63.0));
		assert_eq!(chapter_number_from_name("番外篇"), None);
	}

	#[aidoku_test]
	fn json_num_value_reads_bare_quoted_and_negative_numbers() {
		assert_eq!(json_num_value(r#"{"total_pages":5767}"#, "total_pages"), Some(5767));
		assert_eq!(json_num_value(r#"{"cadult":"1"}"#, "cadult"), Some(1));
		assert_eq!(json_num_value(r#"{"page": 3}"#, "page"), Some(3));
		assert_eq!(json_num_value(r#"{"msg":"章节不存在","code":-1}"#, "code"), Some(-1));
	}

	#[aidoku_test]
	fn json_num_value_does_not_scan_past_a_non_numeric_value() {
		// An empty, null or boolean value must not borrow the next field's digits.
		assert_eq!(json_num_value(r#"{"vip":"","id":5}"#, "vip"), None);
		assert_eq!(json_num_value(r#"{"vip":null,"id":5}"#, "vip"), None);
		assert_eq!(json_num_value(r#"{"vip":true,"id":5}"#, "vip"), None);
		assert_eq!(json_num_value(r#"{"id":5}"#, "vip"), None);
	}

	/// The exact shape `comic/chapter?mid=417` returned: afterwords first, then the five
	/// newest uploads, then the numbered run with 第177话 last.
	#[aidoku_test]
	fn chapters_sort_newest_first_with_extras_at_the_bottom() {
		let api_order = [
			"後記", "第二季後記", "第176话", "第178话", "第179话", "第180话", "第181话", "第01话",
			"第03话", "第175话", "第177话",
		];
		let mut chapters: Vec<Chapter> = api_order
			.iter()
			.map(|name: &&str| Chapter {
				title: Some(String::from(*name)),
				chapter_number: chapter_number_from_name(name),
				..Default::default()
			})
			.collect();

		sort_chapters_newest_first(&mut chapters);

		let titles: Vec<&str> = chapters
			.iter()
			.map(|chapter: &Chapter| chapter.title.as_deref().unwrap_or(""))
			.collect();
		assert_eq!(
			titles,
			[
				"第181话", "第180话", "第179话", "第178话", "第177话", "第176话", "第175话", "第03话",
				"第01话", "第二季後記", "後記",
			]
		);
	}

	#[aidoku_test]
	fn every_title_reads_as_a_webtoon() {
		// `comic/detail` swaps the listing's `条漫` tag for trope tags; the viewer must not
		// flip when the detail response replaces the listing entry.
		let detail = r#"{"id":417,"name":"A","tags":[{"id":92,"name":"美人受"}]}"#;
		let listing = r#"{"id":"1","name":"B","tags":["韩国","条漫"]}"#;
		assert_eq!(parse_comic(detail).expect("detail").viewer, Viewer::Webtoon);
		assert_eq!(parse_comic(listing).expect("listing").viewer, Viewer::Webtoon);
	}

	#[aidoku_test]
	fn page_list_error_prefers_the_server_message() {
		assert_eq!(page_list_error(r#"{"msg":"章节不存在","code":-1}"#), "章节不存在");
		assert_eq!(
			page_list_error(r#"{"code":1,"msg":"图片列表","data":[]}"#),
			"此章節需要 VIP 或金幣，本來源不支援登入"
		);
	}
}
