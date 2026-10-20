use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		html::{Document, Element},
		net::{Request, Response},
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, Result, Viewer,
};

/// The site only answers over http: its certificate expired (2026-10-13). The app allows
/// http (`NSAllowsArbitraryLoads`).
pub const BASE_URL: &str = "http://yumanhua.com";

/// The paged lists (`/data/sort`, `/data/rank`) give 20 works a page and answer
/// `{"code":"-1"}` from page 6 on (2026-10-13).
pub const PAGE_SIZE: usize = 20;
pub const MAX_PAGES: i32 = 5;

/// The listings: id and the `/data/rank` (or `/rank/N`) number behind it.
/// Ranks 4 and 5 are only served as one page of HTML; the API answers -1 for them.
pub const LISTINGS: [(&str, &str); 6] = [
	("rank1", "1"),
	("rank2", "2"),
	("rank3", "3"),
	("rank4", "4"),
	("rank5", "5"),
	("rank6", "6"),
];
pub const HTML_RANKS: [&str; 2] = ["4", "5"];

/// The categories of `/sort/N`, in the order of `res/filters.json`. A tapped tag arrives
/// as the site's name and is looked up here.
pub const CATEGORIES: [(&str, &str); 16] = [
	("冒险", "1"),
	("热血", "2"),
	("都市", "3"),
	("玄幻", "4"),
	("悬疑", "5"),
	("耽美", "6"),
	("恋爱", "7"),
	("生活", "8"),
	("搞笑", "9"),
	("穿越", "10"),
	("修真", "11"),
	("后宫", "12"),
	("女主", "13"),
	("古风", "14"),
	("连载", "15"),
	("完结", "16"),
];

pub fn category_id(name: &str) -> Option<&'static str> {
	CATEGORIES
		.iter()
		.find(|(n, _): &&(&str, &str)| *n == name)
		.map(|(_, id): &(&str, &str)| *id)
}

/// The home page's section headings, as the site writes them and as shown.
pub const HOME_TITLES: [(&str, &str); 5] = [
	("精选漫画", "精選漫畫"),
	("今日推荐", "今日推薦"),
	("人气漫画", "人氣漫畫"),
	("新漫上架", "新漫上架"),
	("完结·一次爽翻天", "完結・一次爽翻天"),
];

/// Tags that raise a work's rating. The site has no adult flag and none of its categories
/// is adult, so the tags are the only evidence; a work without one is `Safe`. Chosen by the
/// user (2026-10-13), the names zh.wmh1234 uses that this site has.
pub const SUGGESTIVE_TAGS: [&str; 2] = ["耽美", "后宫"];

pub fn content_rating<S: AsRef<str>>(tags: &[S]) -> ContentRating {
	if tags
		.iter()
		.any(|t: &S| SUGGESTIVE_TAGS.contains(&t.as_ref()))
	{
		ContentRating::Suggestive
	} else {
		ContentRating::Safe
	}
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Send a request, and send it once more if it got no answer. The site often leaves a
/// request unanswered for 30 seconds or more (3 of 7 in one test, 2026-10-13); the user
/// chose one retry. Only a failed send is retried: an HTTP answer, even an error, is final.
/// The retry runs after the first request has ended, so it holds no other request's slot.
fn send_once_more(build: impl Fn() -> Result<Request>, what: &str) -> Result<Response> {
	match build()?.send() {
		Ok(response) => Ok(response),
		Err(error) => {
			println!("[yumanhua] no answer for {what} ({error:?}), asking again");
			Ok(build()?.send()?)
		}
	}
}

/// A page. A non-2xx answer is an error, so an error page is never read as an empty list.
pub fn fetch_html(url: &str) -> Result<Document> {
	let response = send_once_more(|| Ok(Request::get(url)?), url)?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[yumanhua] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
}

/// The `data` array of a form POST to one of the site's JSON endpoints, or `None` when the
/// site says there is nothing (`-1`: past the last page, `201`: no search result).
pub fn post_list(path: &str, form: &str) -> Result<Option<String>> {
	post_list_with(path, form, true)
}

/// `post_list` with the retry for an unanswered request optional. The app runs the blank
/// search by itself whenever content reloads; two unanswered sends would hold the source for
/// about two minutes (zh.bakamh2 hit the same on device, 2026-10-16), so that one is sent once.
pub fn post_list_with(path: &str, form: &str, retry: bool) -> Result<Option<String>> {
	let url = format!("{BASE_URL}{path}");
	let build = || {
		Ok(Request::post(&url)?
			.header(
				"Content-Type",
				"application/x-www-form-urlencoded; charset=UTF-8",
			)
			.body(form.as_bytes()))
	};
	let response = if retry {
		send_once_more(build, &url)?
	} else {
		build()?.send()?
	};
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[yumanhua] HTTP {status} for {path} {form}");
	}
	let body = response.get_string()?;
	match json_string(&body, "code").as_deref() {
		Some("200") => Ok(json_field(&body, "data").map(String::from)),
		Some("-1") | Some("201") => Ok(None),
		code => bail!("[yumanhua] {path} {form} answered code {code:?}"),
	}
}

