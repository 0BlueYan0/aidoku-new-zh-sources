use aidoku::{
	alloc::{vec, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::defaults_get,
		html::{Element, Html},
		net::Request,
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
	format!("{BASE_URL}/so/key/{}/{page}", encode_uri_component(query.trim()))
}

pub fn fetch_html(url: &str) -> Result<String> {
	Request::get(url)?.string()
}

/// The React Server Components payload of a page: the same data the HTML is rendered
/// from, as plain JSON rows instead of strings escaped inside `<script>` tags.
pub fn fetch_flight(url: &str) -> Result<String> {
	Request::get(url)?.header("RSC", "1").string()
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
/// when it failed both times, so a failure is not shown as an empty result.
pub fn fetch_checked(url: &str, fetch: fn(&str) -> Result<String>) -> Result<String> {
	let payload = fetch(url)?;
	if !site_failed(&payload) {
		return Ok(payload);
	}
	println!("[vomicmh] the site failed to build {url}, asking again");
	let payload = fetch(url)?;
	if site_failed(&payload) {
		bail!("[vomicmh] the site failed to build {url}");
	}
	Ok(payload)
}

/// A reader page's payload. The site renders the page list only for a request that
/// carries the login token as the `_token` cookie; a bearer header is not read there.
pub fn fetch_reader(manga_key: &str, key: &str, token: &str) -> Result<String> {
	Request::get(chapter_url(manga_key, key))?
		.header("RSC", "1")
		.header("Cookie", &format!("_token={token}"))
		.string()
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
	if ids.iter().all(|s: &&str| s.bytes().all(|b: u8| b.is_ascii_digit())) {
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
		status: json_string(obj, "process").map(|p: String| status_of(&p)).unwrap_or_default(),
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
	el.text().map(|t: String| String::from(t.trim())).unwrap_or_default()
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
/// oldest first. Works the site keeps to its own app have none on the web.
pub fn parse_chapters(payload: &str, manga_key: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = objects_with(payload, &["car_id", "img_num", "group_name"])
		.into_iter()
		.filter(|obj: &&str| json_field(obj, "car_id") == Some(manga_key))
		.filter_map(|obj: &str| {
			let key = String::from(json_field(obj, "id")?);
			let title = json_string(obj, "name").map(|t: String| String::from(t.trim()));
			let (chapter, volume) = title.as_deref().map(chapter_numbers).unwrap_or((None, None));
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
		.collect();
	chapters.reverse();
	chapters
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
pub fn parse_pages(payload: &str, original: bool) -> Vec<Page> {
	let Some((_, rest)) = payload.split_once("\"chapterData\":") else {
		return Vec::new();
	};
	let data = &rest[..value_end(rest.as_bytes(), 0)];
	json_field(data, "img_list")
		.map(json_items)
		.unwrap_or_default()
		.into_iter()
		.filter_map(|img: &str| json_string(img, "url"))
		.filter(|url: &String| url.starts_with("http"))
		.map(|url: String| Page {
			content: PageContent::url(if original { String::from(original_image(&url)) } else { url }),
			..Default::default()
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
		assert_eq!(chapter_url("27035", "25743"), "https://www.vomicmh.com/chapter/27035/25743");
		assert_eq!(ids_after("https://www.vomicmh.com/detail/27035", "detail"), vec!["27035"]);
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
	fn categories_match_the_site() {
		// The home page lists every category with its id.
		let home = include_str!("fixtures/home.flight");
		for (id, site, _) in CATEGORIES {
			assert!(
				home.contains(&format!("\"item\":{{\"id\":{id},\"name\":\"{site}\"")),
				"{id} {site}"
			);
		}
	}

	#[aidoku_test]
	fn parses_list_pages() {
		for (fixture, count) in [
			(include_str!("fixtures/home.flight"), 35),
			(include_str!("fixtures/latest.flight"), 12),
			(include_str!("fixtures/cate.flight"), 12),
			(include_str!("fixtures/search.flight"), 7),
			(include_str!("fixtures/cate_empty.flight"), 0),
		] {
			let entries = parse_manga_list(fixture);
			assert_eq!(entries.len(), count);
			for manga in &entries {
				assert!(manga.cover.as_deref().is_some_and(|c: &str| c.contains("sign=")), "{}", manga.title);
				assert_eq!(manga.url, Some(manga_url(&manga.key)));
			}
		}
		let search = parse_manga_list(include_str!("fixtures/search.flight"));
		let first = &search[0];
		assert_eq!(first.key, "27035");
		assert_eq!(first.title, "哪咤");
		assert_eq!(first.authors, Some(vec![String::from("藤奇")]));
		assert_eq!(first.status, MangaStatus::Ongoing);
		assert_eq!(first.viewer, Viewer::Webtoon);
		assert_eq!(
			first.tags,
			Some(vec![String::from("冒險"), String::from("動作"), String::from("奇幻")])
		);
		assert!(first.description.as_deref().is_some_and(|d: &str| d.starts_with("苍茫大地上")));
		// A page-by-page work, and a nested `id` that is not a work.
		let paged = parse_manga_list(
			"x:{\"id\":12418,\"name\":\"你喜欢哪对情侣？\",\"cover\":\"https://c\",\"process\":\"已完结\",\"dis_type\":\"页漫\",\"cat\":[{\"id\":13,\"name\":\"百合\"}]}",
		);
		assert_eq!(paged.len(), 1);
		assert_eq!(paged[0].viewer, Viewer::RightToLeft);
		assert_eq!(paged[0].status, MangaStatus::Completed);
		assert_eq!(paged[0].tags, Some(vec![String::from("百合")]));
	}

	#[aidoku_test]
	fn parses_details() {
		let manga = parse_details(include_str!("fixtures/detail.html"), "27035").expect("details");
		assert_eq!(manga.title, "哪咤");
		assert_eq!(manga.authors, Some(vec![String::from("藤奇")]));
		assert_eq!(
			manga.tags,
			Some(vec![String::from("冒險"), String::from("動作"), String::from("奇幻")])
		);
		assert_eq!(manga.status, MangaStatus::Ongoing);
		assert!(manga.cover.as_deref().is_some_and(|c: &str| c.contains("-cover?sign=")));
		assert!(manga.description.as_deref().is_some_and(|d: &str| d.starts_with("苍茫大地上")));
		assert_eq!(manga.url.as_deref(), Some("https://www.vomicmh.com/detail/27035"));
	}

	#[aidoku_test]
	fn tells_site_failures_from_empty_pages() {
		assert!(site_failed(include_str!("fixtures/cate_failed.flight")));
		assert!(site_failed(include_str!("fixtures/detail_failed.flight")));
		assert!(parse_manga_list(include_str!("fixtures/cate_failed.flight")).is_empty());
		assert!(parse_chapters(include_str!("fixtures/detail_failed.flight"), "27035").is_empty());
		for fixture in [
			include_str!("fixtures/home.flight"),
			include_str!("fixtures/latest.flight"),
			include_str!("fixtures/cate.flight"),
			include_str!("fixtures/cate_empty.flight"),
			include_str!("fixtures/search.flight"),
			include_str!("fixtures/detail.flight"),
			include_str!("fixtures/detail.html"),
			include_str!("fixtures/detail_app_only.flight"),
			include_str!("fixtures/detail_volumes.flight"),
			include_str!("fixtures/reader.flight"),
		] {
			assert!(!site_failed(fixture));
		}
	}

	#[aidoku_test]
	fn parses_pages() {
		let url = |page: &Page| match &page.content {
			PageContent::Url(url, _) => url.clone(),
			_ => String::new(),
		};
		let pages = parse_pages(include_str!("fixtures/reader.flight"), false);
		assert_eq!(pages.len(), 22);
		assert!(url(&pages[0]).starts_with("https://cdm.vomicer.com/27035/68d4e911030bd140a0c6a72569da49ee-1?sign="));
		assert!(url(&pages[0]).ends_with("&imageMogr2/thumbnail/800x/format/webp/quality/75"));
		assert!(url(&pages[21]).contains("-22?sign="));
		let original = parse_pages(include_str!("fixtures/reader.flight"), true);
		assert_eq!(original.len(), 22);
		assert!(!url(&original[0]).contains("imageMogr2"));
		assert!(url(&original[0]).contains("&t="));
		assert!(parse_pages(include_str!("fixtures/reader_bad_token.flight"), false).is_empty());
		assert!(parse_pages(include_str!("fixtures/reader_logged_out.flight"), false).is_empty());
		assert_eq!(
			original_image("https://cdm.vomicer.com/x-1?sign=a&t=1&imageMogr2/thumbnail/800x"),
			"https://cdm.vomicer.com/x-1?sign=a&t=1"
		);
		assert_eq!(original_image("https://cdm.vomicer.com/x-1?sign=a&t=1"), "https://cdm.vomicer.com/x-1?sign=a&t=1");
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
	fn parses_chapters() {
		let chapters = parse_chapters(include_str!("fixtures/detail.flight"), "27035");
		let keys: Vec<&str> = chapters.iter().map(|c: &Chapter| c.key.as_str()).collect();
		assert_eq!(keys, ["25740", "25741", "25742", "25743"]);
		assert_eq!(chapters[0].title.as_deref(), Some("3.2话"));
		assert_eq!(chapters[0].chapter_number, Some(3.2));
		assert_eq!(chapters[3].url.as_deref(), Some("https://www.vomicmh.com/chapter/27035/25743"));
		// 2024-11-18 05:42:13 China time.
		assert_eq!(chapters[3].date_uploaded, Some(1731879733));

		let volumes = parse_chapters(include_str!("fixtures/detail_volumes.flight"), "56100");
		assert_eq!(volumes.len(), 36);
		assert_eq!(volumes[0].title.as_deref(), Some("27话"));
		let find = |title: &str| volumes.iter().find(|c: &&Chapter| c.title.as_deref() == Some(title)).unwrap();
		assert_eq!((find("2卷").chapter_number, find("2卷").volume_number), (None, Some(2.0)));
		assert_eq!((find("第3卷番外").chapter_number, find("第3卷番外").volume_number), (None, None));
		assert_eq!(find("14话").chapter_number, Some(14.0));

		assert!(parse_chapters(include_str!("fixtures/detail_app_only.flight"), "19701").is_empty());
	}
}
