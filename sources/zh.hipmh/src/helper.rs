use aidoku::{
	alloc::{collections::BTreeSet, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{defaults::defaults_get, net::Request, std::parse_date},
	prelude::*,
	Chapter, ContentRating, Manga, MangaStatus, Result, Viewer,
};

pub const BASE_URL: &str = "https://m.hipmh.com";
/// The site's own JSON API. Every endpoint answers without any header.
pub const API_URL: &str = "https://hipapi1.s3file.top";
const COVER_HOST: &str = "https://cover.s3imgs.top";

/// Setting key for the image route picker in `res/settings.json`.
pub const IMAGE_LINE_KEY: &str = "image_line";
const DEFAULT_IMAGE_LINE: &str = "tx";

/// `/v1/manga/chapters` caps `per_page` at 50 whatever is asked for.
pub const CHAPTERS_PER_PAGE: i32 = 50;
pub const LIST_PER_PAGE: i32 = 30;
pub const SEARCH_PAGE_SIZE: i32 = 30;

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------

/// Fetch an API endpoint and return the `data` value of its `{code, message, data}`
/// envelope. A non-2xx status or a non-200 `code` becomes an error carrying the
/// server's message.
pub fn fetch_data(url: &str) -> Result<String> {
	let response = Request::get(url)?.send()?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[hipmh] {url}: HTTP {status}");
	}
	let body = response.get_string()?;
	let code = json_i64(&body, "code");
	if code != Some(200) {
		bail!(
			"[hipmh] {url}: code {code:?} {}",
			json_string(&body, "message").unwrap_or_default()
		);
	}
	json_field(&body, "data")
		.map(String::from)
		.ok_or_else(|| error!("[hipmh] {url}: no data"))
}

// ---------------------------------------------------------------------------
// Hand-rolled JSON scanning (no serde: keeps the wasm binary small)
//
// Unlike the `json_str_value` family in zh.tibiu, which returns the first occurrence of a
// key anywhere in the text, these only look at the fields of the object they are given.
// That matters here: `/v1/manga` lists `first_chapter: {"title": ...}` before its own
// `title`, so a text search would name every manga after its first chapter.
// ---------------------------------------------------------------------------

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
	while i < bytes.len() && bytes[i].is_ascii_whitespace() {
		i += 1;
	}
	i
}

/// `i` points at an opening quote; returns the index just past the closing one.
fn string_end(bytes: &[u8], mut i: usize) -> usize {
	i += 1;
	while i < bytes.len() {
		match bytes[i] {
			b'\\' => i += 2,
			b'"' => return i + 1,
			_ => i += 1,
		}
	}
	bytes.len()
}

/// `i` points at the first byte of a value; returns the index just past it.
fn value_end(bytes: &[u8], i: usize) -> usize {
	match bytes.get(i) {
		Some(b'"') => string_end(bytes, i),
		Some(b'{') | Some(b'[') => {
			let mut depth = 0i32;
			let mut j = i;
			while j < bytes.len() {
				match bytes[j] {
					b'"' => {
						j = string_end(bytes, j);
						continue;
					}
					b'{' | b'[' => depth += 1,
					b'}' | b']' => {
						depth -= 1;
						if depth == 0 {
							return j + 1;
						}
					}
					_ => {}
				}
				j += 1;
			}
			bytes.len()
		}
		_ => {
			let mut j = i;
			while j < bytes.len() && !matches!(bytes[j], b',' | b'}' | b']') {
				j += 1;
			}
			j
		}
	}
}

/// The raw text of every element of a JSON array, or every value of a JSON object
/// together with its raw key.
fn members(json: &str) -> Vec<(Option<&str>, &str)> {
	let bytes = json.as_bytes();
	let mut out = Vec::new();
	let mut i = skip_ws(bytes, 0);
	let is_object = match bytes.get(i) {
		Some(b'{') => true,
		Some(b'[') => false,
		_ => return out,
	};
	i += 1;
	loop {
		i = skip_ws(bytes, i);
		match bytes.get(i) {
			None | Some(b'}') | Some(b']') => break,
			Some(b',') => {
				i += 1;
				continue;
			}
			_ => {}
		}
		let mut key = None;
		if is_object {
			if bytes[i] != b'"' {
				break;
			}
			let end = string_end(bytes, i);
			key = Some(&json[i + 1..end.saturating_sub(1).max(i + 1)]);
			i = skip_ws(bytes, end);
			if bytes.get(i) != Some(&b':') {
				break;
			}
			i = skip_ws(bytes, i + 1);
		}
		let end = value_end(bytes, i);
		if end <= i {
			break;
		}
		out.push((key, json[i..end].trim_end()));
		i = end;
	}
	out
}