pub fn sort_form(category: &str, page: i32) -> String {
	format!("s={category}&p={page}")
}

pub fn search_form(query: &str) -> String {
	format!("k={}", encode_uri_component(query))
}

// ---------------------------------------------------------------------------
// Hand-rolled JSON scanning (no serde: keeps the wasm binary small). Copied from
// zh.hipmh: these only look at the fields of the object they are given.
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
		.map(|s: String| String::from(s.trim()))
		.filter(|s: &String| !s.is_empty())
}

/// The string elements of a top-level JSON array, e.g. the decoded image list.
pub fn json_top_strings(array: &str) -> Vec<String> {
	json_items(array)
		.into_iter()
		.filter_map(as_string)
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

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/{key}/")
}

pub fn chapter_url(manga_key: &str, key: &str) -> String {
	format!("{BASE_URL}/{manga_key}/{key}.html")
}

fn is_id(segment: &str) -> bool {
	!segment.is_empty() && segment.bytes().all(|b: u8| b.is_ascii_alphanumeric())
}

/// The path of a link on the site, without scheme, host, query or fragment.
fn site_path(href: &str) -> &str {
	let href = href.trim();
	let href = match href.find("://") {
		Some(i) => {
			let rest = &href[i + 3..];
			rest.find('/').map(|j: usize| &rest[j..]).unwrap_or("/")
		}
		None => href,
	};
	href.split(['?', '#']).next().unwrap_or_default()
}

/// The site's own pages and endpoints, which share the shape of a work's address
/// (`/rank/`, `/s`). Work ids are not checked by length: only 7-letter ones were seen.
const SITE_PATHS: [&str; 9] = [
	"rank",
	"sort",
	"data",
	"static",
	"user",
	"s",
	"login",
	"register",
	"morechapter",
];

/// `/trsKtGi/` → `trsKtGi`; a shared address may lack the final slash. Site pages such as
/// `/rank/1` and `/user.html` are not works.
pub fn manga_key(href: &str) -> Option<String> {
	let path = site_path(href).strip_prefix('/')?;
	let segment = path.strip_suffix('/').unwrap_or(path);
	(is_id(segment) && !SITE_PATHS.contains(&segment)).then(|| String::from(segment))
}

/// `/trsKtGi/IVpdIdWF.html` → (`trsKtGi`, `IVpdIdWF`).
pub fn chapter_keys(href: &str) -> Option<(String, String)> {
	let path = site_path(href);
	let (manga, chapter) = path.strip_prefix('/')?.split_once('/')?;
	let chapter = chapter.strip_suffix(".html")?;
	(is_id(manga) && is_id(chapter) && !SITE_PATHS.contains(&manga))
		.then(|| (String::from(manga), String::from(chapter)))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// Covers are lazily loaded: `src` is a placeholder and `data-src` the image.
fn lazy_src(el: &Element) -> Option<String> {
	el.attr("data-src")
		.map(|s: String| String::from(s.trim()))
		.filter(|s: &String| s.starts_with("http"))
}

/// A list card. List entries carry no tags, so they are `Safe` until the details load.
fn card(key: String, title: String, cover: Option<String>, authors: Vec<String>) -> Manga {
	Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		content_rating: ContentRating::Safe,
		viewer: Viewer::Webtoon,
		..Default::default()
	}
}

