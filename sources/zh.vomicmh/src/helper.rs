use aidoku::{
	alloc::{vec, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::defaults_get,
		html::{Element, Html},
		net::{Request, Response},
		std::parse_date,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaStatus, Page, PageContent, Result, Viewer,
};

pub const BASE_URL: &str = "https://www.vomicmh.com";

/// The site's dates are China time without a zone.
const SITE_UTC_OFFSET: i64 = 8 * 3600;

/// The site's categories: id, the site's (simplified) name, and the name shown in the
/// app, which is also the option in `res/filters.json`.
pub const CATEGORIES: [(&str, &str, &str); 37] = [
	("4", "冒险", "冒險"),
	("5", "搞笑", "搞笑"),
	("6", "动作", "動作"),
	("7", "科幻", "科幻"),
	("8", "爱情", "愛情"),
	("9", "侦探", "偵探"),
	("10", "竞技", "競技"),
	("11", "魔法", "魔法"),
	("12", "校园", "校園"),
	("13", "百合", "百合"),
	("14", "耽美", "耽美"),
	("15", "历史", "歷史"),
	("16", "战争", "戰爭"),
	("17", "宅系", "宅系"),
	("18", "治愈", "治癒"),
	("20", "武侠", "武俠"),
	("21", "职场", "職場"),
	("22", "神鬼", "神鬼"),
	("23", "奇幻", "奇幻"),
	("24", "生活", "生活"),
	("25", "其他", "其他"),
	("26", "热血", "熱血"),
	("27", "古风", "古風"),
	("28", "悬疑", "懸疑"),
	("29", "都市", "都市"),
	("30", "架空", "架空"),
	("31", "青春", "青春"),
	("32", "剧情", "劇情"),
	("34", "犯罪", "犯罪"),
	("35", "致郁", "致鬱"),
	("36", "纯爱", "純愛"),
	("37", "恋爱", "戀愛"),
	("38", "体育", "體育"),
	("39", "末世", "末世"),
	("40", "少女", "少女"),
	("41", "重生", "重生"),
	("43", "美食", "美食"),
];

/// The app's name for a category the site names `name`; names outside the table pass through.
pub fn category_display(name: &str) -> String {
	CATEGORIES
		.iter()
		.find(|(_, site, _)| *site == name)
		.map(|(_, _, shown)| String::from(*shown))
		.unwrap_or_else(|| String::from(name))
}

/// The id of a category by the name the app shows (a tapped tag) or the site's name.
pub fn category_id(name: &str) -> Option<&'static str> {
	CATEGORIES
		.iter()
		.find(|(_, site, shown)| *shown == name || *site == name)
		.map(|(id, _, _)| *id)
}

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/detail/{key}")
}

pub fn chapter_url(manga_key: &str, key: &str) -> String {
	format!("{BASE_URL}/chapter/{manga_key}/{key}")
}

/// A category's works, newest update first. Category `0` is the whole site.
pub fn category_url(id: &str, page: i32) -> String {
	format!("{BASE_URL}/so/cate/{id}/{page}")
}

/// The search matches titles and authors loosely and does not say when it runs out.
pub fn search_url(query: &str, page: i32) -> String {
	format!(
		"{BASE_URL}/so/key/{}/{page}",
		encode_uri_component(query.trim())
	)
}

pub fn fetch_html(url: &str) -> Result<String> {
	read_body(Request::get(url)?.send()?, url)
}

/// The body of a response the site answered with 2xx. A 502 or 503 page has neither the
/// site's own failure wording nor its works, so read as a page it would pass for an empty
/// list or a work without chapters.
pub fn read_body(response: Response, url: &str) -> Result<String> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[vomicmh] HTTP {status} for {url}");
	}
	response.get_string()
}

/// The React Server Components payload of a page: the same data the HTML is rendered
/// from, as plain JSON rows instead of strings escaped inside `<script>` tags.
pub fn fetch_flight(url: &str) -> Result<String> {
	read_body(flight_request(url)?.send()?, url)
}

/// Build-only half of `fetch_flight`, so `get_home` can batch rows with `send_all`.
pub fn flight_request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?.header("RSC", "1"))
}

/// Whether the site's server failed to load the data behind a page. It does so in bursts
/// of 10 to 20 seconds and still answers 200: a details page then says so in words, and a
/// list page comes back with no works and no category bar (`"children":"$undefined"`),
/// unlike a list that has simply run out.
pub fn site_failed(payload: &str) -> bool {
	if payload.contains("服务器出小差") {
		return true;
	}
	payload
		.split_once("\"children\":\"分类\"}]")
		.and_then(|(_, rest)| rest.split_once("\"children\":"))
		.is_some_and(|(_, value)| value.starts_with("\"$undefined\""))
}