/// The raw value of a top-level field of `obj`.
pub fn json_field<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
	members(obj)
		.into_iter()
		.find(|(k, _): &(Option<&str>, &str)| *k == Some(key))
		.map(|(_, value): (Option<&str>, &str)| value)
}

/// The raw elements of a JSON array.
pub fn json_items(array: &str) -> Vec<&str> {
	members(array)
		.into_iter()
		.map(|(_, value): (Option<&str>, &str)| value)
		.collect()
}

/// A raw value as a string, if it is a JSON string literal.
fn as_string(raw: &str) -> Option<String> {
	let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
	Some(json_unescape(inner))
}

/// A top-level string field, with empty strings treated as absent.
pub fn json_string(obj: &str, key: &str) -> Option<String> {
	json_field(obj, key)
		.and_then(as_string)
		.filter(|s: &String| !s.trim().is_empty())
}

/// A top-level numeric field. Fractions are truncated.
pub fn json_i64(obj: &str, key: &str) -> Option<i64> {
	json_f64(obj, key).map(|n: f64| n as i64)
}

pub fn json_f64(obj: &str, key: &str) -> Option<f64> {
	json_field(obj, key)?.parse::<f64>().ok()
}

/// The string elements of a top-level JSON array, e.g. the decoded image list.
pub fn json_top_strings(array: &str) -> Vec<String> {
	json_items(array)
		.into_iter()
		.filter_map(as_string)
		.collect()
}

/// A field holding either `["a","b"]` or `[{"name":"a"},{"name":"b"}]`. The list and
/// search endpoints disagree on which shape `genres` takes.
pub fn json_names(obj: &str, key: &str) -> Vec<String> {
	let Some(array) = json_field(obj, key) else {
		return Vec::new();
	};
	json_items(array)
		.into_iter()
		.filter_map(|item: &str| as_string(item).or_else(|| json_string(item, "name")))
		.map(|name: String| String::from(name.trim()))
		.filter(|name: &String| !name.is_empty())
		.collect()
}