/// `Fleecy cloud · 2love/SHYATAN` → three names. The site mixes authors, studios and
/// publishers in one field.
/// Only a spaced `·` separates names; `J·K·罗琳` is one name.
pub fn split_authors(text: &str) -> Vec<String> {
	text.replace(" · ", "/")
		.split(['/', '／', '、', ',', '，', '&'])
		.map(|n: &str| String::from(n.trim()))
		.filter(|n: &String| !n.is_empty())
		.collect()
}

/// The `data` array of `/data/sort`, `/data/rank` or `/s`.
pub fn parse_json_list(data: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	for item in json_items(data) {
		let (Some(key), Some(title)) = (json_string(item, "id"), json_string(item, "name")) else {
			continue;
		};
		if !is_id(&key) || entries.iter().any(|m: &Manga| m.key == key) {
			continue;
		}
		let cover = json_string(item, "imgurl").filter(|s: &String| s.starts_with("http"));
		let authors = json_string(item, "author")
			.map(|a: String| split_authors(&a))
			.unwrap_or_default();
		entries.push(card(key, title, cover, authors));
	}
	entries
}

/// A page of a paged list from its `data` array. The site stops at page 5 whatever the
/// list's length. A full page is judged by the site's count, before duplicates are dropped.
pub fn page_result(data: &str, page: i32) -> MangaPageResult {
	MangaPageResult {
		has_next_page: json_items(data).len() >= PAGE_SIZE && page < MAX_PAGES,
		entries: parse_json_list(data),
	}
}

/// The home page's sections: displayed heading and works.
pub fn parse_home(html: &Document) -> Vec<(String, Vec<Manga>)> {
	let mut sections = Vec::new();
	for section in html.select(".mults").into_iter().flatten() {
		let Some(heading) = section
			.select_first(".mult-head")
			.map(|el: Element| text_of(&el))
		else {
			continue;
		};
		let title = HOME_TITLES
			.iter()
			.find(|(site, _): &&(&str, &str)| *site == heading)
			.map(|(_, shown): &(&str, &str)| String::from(*shown))
			.unwrap_or(heading);
		let mut entries: Vec<Manga> = Vec::new();
		for link in section
			.select(".mult-body li a[href]")
			.into_iter()
			.flatten()
		{
			let Some(key) = link.attr("href").and_then(|h: String| manga_key(&h)) else {
				continue;
			};
			let name = link
				.select_first(".card-title")
				.map(|el: Element| text_of(&el))
				.or_else(|| link.attr("title"))
				.unwrap_or_default();
			if name.is_empty() || entries.iter().any(|m: &Manga| m.key == key) {
				continue;
			}
			let cover = link
				.select_first("img")
				.and_then(|img: Element| lazy_src(&img));
			entries.push(card(key, name, cover, Vec::new()));
		}
		sections.push((title, entries));
	}
	sections
}

/// `/rank/4` and `/rank/5`: one page of 50 works.
pub fn parse_rank_html(html: &Document) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	for item in html.select(".rank-list > li").into_iter().flatten() {
		let Some(link) = item.select_first(".simple-info a[href]") else {
			continue;
		};
		let Some(key) = link.attr("href").and_then(|h: String| manga_key(&h)) else {
			continue;
		};
		let title = link
			.select_first("h2")
			.map(|el: Element| text_of(&el))
			.unwrap_or_default();
		if title.is_empty() || entries.iter().any(|m: &Manga| m.key == key) {
			continue;
		}
		// The first `<p>` is the author, the second the latest chapter.
		let authors = link
			.select_first("p")
			.map(|el: Element| split_authors(&text_of(&el)))
			.unwrap_or_default();
		let cover = item
			.select_first("img.cartoon-poster")
			.and_then(|img: Element| lazy_src(&img));
		entries.push(card(key, title, cover, authors));
	}
	entries
}