/// A page fetched with `fetch`, asked once more when the site failed to build it. `Err`
/// when it failed both times, or when either request fails or is answered with a
/// non-2xx status (not asked again), so a failure is not shown as an empty result.
pub fn fetch_checked(url: &str, fetch: fn(&str) -> Result<String>) -> Result<String> {
	let payload = fetch(url)?;
	if !site_failed(&payload) {
		return Ok(payload);
	}
	let payload = fetch(url)?;
	if site_failed(&payload) {
		bail!("[vomicmh] the site failed to build {url}");
	}
	Ok(payload)
}

/// A reader page's payload. The site renders the page list only for a request that
/// carries the login token as the `_token` cookie; a bearer header is not read there.
pub fn fetch_reader(manga_key: &str, key: &str, token: &str) -> Result<String> {
	let url = chapter_url(manga_key, key);
	let response = Request::get(&url)?
		.header("RSC", "1")
		.header("Cookie", &format!("_token={token}"))
		.send()?;
	read_body(response, &url)
}

/// `original` drops the site's resize-and-WebP suffix from page addresses.
pub fn wants_original_images() -> bool {
	defaults_get::<String>("image_quality").as_deref() == Some("original")
}

/// `/detail/27035` → `27035`; `/chapter/27035/25743` → (`27035`, `25743`).
pub fn ids_after<'a>(url: &'a str, kind: &str) -> Vec<&'a str> {
	let Some((_, rest)) = url.split_once(&format!("/{kind}/")) else {
		return Vec::new();
	};
	let rest = rest.split(['?', '#']).next().unwrap_or("");
	let ids: Vec<&str> = rest.split('/').filter(|s: &&str| !s.is_empty()).collect();
	if ids
		.iter()
		.all(|s: &&str| s.bytes().all(|b: u8| b.is_ascii_digit()))
	{
		ids
	} else {
		Vec::new()
	}
}

// ---------------------------------------------------------------------------
// JSON scanning (as in zh.hipmh): only top-level fields of an object are read, so a
// nested `id` or `name` never answers for the object itself.

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
	while i < bytes.len() && bytes[i].is_ascii_whitespace() {
		i += 1;
	}
	i
}

/// `i` points at an opening quote; returns the index just past the closing quote.
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

/// Every object in a page payload that starts with an `id` and has all of `fields` at
/// its top level, first occurrence of each id only. The payload splits its data into
/// rows that refer to each other, and a text row is not newline-terminated, so the
/// objects are found by their opening bytes rather than by row.
fn objects_with<'a>(payload: &'a str, fields: &[&str]) -> Vec<&'a str> {
	let bytes = payload.as_bytes();
	let mut out: Vec<&str> = Vec::new();
	let mut ids: Vec<&str> = Vec::new();
	let mut from = 0;
	while let Some(offset) = payload[from..].find("{\"id\":") {
		let start = from + offset;
		from = start + 1;
		let end = value_end(bytes, start);
		let obj = &payload[start..end];
		let Some(id) = json_field(obj, "id") else {
			continue;
		};
		if fields.iter().all(|f: &&str| json_field(obj, f).is_some()) && !ids.contains(&id) {
			ids.push(id);
			out.push(obj);
		}
	}
	out
}

// ---------------------------------------------------------------------------
// Works

fn status_of(process: &str) -> MangaStatus {
	match process.trim() {
		"连载中" => MangaStatus::Ongoing,
		"已完结" => MangaStatus::Completed,
		_ => MangaStatus::Unknown,
	}
}

/// A work as the list pages describe it.
fn parse_manga(obj: &str) -> Option<Manga> {
	let key = String::from(json_field(obj, "id")?);
	let title = json_string(obj, "name")?;
	let tags: Vec<String> = json_field(obj, "cat")
		.map(json_items)
		.unwrap_or_default()
		.into_iter()
		.filter_map(|cat: &str| json_string(cat, "name"))
		.map(|name: String| category_display(name.trim()))
		.collect();
	Some(Manga {
		url: Some(manga_url(&key)),
		cover: json_string(obj, "cover"),
		authors: json_string(obj, "author").map(|a: String| vec![String::from(a.trim())]),
		description: json_string(obj, "intro").map(|d: String| String::from(d.trim())),
		tags: (!tags.is_empty()).then_some(tags),
		status: json_string(obj, "process")
			.map(|p: String| status_of(&p))
			.unwrap_or_default(),
		content_rating: ContentRating::Safe,
		viewer: match json_string(obj, "dis_type").as_deref() {
			Some("页漫") => Viewer::RightToLeft,
			_ => Viewer::Webtoon,
		},
		key,
		title,
		..Default::default()
	})
}