/// Decode JSON string escapes, including surrogate pairs.
pub fn json_unescape(value: &str) -> String {
	if !value.contains('\\') {
		return String::from(value);
	}

	let mut out = String::with_capacity(value.len());
	let mut chars = value.chars();
	let mut pending_high: Option<u32> = None;

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
				for _ in 0..4 {
					match chars.next().and_then(|c: char| c.to_digit(16)) {
						Some(digit) => code = code * 16 + digit,
						None => return out,
					}
				}
				match (pending_high.take(), code) {
					(None, 0xD800..=0xDBFF) => pending_high = Some(code),
					(Some(high), 0xDC00..=0xDFFF) => {
						let combined = 0x10000 + ((high - 0xD800) << 10) + (code - 0xDC00);
						if let Some(decoded) = char::from_u32(combined) {
							out.push(decoded);
						}
					}
					(_, code) => {
						if let Some(decoded) = char::from_u32(code) {
							out.push(decoded);
						}
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

// ---------------------------------------------------------------------------
// Keys and URLs
// ---------------------------------------------------------------------------

/// Manga ids come in two shapes: `bTo4MDgz` (base64url of `m:8083`) and the list
/// endpoints' `bTo4MDgz-wo-tian-ming-da-fan-pai-8076`. Only the short one works as the
/// `mid` of `/v1/manga` (the long one is answered with 400), so that is the manga key.
pub fn short_mid(mid: &str) -> &str {
	mid.split('-').next().unwrap_or(mid)
}

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/works/{key}")
}

fn cover_url(path: &str) -> String {
	if path.starts_with("http://") || path.starts_with("https://") {
		String::from(path)
	} else {
		format!("{COVER_HOST}{path}")
	}
}

pub fn search_url(query: &str, page: i32, page_size: i32) -> String {
	format!(
		"{API_URL}/v1/search?q={}&page={page}&page_size={page_size}",
		encode_uri_component(query)
	)
}

pub fn chapters_url(key: &str, page: i32, timestamp: i64) -> String {
	format!(
		"{API_URL}/v1/manga/chapters?mid={key}&page={page}&per_page={CHAPTERS_PER_PAGE}&order=desc&_={timestamp}"
	)
}

/// A timestamp changes the CDN cache key, but does not pin a server-side snapshot.
/// Reject inconsistent pages rather than replacing a library's chapters with gaps.
pub fn fetch_chapters(
	key: &str,
	timestamp: i64,
	mut fetch: impl FnMut(&str) -> Result<String>,
) -> Option<Vec<Chapter>> {
	let mut chapters = Vec::new();
	let mut expected = None;
	let mut page = 1;
	loop {
		let data = fetch(&chapters_url(key, page, timestamp)).ok()?;
		let total: usize = json_field(&data, "total")?.parse().ok()?;
		let total_pages: i32 = json_field(&data, "total_pages")?.parse().ok()?;
		let response_page: i32 = json_field(&data, "page")?.parse().ok()?;
		let per_page: i32 = json_field(&data, "per_page")?.parse().ok()?;
		if total == 0
			|| total_pages < 1
			|| total.div_ceil(CHAPTERS_PER_PAGE as usize) != usize::try_from(total_pages).ok()?
			|| response_page != page
			|| per_page != CHAPTERS_PER_PAGE
			|| expected.is_some_and(|metadata| metadata != (total, total_pages))
		{
			return None;
		}
		expected = Some((total, total_pages));
		let parsed = parse_chapter_page(&data)?;
		if parsed.len()
			!= total
				.checked_sub(chapters.len())?
				.min(CHAPTERS_PER_PAGE as usize)
		{
			return None;
		}
		chapters.extend(parsed);
		if page == total_pages {
			break;
		}
		page += 1;
	}
	number_by_position(&mut chapters);
	chapters_to_store(true, chapters)
}

/// When no title in the list has a number, the position in the list (oldest is 1) is the
/// chapter number, so the tracker still has one to sync. A list that mixes numbered and
/// unnumbered titles keeps `None` for the unnumbered ones, as the official zh.jmcomic does.
/// The list is newest first, so the position is `len - index`.
pub fn number_by_position(chapters: &mut [Chapter]) {
	if !chapters
		.iter()
		.all(|c: &Chapter| c.chapter_number.is_none() && c.volume_number.is_none())
	{
		return;
	}
	let len = chapters.len();
	for (index, chapter) in chapters.iter_mut().enumerate() {
		chapter.chapter_number = Some((len - index) as f32);
	}
}

/// Image host for the picked route. `line == 9` in a chapter response switches the
/// reader to the `-s1` host of the same route, so this does too.
pub fn image_host(line: Option<i64>) -> String {
	let route = defaults_get::<String>(IMAGE_LINE_KEY)
		.filter(|route: &String| route == "tx" || route == "cf")
		.unwrap_or_else(|| String::from(DEFAULT_IMAGE_LINE));
	let server = if line == Some(9) { "s1" } else { "1" };
	format!("https://hip-{route}-{server}.s3imgs.top")
}

// ---------------------------------------------------------------------------
// Chapter hids
// ---------------------------------------------------------------------------

/// Chapter lists hand out the reader hid, `base64url("m:8083-c:9203")-base64url("8083:1.00")`.
/// `/v2/chapter` only takes the API hid, `base64url("c:9203")-` plus the same tail, and
/// answers 404 to the reader hid. Returns `(manga key, api hid)`.
pub fn api_hid(reader_hid: &str) -> Option<(String, String)> {
	let (head, tail) = reader_hid.split_once('-')?;
	let decoded = String::from_utf8(base64_url_decode(head)?).ok()?;
	let (manga, chapter) = decoded.split_once('-')?;
	if !manga.starts_with("m:") || !chapter.starts_with("c:") || tail.is_empty() {
		return None;
	}
	Some((
		base64_url_encode(manga.as_bytes()),
		format!("{}-{tail}", base64_url_encode(chapter.as_bytes())),
	))
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url, as the site writes its ids.
fn base64_url_encode(bytes: &[u8]) -> String {
	let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
	for chunk in bytes.chunks(3) {
		let n = (u32::from(chunk[0]) << 16)
			| (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
			| u32::from(*chunk.get(2).unwrap_or(&0));
		for i in 0..=chunk.len() {
			out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
		}
	}
	out
}

fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
	let mut out = Vec::with_capacity(text.len() * 3 / 4);
	let mut buffer = 0u32;
	let mut bits = 0u32;
	for byte in text.bytes() {
		if byte == b'=' {
			break;
		}
		let value = B64.iter().position(|&c: &u8| c == byte)? as u32;
		buffer = ((buffer << 6) | value) & 0xFFFF;
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((buffer >> bits) as u8);
		}
	}
	Some(out)
}

// ---------------------------------------------------------------------------
// Domain parsing
// ---------------------------------------------------------------------------

/// `/v1/mangas` rates with a number (1 safe, 2 adult); `/v1/search` and `/v1/manga` with a
/// string (`safe`, `adult`). Anything that is not explicitly safe counts as adult.
fn content_rating(obj: &str) -> ContentRating {
	match json_field(obj, "content_rating") {
		None | Some("null") => ContentRating::Unknown,
		Some("1") | Some("\"safe\"") => ContentRating::Safe,
		Some(_) => ContentRating::NSFW,
	}
}

fn status(obj: &str) -> MangaStatus {
	match json_string(obj, "status").as_deref() {
		Some("ongoing") => MangaStatus::Ongoing,
		Some("completed") => MangaStatus::Completed,
		Some("hiatus") => MangaStatus::Hiatus,
		Some("cancelled") => MangaStatus::Cancelled,
		_ => MangaStatus::Unknown,
	}
}

/// Parse a manga from `/v1/mangas` (`mid`, string `genres`, `author_names`), `/v1/search`
/// (`id`, object `genres` and `authors`), `/v1/manga` (`id` is numeric there, so the key is
/// taken from `fallback_key`) or the `mid`-shaped rows of `/v1/home`.
pub fn parse_manga(obj: &str, fallback_key: Option<&str>) -> Option<Manga> {
	let key = json_string(obj, "mid")
		.or_else(|| json_string(obj, "id"))
		.map(|mid: String| String::from(short_mid(&mid)))
		.or_else(|| fallback_key.map(String::from))?;
	let title = json_string(obj, "title")?;

	let mut authors = json_names(obj, "authors");
	if authors.is_empty() {
		authors = json_names(obj, "author_names");
	}
	let tags = json_names(obj, "genres");

	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover: json_string(obj, "vertical_image_url")
			.or_else(|| json_string(obj, "cover_image_url"))
			.map(|path: String| cover_url(&path)),
		authors: (!authors.is_empty()).then_some(authors),
		description: json_string(obj, "description"),
		tags: (!tags.is_empty()).then_some(tags),
		status: status(obj),
		content_rating: content_rating(obj),
		// The site's reader is a vertical strip for every title, and the catalogue is
		// Korean and Chinese long-strip work almost throughout.
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

/// Parse a list of manga objects, dropping repeats: search returns some titles twice
/// under the same short id with different slugs.
pub fn parse_manga_list(array: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	for obj in json_items(array) {
		if let Some(manga) = parse_manga(obj, None) {
			if !entries.iter().any(|m: &Manga| m.key == manga.key) {
				entries.push(manga);
			}
		}
	}
	entries
}

/// `banners` and `featured` rows of `/v1/home` link to a manga instead of describing one.
pub fn parse_home_link(obj: &str) -> Option<Manga> {
	let link = json_string(obj, "link")?;
	let key = String::from(short_mid(&link));
	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title: json_string(obj, "title")?,
		cover: json_string(obj, "image_url").map(|path: String| cover_url(&path)),
		description: json_string(obj, "sub_title"),
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

/// Pull the chapter number out of a title like `第359话 她一定很想我吧？` → `359`.
///
/// The API's own `chapter_number` is a running index (that chapter is 361), so the
/// title is the number readers see. Titles without one (番外, 序章) give `None`.
pub fn chapter_number_from_title(title: &str) -> Option<f32> {
	const NUMERALS: &str = "零一二三四五六七八九十";
	for (idx, ch) in title.char_indices() {
		if ch != '第' {
			continue;
		}
		let rest = &title[idx + ch.len_utf8()..];
		let mut buf = String::new();
		for c in rest.chars() {
			if c.is_ascii_digit()
				|| NUMERALS.contains(c)
				|| (c == '.' && !buf.is_empty() && !buf.contains('.'))
			{
				buf.push(c);
			} else {
				break;
			}
		}
		let consumed = buf.len();
		let buf = buf.trim_end_matches('.');
		// Only a number followed by a chapter unit is a chapter: `第2季` and a range
		// such as `第1-3話` are not chapter 2 or chapter 1.
		let unit = rest[consumed..].chars().next();
		if !buf.is_empty() && matches!(unit, Some('話' | '话' | '回')) {
			return buf.parse::<f32>().ok().or_else(|| chinese_number(buf));
		}
	}
	None
}

fn chinese_number(text: &str) -> Option<f32> {
	let digit = |c: char| "零一二三四五六七八九".chars().position(|d: char| d == c);
	let chars: Vec<char> = text.chars().collect();
	match chars.as_slice() {
		['十'] => Some(10.0),
		[c] => digit(*c).map(|d: usize| d as f32),
		['十', ones] => digit(*ones).map(|d: usize| 10.0 + d as f32),
		[tens, '十'] => digit(*tens).map(|d: usize| d as f32 * 10.0),
		[tens, '十', ones] => Some((digit(*tens)? * 10 + digit(*ones)?) as f32),
		_ => None,
	}
}

/// `第2季 第5话` → (Some(5), Some(2)), `第二季` → (None, Some(2)), `第3卷` → (None, Some(3)),
/// `第359话` → (Some(359), None). A leading volume, season, part or book is the volume, as
/// the official zh.copymanga does. A volume followed by text that is not a chapter
/// (`第3卷 後記`) is an appendix of that volume and gets none, as the official zh.zaimanhua
/// does (`is_extra`): it would share the volume's number with `第3卷` itself.
/// A list where no title has a number is numbered by position in `number_by_position`.
pub fn numbers_from_title(title: &str) -> (Option<f32>, Option<f32>) {
	let title = title.trim();
	if let Some(rest) = volume_marker_rest(title) {
		if let Some(end) = rest.find(['卷', '季', '部', '冊', '册']) {
			let number = rest[..end].trim();
			if let Some(volume) = number
				.parse::<f32>()
				.ok()
				.or_else(|| chinese_number(number))
			{
				let unit = rest[end..].chars().next().unwrap_or('卷');
				let after = rest[end + unit.len_utf8()..].trim();
				let chapter = chapter_number_from_title(after);
				if unit == '卷' && !after.is_empty() && chapter.is_none() {
					return (None, None);
				}
				return (chapter, Some(volume));
			}
		}
	}
	(chapter_number_from_title(title), bare_volume(title))
}

/// The text a volume number is read from. `第3卷` gives `3卷`. The `第` may be left out
/// (`3卷 第5話`, `三季 第2話`) when the number is followed by a volume unit and then the end of
/// the title, a space or `第`, as in the official zh.boylove (`chapter_list/mod.rs:111`) and
/// zh.zaimanhua. `3卷後記`, `三部曲` and `一部分` have text right behind the unit, so they are
/// not volumes.
fn volume_marker_rest(title: &str) -> Option<&str> {
	if let Some(rest) = title.strip_prefix('第') {
		return Some(rest);
	}
	let end = title.find(['卷', '季', '部', '冊', '册'])?;
	// Without 第, a space between the number and the unit is not a volume, so the title
	// keeps the result it had before 第 became optional.
	let number = &title[..end];
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))?;
	let after = &title[end + title[end..].chars().next()?.len_utf8()..];
	(after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with('第'))
		.then_some(title)
}

/// `3卷` → 3 and `三卷` → 3. Only a title that is just the number and `卷` counts, as with
/// `第3卷`; `3卷後記` is an appendix and gives `None`.
fn bare_volume(title: &str) -> Option<f32> {
	let number = title.trim().strip_suffix('卷')?.trim();
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))
}