/// The details page has no publication status, and its `更新：06月13日` has no year, so
/// neither is filled.
pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first("h1.name")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first(".thumbnail img")
		.and_then(|img: Element| lazy_src(&img));
	let tags: Vec<String> = html
		.select(".comic-tags span")
		.into_iter()
		.flatten()
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())
		.collect();
	let authors = html
		.select(".comic-info-detail p")
		.into_iter()
		.flatten()
		.map(|el: Element| text_of(&el))
		.find_map(|t: String| t.strip_prefix("作者：").map(split_authors))
		.unwrap_or_default();
	let description = html
		.select_first(".cartoon-introduction")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		content_rating: content_rating(&tags),
		viewer: Viewer::Webtoon,
		tags: (!tags.is_empty()).then_some(tags),
		..Default::default()
	})
}

/// `第177话（完）` → 177, `080 玩物丧志` → 80, `702 701-2 漩涡中的旋风②` → 702. `插图` gets none.
pub fn chapter_number(title: &str) -> Option<f32> {
	let title = title.trim();
	if let Some(rest) = title.strip_prefix('第') {
		let end = rest.find(['话', '話', '回', '章'])?;
		return rest[..end].trim().parse::<f32>().ok();
	}
	let number = title.split_whitespace().next()?;
	number.parse::<f32>().ok()
}

/// `第72卷` → 72. Some works list their volumes after the chapters (火影忍者).
pub fn volume_number(title: &str) -> Option<f32> {
	let rest = title.trim().strip_prefix('第')?;
	let end = rest.find('卷')?;
	rest[..end].trim().parse::<f32>().ok()
}

fn chapter(manga_key: &str, key: String, title: String) -> Chapter {
	Chapter {
		url: Some(chapter_url(manga_key, &key)),
		chapter_number: chapter_number(&title),
		volume_number: volume_number(&title),
		title: (!title.is_empty()).then_some(title),
		key,
		..Default::default()
	}
}

/// Whether the details page lists only its first 20 chapters and loads the rest from
/// `/morechapter`.
pub fn has_more_chapters(html: &Document) -> bool {
	html.select_first(".chaplist-more").is_some()
}

/// The chapters on the details page, newest first, as the app wants.
pub fn parse_page_chapters(html: &Document, manga_key: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();
	for link in html
		.select(".chaplist-box li a[href]")
		.into_iter()
		.flatten()
	{
		let Some((manga, key)) = link.attr("href").and_then(|h: String| chapter_keys(&h)) else {
			continue;
		};
		if manga != manga_key || chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		chapters.push(chapter(manga_key, key, text_of(&link)));
	}
	chapters
}

/// The rest of the chapters from `/morechapter`, appended in the site's order. Titles and
/// order are the site's: some works list two runs (`702 …` then `第710话 …`).
pub fn append_more_chapters(chapters: &mut Vec<Chapter>, data: &str, manga_key: &str) {
	for item in json_items(data) {
		let Some(key) = json_string(item, "chapterid").filter(|k: &String| is_id(k)) else {
			continue;
		};
		if chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		let title = json_string(item, "chaptername").unwrap_or_default();
		chapters.push(chapter(manga_key, key, title));
	}
}