/// The works on a list page (the home page, a category, a search), in page order.
pub fn parse_manga_list(payload: &str) -> Vec<Manga> {
	objects_with(payload, &["cover", "dis_type"])
		.into_iter()
		.filter_map(parse_manga)
		.collect()
}

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// The status in the page's structured data, which the payload carries as
/// `"creativeWorkStatus": 连载中,` (not valid JSON, and escaped once more in the HTML).
fn structured_status(html: &str) -> MangaStatus {
	let Some((_, rest)) = html.split_once("creativeWorkStatus") else {
		return MangaStatus::Unknown;
	};
	let rest = rest.trim_start_matches(['\\', '"', ':', ' ']);
	let value = rest.split([',', '\\', '\n']).next().unwrap_or("");
	status_of(value)
}

/// The details page. Its viewer is not on the page; nearly every work on the site is a
/// vertical strip, so that is the default.
pub fn parse_details(html: &str, key: &str) -> Option<Manga> {
	let doc = Html::parse_with_url(html, BASE_URL).ok()?;
	let title = doc
		.select_first(".detail .text-nowrap")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = doc
		.select_first(".detail-content img[alt=cover]")
		.and_then(|el: Element| el.attr("src"));
	let mut authors: Option<Vec<String>> = None;
	let mut tags: Vec<String> = Vec::new();
	for row in doc.select(".detail .text-base").into_iter().flatten() {
		let label = row
			.select_first("span.text-sm")
			.map(|el: Element| text_of(&el))
			.unwrap_or_default();
		if label.starts_with("作者") {
			authors = row
				.select_first("span.text-xs")
				.map(|el: Element| text_of(&el))
				.filter(|a: &String| !a.is_empty())
				.map(|a: String| vec![a]);
		} else if label.starts_with("分类") {
			for link in row.select("a[href*=\"/so/cate/\"]").into_iter().flatten() {
				let name = category_display(&text_of(&link));
				if !name.is_empty() && !tags.contains(&name) {
					tags.push(name);
				}
			}
		}
	}
	let description = doc
		.select_first(".detail .line-clamp-4 span.text-xs")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		title,
		cover,
		authors,
		description,
		tags: (!tags.is_empty()).then_some(tags),
		status: structured_status(html),
		content_rating: ContentRating::Safe,
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

// ---------------------------------------------------------------------------
// Chapters

/// `12话` / `第12话` → (Some(12), None), `第3卷` → (None, Some(3)). Extras such as
/// `3卷番外` or `1卷加笔` carry no number.
pub fn chapter_numbers(title: &str) -> (Option<f32>, Option<f32>) {
	let rest = title.trim();
	let rest = rest.strip_prefix('第').unwrap_or(rest);
	let end = rest
		.find(|c: char| !(c.is_ascii_digit() || c == '.'))
		.unwrap_or(rest.len());
	let Ok(number) = rest[..end].parse::<f32>() else {
		return (None, None);
	};
	let mut after = rest[end..].chars();
	match (after.next(), after.next()) {
		(Some('话' | '話' | '回'), _) => (Some(number), None),
		(Some('卷'), None) => (None, Some(number)),
		_ => (None, None),
	}
}

/// `2024-11-18 05:42:13` in China time → Unix seconds.
fn site_date(raw: &str) -> Option<i64> {
	parse_date(raw, "yyyy-MM-dd HH:mm:ss").map(|t: i64| t - SITE_UTC_OFFSET)
}

/// A work's chapters from its details page payload, newest first. The site lists them
/// oldest first. Works the site keeps to its own app have none on the web, so an empty
/// list is a real answer; `None` when the payload is not this work's details page (it
/// lacks the work's chapter list entry) or when one of this work's chapters has no usable id, so
/// the library keeps its chapters instead of a shortened list.
pub fn parse_chapters(payload: &str, manga_key: &str) -> Option<Vec<Chapter>> {
	if !payload.contains(&format!("\"id\":\"{manga_key}\",\"list\":")) {
		return None;
	}
	let chapters = objects_with(payload, &["car_id", "img_num", "group_name"])
		.into_iter()
		.rev()
		.filter(|obj: &&str| json_field(obj, "car_id") == Some(manga_key))
		.map(|obj: &str| {
			// `objects_with` only yields objects with an `id`, so the failure to catch is
			// one that is not a chapter id (`null`, a string), not a missing field.
			let key = json_field(obj, "id")
				.filter(|id: &&str| !id.is_empty() && id.bytes().all(|b: u8| b.is_ascii_digit()))
				.map(String::from)?;
			let title = json_string(obj, "name").map(|t: String| String::from(t.trim()));
			let (chapter, volume) = title
				.as_deref()
				.map(chapter_numbers)
				.unwrap_or((None, None));
			// Only the `单行本` group holds volumes; elsewhere `第3卷` would number an extra.
			let is_volume = json_string(obj, "group_name").as_deref() == Some("单行本");
			Some(Chapter {
				url: Some(chapter_url(manga_key, &key)),
				chapter_number: if is_volume { None } else { chapter },
				volume_number: if is_volume { volume } else { None },
				date_uploaded: json_string(obj, "chapter_updated_at")
					.or_else(|| json_string(obj, "created_at"))
					.and_then(|d: String| site_date(&d)),
				key,
				title,
				..Default::default()
			})
		})
		.collect::<Option<Vec<Chapter>>>()?;
	Some(chapters)
}

// ---------------------------------------------------------------------------
// Pages

/// `…-5?sign=…&t=…&imageMogr2/thumbnail/800x/format/webp/quality/75` → the same address
/// without the processing suffix, which is the uploaded JPEG. The signature covers the
/// path only, so it still holds.
pub fn original_image(url: &str) -> &str {
	url.split("&imageMogr2").next().unwrap_or(url)
}

/// The page images of a reader page. Empty when the page was rendered without a valid
/// login: the site then sends `"chapterData":{}`, even for a token it does not accept.
/// `Err` when an entry has no usable address, which would otherwise shorten the chapter.
pub fn parse_pages(payload: &str, original: bool) -> Result<Vec<Page>> {
	let Some((_, rest)) = payload.split_once("\"chapterData\":") else {
		return Ok(Vec::new());
	};
	let data = &rest[..value_end(rest.as_bytes(), 0)];
	json_field(data, "img_list")
		.map(json_items)
		.unwrap_or_default()
		.into_iter()
		.enumerate()
		.map(|(index, img)| match json_string(img, "url") {
			Some(url) if url.starts_with("http") => Ok(Page {
				content: PageContent::url(if original {
					String::from(original_image(&url))
				} else {
					url
				}),
				..Default::default()
			}),
			_ => Err(error!("[vomicmh] page {} has no image address", index + 1)),
		})
		.collect()
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn builds_addresses() {
		assert_eq!(category_url("0", 2), "https://www.vomicmh.com/so/cate/0/2");
		assert_eq!(
			search_url(" 哪咤 ", 1),
			"https://www.vomicmh.com/so/key/%E5%93%AA%E5%92%A4/1"
		);
		assert_eq!(
			chapter_url("27035", "25743"),
			"https://www.vomicmh.com/chapter/27035/25743"
		);
		assert_eq!(
			ids_after("https://www.vomicmh.com/detail/27035", "detail"),
			vec!["27035"]
		);
		assert_eq!(
			ids_after("https://www.vomicmh.com/chapter/27035/25743?x=1", "chapter"),
			vec!["27035", "25743"]
		);
		assert!(ids_after("https://www.vomicmh.com/so/cate/4/1", "detail").is_empty());
		assert!(ids_after("https://www.vomicmh.com/detail/abc", "detail").is_empty());
	}

	#[aidoku_test]
	fn categories_match_filters_json() {
		let json = include_str!("../res/filters.json");
		for (id, _, shown) in CATEGORIES {
			assert!(json.contains(&format!("\"{shown}\"")), "{shown}");
			assert!(json.contains(&format!("\"{id}\"")), "{id}");
		}
		assert_eq!(category_id("冒險"), Some("4"));
		assert_eq!(category_id("冒险"), Some("4"));
		assert_eq!(category_id("不存在"), None);
		assert_eq!(category_display("治愈"), "治癒");
		assert_eq!(category_display("新分类"), "新分类");
	}

	#[aidoku_test]
	fn numbers_chapters() {
		assert_eq!(chapter_numbers("1话"), (Some(1.0), None));
		assert_eq!(chapter_numbers("3.2话"), (Some(3.2), None));
		assert_eq!(chapter_numbers("第12话"), (Some(12.0), None));
		assert_eq!(chapter_numbers("2卷"), (None, Some(2.0)));
		assert_eq!(chapter_numbers("第3卷番外"), (None, None));
		assert_eq!(chapter_numbers("1卷加笔"), (None, None));
		assert_eq!(chapter_numbers("短篇"), (None, None));
		assert_eq!(chapter_numbers("全一话"), (None, None));
	}

	#[aidoku_test]
	fn a_chapter_without_an_id_rejects_the_list() {
		let page = |second: &str| {
			format!(
				r#"{{"id":"27035","list":[]}}{{"id":1,"car_id":27035,"img_num":3,"group_name":"正篇","name":"1话"}}{second}"#
			)
		};
		let good = page(r#"{"id":2,"car_id":27035,"img_num":3,"group_name":"正篇","name":"2话"}"#);
		assert_eq!(parse_chapters(&good, "27035").map(|c| c.len()), Some(2));
		let bad =
			page(r#"{"id":null,"car_id":27035,"img_num":3,"group_name":"正篇","name":"2话"}"#);
		assert!(parse_chapters(&bad, "27035").is_none());
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