/// Parse one entry of `/v1/manga/chapters`. The key is the API hid, so opening a chapter
/// needs no further lookups; the url keeps the reader hid the site itself links to.
pub fn parse_chapter(obj: &str) -> Option<Chapter> {
	let reader_hid = json_string(obj, "hid")?;
	let (_, key) = api_hid(&reader_hid)?;
	let title = json_string(obj, "title");
	let (chapter_number, volume_number) = title.as_deref().map_or((None, None), numbers_from_title);
	// `2026-02-07T18:24:45.755357Z`: the fraction varies in length, so parse the first
	// 19 characters, which are UTC. The `T` becomes a space because aidoku-test-runner
	// does not understand the quoted `'T'` literal the app's DateFormatter would accept.
	let date_uploaded = json_string(obj, "created_at")
		.filter(|date: &String| date.len() >= 19 && date.is_char_boundary(19))
		.and_then(|date: String| parse_date(date[..19].replace('T', " "), "yyyy-MM-dd HH:mm:ss"));

	Some(Chapter {
		key,
		title,
		chapter_number,
		volume_number,
		date_uploaded,
		url: Some(format!("{BASE_URL}/chapter/go?hid={reader_hid}")),
		..Default::default()
	})
}

/// One page of `/v1/manga/chapters`, or `None` when it has no `items` or an entry cannot
/// be read: either would silently drop chapters from the list.
pub fn parse_chapter_page(data: &str) -> Option<Vec<Chapter>> {
	let items = json_field(data, "items")?;
	if !items.starts_with('[') || !items.ends_with(']') {
		return None;
	}
	json_items(items).into_iter().map(parse_chapter).collect()
}