/// The reader page's inline scripts and the `data-id` that picks the image key.
pub fn reader_parts(html: &Document) -> (Vec<String>, Option<usize>) {
	let scripts = html
		.select("script")
		.into_iter()
		.flatten()
		// The test runner answers `data()` with `None` for `<script>`.
		.filter_map(|el: Element| el.data().or_else(|| el.html()))
		.collect();
	let key_index = html
		.select_first(".readerContainer")
		.and_then(|el: Element| el.attr("data-id"))
		.and_then(|id: String| id.trim().parse::<usize>().ok());
	(scripts, key_index)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, BASE_URL).expect("parse")
	}

	#[aidoku_test]
	fn reads_keys() {
		assert_eq!(manga_key("/trsKtGi/").as_deref(), Some("trsKtGi"));
		assert_eq!(
			manga_key("http://yumanhua.com/trsKtGi/?a=1").as_deref(),
			Some("trsKtGi")
		);
		assert_eq!(manga_key("/rank/1"), None);
		assert_eq!(manga_key("http://yumanhua.com/rank/"), None);
		assert_eq!(manga_key("/sort/"), None);
		assert_eq!(
			manga_key("http://yumanhua.com/trsKtGi").as_deref(),
			Some("trsKtGi")
		);
		assert_eq!(manga_key("/rank"), None);
		assert_eq!(manga_key("http://yumanhua.com/s"), None);
		assert_eq!(chapter_keys("/rank/1.html"), None);
		assert_eq!(manga_key("/user.html"), None);
		assert_eq!(manga_key("/"), None);
		assert_eq!(
			chapter_keys("http://www.yumanhua.com/trsKtGi/IVpdIdWF.html"),
			Some((String::from("trsKtGi"), String::from("IVpdIdWF")))
		);
		assert_eq!(chapter_keys("/trsKtGi/"), None);
		assert_eq!(chapter_keys("/static/js/all.js"), None);
		assert_eq!(manga_url("trsKtGi"), "http://yumanhua.com/trsKtGi/");
		assert_eq!(
			chapter_url("trsKtGi", "IVpdIdWF"),
			"http://yumanhua.com/trsKtGi/IVpdIdWF.html"
		);
		assert_eq!(
			search_form("斗破 苍穹"),
			"k=%E6%96%97%E7%A0%B4%20%E8%8B%8D%E7%A9%B9"
		);
		assert_eq!(sort_form("6", 2), "s=6&p=2");
	}

	#[aidoku_test]
	fn splits_authors() {
		assert_eq!(
			split_authors("Fleecy cloud · 2love/SHYATAN"),
			["Fleecy cloud", "2love", "SHYATAN"]
		);
		assert_eq!(split_authors("J·K·罗琳"), ["J·K·罗琳"]);
		assert_eq!(
			split_authors("DUBU（REDICE STUDIO)"),
			["DUBU（REDICE STUDIO)"]
		);
	}

	#[aidoku_test]
	fn numbers_chapters() {
		assert_eq!(chapter_number("第177话（完）"), Some(177.0));
		assert_eq!(chapter_number("第281回 弟子白衣?！"), Some(281.0));
		assert_eq!(chapter_number("080 玩物丧志"), Some(80.0));
		assert_eq!(chapter_number("702 701-2 漩涡中的旋风②"), Some(702.0));
		assert_eq!(chapter_number("插图"), None);
		assert_eq!(chapter_number("129.将计就计"), None);
		assert_eq!(chapter_number("第72卷"), None);
		assert_eq!(volume_number("第72卷"), Some(72.0));
		assert_eq!(volume_number("第177话（完）"), None);
		assert_eq!(volume_number("080 玩物丧志"), None);
	}

	#[aidoku_test]
	fn rates_by_tag() {
		assert_eq!(content_rating(&["冒险", "热血"]), ContentRating::Safe);
		assert_eq!(content_rating(&["青春", "耽美"]), ContentRating::Suggestive);
		assert_eq!(content_rating(&["后宫"]), ContentRating::Suggestive);
		for tag in SUGGESTIVE_TAGS {
			assert!(category_id(tag).is_some(), "{tag}");
		}
	}

	#[aidoku_test]
	fn parses_json_lists() {
		let sort = include_str!("fixtures/sort.json");
		assert_eq!(json_string(sort, "code").as_deref(), Some("200"));
		let list = parse_json_list(json_field(sort, "data").unwrap());
		assert_eq!(list.len(), 20);
		assert_eq!(list[0].key, "CmLCgCl");
		assert_eq!(list[0].title, "无根之树");
		assert_eq!(
			list[0].authors.as_deref(),
			Some(&[String::from("mark")][..])
		);
		assert!(list[0]
			.cover
			.as_deref()
			.unwrap()
			.starts_with("https://p6.ecombdimg.com/"));
		let data = json_field(sort, "data").unwrap();
		assert!(page_result(data, 1).has_next_page);
		assert!(!page_result(data, MAX_PAGES).has_next_page);
		// A full page with a repeated id still has a next page.
		let items = json_items(data);
		let doubled = data.replacen(items[1], items[0], 1);
		assert_eq!(page_result(&doubled, 1).entries.len(), 19);
		assert!(page_result(&doubled, 1).has_next_page);

		let search = include_str!("fixtures/search.json");
		let list = parse_json_list(json_field(search, "data").unwrap());
		assert!(!list.is_empty());
		assert!(list.iter().all(|m: &Manga| m.title.contains("斗破")));
		assert!(list.len() < PAGE_SIZE);

		for empty in [
			include_str!("fixtures/sort_end.json"),
			include_str!("fixtures/search_empty.json"),
		] {
			let code = json_string(empty, "code");
			assert!(
				matches!(code.as_deref(), Some("-1") | Some("201")),
				"{code:?}"
			);
		}
	}

	#[aidoku_test]
	fn parses_home() {
		let sections = parse_home(&doc(include_str!("fixtures/home.html")));
		let titles: Vec<&str> = sections
			.iter()
			.map(|(t, _): &(String, Vec<Manga>)| t.as_str())
			.collect();
		let shown: Vec<&str> = HOME_TITLES.iter().map(|(_, s): &(&str, &str)| *s).collect();
		assert_eq!(titles, shown);
		for (title, entries) in &sections {
			assert_eq!(entries.len(), 6, "{title}");
			assert!(entries.iter().all(|m: &Manga| m.cover.is_some()), "{title}");
		}
		assert_eq!(sections[0].1[0].key, "trsKtGi");
		assert_eq!(sections[0].1[0].title, "火影忍者");
	}

	#[aidoku_test]
	fn parses_rank_page() {
		let list = parse_rank_html(&doc(include_str!("fixtures/rank.html")));
		assert_eq!(list.len(), 50);
		assert_eq!(list[0].key, "TREEcUf");
		assert_eq!(list[0].title, "我获得了神级装备");
		assert!(list.iter().all(|m: &Manga| m.cover.is_some()));
		assert!(list
			.iter()
			.all(|m: &Manga| m.content_rating == ContentRating::Safe));
	}

	#[aidoku_test]
	fn parses_details_and_chapters() {
		let html = doc(include_str!("fixtures/details.html"));
		let manga = parse_details(&html, "trsKtGi").expect("details");
		assert_eq!(manga.title, "火影忍者");
		assert_eq!(
			manga.authors.as_deref(),
			Some(&[String::from("岸本齐史")][..])
		);
		assert_eq!(
			manga.tags.as_deref(),
			Some(
				&[
					String::from("冒险"),
					String::from("热血"),
					String::from("少年")
				][..]
			)
		);
		assert_eq!(manga.content_rating, ContentRating::Safe);
		assert!(manga
			.description
			.as_deref()
			.unwrap()
			.starts_with("从小身上封印着"));
		assert!(manga.cover.is_some());

		assert!(has_more_chapters(&html));
		let mut chapters = parse_page_chapters(&html, "trsKtGi");
		assert_eq!(chapters.len(), 20);
		assert_eq!(chapters[0].key, "IVpdIdWF");
		assert_eq!(
			chapters[0].title.as_deref(),
			Some("702 701-2 漩涡中的旋风②")
		);
		let more = include_str!("fixtures/morechapter.json");
		append_more_chapters(&mut chapters, json_field(more, "data").unwrap(), "trsKtGi");
		assert_eq!(chapters.len(), 546);
		assert_eq!(chapters[20].key, "vhTTvGi");
		assert_eq!(chapters[20].title.as_deref(), Some("第695话 鸣人与佐助②"));
		assert_eq!(
			chapters[20].url.as_deref(),
			Some("http://yumanhua.com/trsKtGi/vhTTvGi.html")
		);
	}

	#[aidoku_test]
	fn reads_reader_parts() {
		let (scripts, key_index) = reader_parts(&doc(include_str!("fixtures/reader.html")));
		assert_eq!(key_index, Some(2));
		let pages = crate::decoder::decode_pages(&scripts, 2).expect("decoded");
		assert_eq!(pages.len(), 30);
	}
}