/// The walked list, if the library should take it. A partial list would read as chapters
/// having been removed, and an unknown mid answers an empty list (checked 2026-10-20).
pub fn chapters_to_store(complete: bool, chapters: Vec<Chapter>) -> Option<Vec<Chapter>> {
	if !complete || chapters.is_empty() {
		return None;
	}
	let unique = {
		let keys: BTreeSet<&str> = chapters
			.iter()
			.map(|chapter| chapter.key.as_str())
			.collect();
		keys.len() == chapters.len()
	};
	unique.then_some(chapters)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn chapter_requests_avoid_the_fixed_cache_key() {
		let url = chapters_url("bTo3MDU1", 1, 1793613600);
		assert!(url.ends_with("&_=1793613600"), "{url}");
	}

	#[aidoku_test]
	fn duplicate_chapter_keys_reject_the_list() {
		let chapter = r#"{"hid":"bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw","title":"第1话"}"#;
		let chapters =
			parse_chapter_page(&format!(r#"{{"items":[{chapter},{chapter}]}}"#)).expect("chapters");
		assert!(chapters_to_store(true, chapters).is_none());
	}

	#[aidoku_test]
	fn invalid_pagination_never_returns_a_partial_list() {
		let chapter = r#"{"hid":"bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw","title":"第1话"}"#;
		let valid =
			format!(r#"{{"page":1,"per_page":50,"total":1,"total_pages":1,"items":[{chapter}]}}"#);
		let chapters = fetch_chapters("bTo4MDgz", 123, |_| Ok(valid.clone())).expect("valid page");
		assert_eq!(chapters.len(), 1);
		assert_eq!(chapters[0].key, "Yzo5MjAz-ODA4MzoxLjAw");
		for invalid in [
			valid.replace(r#""total":1,"#, ""),
			valid.replace(r#""total":1"#, r#""total":-1"#),
			valid.replace(r#""total":1"#, r#""total":1.5"#),
			valid.replace(r#""total":1"#, r#""total":2"#),
			valid.replace(r#""total_pages":1"#, r#""total_pages":0"#),
			valid.replace(r#""total_pages":1"#, r#""total_pages":2"#),
			valid.replace(r#""page":1"#, r#""page":2"#),
			valid.replace(r#""per_page":50"#, r#""per_page":20"#),
			valid.replace(&format!("[{chapter}]"), "[]"),
			valid.replace(&format!("[{chapter}]"), "null"),
			valid.replace(&format!("[{chapter}]"), "{}"),
		] {
			let mut requests = 0;
			assert!(
				fetch_chapters("bTo4MDgz", 123, |_| {
					requests += 1;
					Ok(invalid.clone())
				})
				.is_none(),
				"{invalid}"
			);
			assert_eq!(requests, 1);
		}
	}

	#[aidoku_test]
	fn fields_are_read_from_the_top_level_only() {
		let detail = r#"{"first_chapter":{"title":"第1话"},"id":8083,"title":"我！天命大反派"}"#;
		assert_eq!(
			json_string(detail, "title").as_deref(),
			Some("我！天命大反派")
		);
		assert_eq!(json_i64(detail, "id"), Some(8083));
	}

	#[aidoku_test]
	fn brackets_inside_strings_do_not_end_values() {
		let obj = r#"{"description":"【独家】 {x] \"q\"","title":"A"}"#;
		assert_eq!(json_string(obj, "title").as_deref(), Some("A"));
		assert_eq!(
			json_string(obj, "description").as_deref(),
			Some("【独家】 {x] \"q\"")
		);
	}

	#[aidoku_test]
	fn names_read_both_shapes() {
		assert_eq!(
			json_names(r#"{"genres":["古风","少年"]}"#, "genres"),
			["古风", "少年"]
		);
		assert_eq!(
			json_names(
				r#"{"authors":[{"id":2178,"name":" Chira","role":null}]}"#,
				"authors"
			),
			["Chira"]
		);
	}

	#[aidoku_test]
	fn unescape_handles_surrogate_pairs() {
		assert_eq!(json_unescape(r#"\ud83d\ude00\u003c\/"#), "😀</");
	}

	#[aidoku_test]
	fn api_hid_is_derived_from_the_reader_hid() {
		assert_eq!(
			api_hid("bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw"),
			Some((
				String::from("bTo4MDgz"),
				String::from("Yzo5MjAz-ODA4MzoxLjAw")
			))
		);
		assert_eq!(
			api_hid("bTo4MDgzLWM6MTkyMzc1-ODA4MzozNjEuMDA"),
			Some((
				String::from("bTo4MDgz"),
				String::from("YzoxOTIzNzU-ODA4MzozNjEuMDA")
			))
		);
		assert_eq!(api_hid("bTo4MDgz"), None);
	}

	#[aidoku_test]
	fn content_rating_reads_numbers_and_strings() {
		assert_eq!(
			content_rating(r#"{"content_rating":1}"#),
			ContentRating::Safe
		);
		assert_eq!(
			content_rating(r#"{"content_rating":2}"#),
			ContentRating::NSFW
		);
		assert_eq!(
			content_rating(r#"{"content_rating":"safe"}"#),
			ContentRating::Safe
		);
		assert_eq!(
			content_rating(r#"{"content_rating":"adult"}"#),
			ContentRating::NSFW
		);
		assert_eq!(content_rating(r#"{"title":"A"}"#), ContentRating::Unknown);
	}

	#[aidoku_test]
	fn list_and_search_rows_share_a_key() {
		let list = r#"[{"mid":"bToyODU1MQ-dai-zun-mi-lu-dm-2827-21341","title":"怠尊秘录","content_rating":1,"vertical_image_url":"/dongman/vertical/x.webp","author_names":["悲歌"],"genres":["古风"]}]"#;
		let search = r#"[{"id":"bToyODU1MQ-dai-zun-mi-lu-dm-2827-21341","title":"怠尊秘录"},{"id":"bToyODU1MQ-dai-zun-mi-lu-dongman-2827-21341","title":"怠尊秘录"}]"#;
		let from_list = parse_manga_list(list);
		assert_eq!(from_list[0].key, "bToyODU1MQ");
		assert_eq!(
			from_list[0].cover.as_deref(),
			Some("https://cover.s3imgs.top/dongman/vertical/x.webp")
		);
		assert_eq!(
			from_list[0].url.as_deref(),
			Some("https://m.hipmh.com/works/bToyODU1MQ")
		);
		let from_search = parse_manga_list(search);
		assert_eq!(from_search.len(), 1);
		assert_eq!(from_search[0].key, "bToyODU1MQ");
	}

	#[aidoku_test]
	fn chapter_numbers_come_from_the_title() {
		assert_eq!(
			chapter_number_from_title("第359话 她一定很想我吧？"),
			Some(359.0)
		);
		assert_eq!(chapter_number_from_title("第三季 第2話"), Some(2.0));
		assert_eq!(chapter_number_from_title("番外"), None);
		assert_eq!(chapter_number_from_title("第2季"), None);
		assert_eq!(chapter_number_from_title("第1-3話"), None);
		assert_eq!(numbers_from_title("第2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers_from_title("第二季"), (None, Some(2.0)));
		assert_eq!(numbers_from_title("第3卷"), (None, Some(3.0)));
		assert_eq!(numbers_from_title("第3卷 第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers_from_title("第3卷 後記"), (None, None));
		assert_eq!(numbers_from_title("3卷"), (None, Some(3.0)));
		assert_eq!(numbers_from_title("三卷"), (None, Some(3.0)));
		assert_eq!(numbers_from_title("01卷後記"), (None, None));
		assert_eq!(numbers_from_title("3卷 第5話"), (Some(5.0), Some(3.0)));
		// A space between the number and the unit is not read as a volume.
		assert_eq!(numbers_from_title("1 卷 第5話"), (Some(5.0), None));
		assert_eq!(numbers_from_title("3卷第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers_from_title("三卷 第五話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers_from_title("2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers_from_title("3卷 後記"), (None, None));
		assert_eq!(numbers_from_title("3卷卷首"), (None, None));
		assert_eq!(numbers_from_title("06卷番外"), (None, None));
		assert_eq!(numbers_from_title("一部分"), (None, None));
		assert_eq!(numbers_from_title("三部曲 第2話"), (Some(2.0), None));
		assert_eq!(numbers_from_title("卷三"), (None, None));
		assert_eq!(numbers_from_title("卷 3"), (None, None));
		assert_eq!(chapter_number_from_title("第十二話"), Some(12.0));
		assert_eq!(numbers_from_title("第1部 第5話"), (Some(5.0), Some(1.0)));
		assert_eq!(
			numbers_from_title("第359话 她一定很想我吧？"),
			(Some(359.0), None)
		);
		assert_eq!(numbers_from_title("番外"), (None, None));
	}

	#[aidoku_test]
	fn a_mixed_list_keeps_none_and_a_list_without_numbers_is_numbered() {
		let numbers_of = |titles: &[&str]| -> Vec<(Option<f32>, Option<f32>)> {
			let mut chapters: Vec<Chapter> = titles
				.iter()
				.map(|title: &&str| {
					let (chapter_number, volume_number) = numbers_from_title(title);
					Chapter {
						chapter_number,
						volume_number,
						..Default::default()
					}
				})
				.collect();
			number_by_position(&mut chapters);
			chapters
				.iter()
				.map(|c: &Chapter| (c.chapter_number, c.volume_number))
				.collect()
		};
		// Newest first. Numbered and unnumbered titles mixed: the unnumbered stay None.
		assert_eq!(
			numbers_of(&["第3卷 後記", "第二季", "番外", "第1-3話", "第1話"]),
			[
				(None, None),
				(None, Some(2.0)),
				(None, None),
				(None, None),
				(Some(1.0), None)
			]
		);
		// No title has a number: the position, oldest first.
		assert_eq!(
			numbers_of(&["番外", "第1-3話", "序章"]),
			[(Some(3.0), None), (Some(2.0), None), (Some(1.0), None)]
		);
	}

	#[aidoku_test]
	fn chapter_keys_are_api_hids() {
		let obj = r#"{"hid":"bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw","chapter_number":1,"title":"第1话 我穿成了大反派？","created_at":"2026-02-07T18:24:45.755357Z"}"#;
		let chapter = parse_chapter(obj).expect("chapter");
		assert_eq!(chapter.key, "Yzo5MjAz-ODA4MzoxLjAw");
		assert_eq!(chapter.chapter_number, Some(1.0));
		assert_eq!(
			chapter.url.as_deref(),
			Some("https://m.hipmh.com/chapter/go?hid=bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw")
		);
		assert_eq!(chapter.date_uploaded, Some(1770488685));
	}

	#[aidoku_test]
	fn a_bad_chapter_page_rejects_the_list() {
		let good = r#"{"hid":"bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw","title":"第1话"}"#;
		let page = format!(r#"{{"items":[{good}],"total_pages":1}}"#);
		assert_eq!(parse_chapter_page(&page).map(|c| c.len()), Some(1));
		// No `items` field.
		assert!(parse_chapter_page(r#"{"total_pages":1}"#).is_none());
		// One entry without a hid.
		let page = format!(r#"{{"items":[{good},{{"title":"第2话"}}],"total_pages":1}}"#);
		assert!(parse_chapter_page(&page).is_none());

		let chapters = parse_chapter_page(r#"{"items":[],"total_pages":0}"#).expect("empty page");
		assert!(chapters_to_store(true, chapters).is_none());
		let chapters = parse_chapter_page(&format!(r#"{{"items":[{good}]}}"#)).expect("page");
		assert!(chapters_to_store(false, chapters).is_none());
		let chapters = parse_chapter_page(&format!(r#"{{"items":[{good}]}}"#)).expect("page");
		assert_eq!(chapters_to_store(true, chapters).map(|c| c.len()), Some(1));
	}
}
