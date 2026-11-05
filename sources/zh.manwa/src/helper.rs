use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::defaults_get,
		html::{Document, Element},
		net::{Request, Response},
		std::parse_date_with_options,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result,
};
use serde::{de::DeserializeOwned, Deserialize};

use crate::auth;

pub const BASE_URL: &str = "https://manwacn.cc";

/// Page sizes the site's own pages use for each endpoint.
pub const CATE_PAGE_SIZE: i32 = 36;
pub const SEARCH_PAGE_SIZE: i32 = 20;
pub const RANK_PAGE_SIZE: i32 = 20;
/// The reader API's page size. The site's reader asks for 60 as well.
const IMAGE_PAGE_SIZE: i32 = 60;
/// More pages than this means the response is not what we think it is: the longest chapter
/// seen has 2 pages of 60 images (2026-11-05).
const MAX_IMAGE_PAGES: i32 = 100;

/// The label added to a VIP early-access chapter the account cannot open.
const VIP_MARK: &str = "・VIP 搶先看";
/// The label of a locked chapter that has no title of its own.
const VIP_MARK_ALONE: &str = "VIP 搶先看";

/// The details page shows chapter dates in China time: for 20 chapters of 94789, the
/// chapter API's timestamps give the page's date in UTC+8 every time and in UTC for 14 of
/// them (2026-11-05). Taipei has no daylight saving and the same offset, and zh.creativecomic
/// already parses its dates with it.
const SITE_TIMEZONE: &str = "Asia/Taipei";

// ---------------------------------------------------------------------------
// Base URL
// ---------------------------------------------------------------------------

/// The custom address when one is set, otherwise the address picked in the app's URL menu.
pub fn base_url() -> String {
	for key in ["customBaseUrl", "url"] {
		if let Some(url) = defaults_get::<String>(key)
			.as_deref()
			.and_then(normalize_base_url)
		{
			return url;
		}
	}
	String::from(BASE_URL)
}

/// Tidy a base URL typed by the user or written by the app: trim it, drop the trailing
/// slash and supply `https://` when the scheme was left out. Blank input yields `None`.
pub fn normalize_base_url(value: &str) -> Option<String> {
	let trimmed = value.trim().trim_end_matches('/');
	if trimmed.is_empty() {
		None
	} else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
		Some(String::from(trimmed))
	} else {
		Some(format!("https://{trimmed}"))
	}
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Every API response is `{code, msg, data}`; `code` is 200 on success. The JSON is read
/// with serde: titles carry HTML entities and escaped characters (`\,` inside author
/// names), which a hand-written scan would have to get right one by one.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
struct Envelope<T> {
	#[serde(default)]
	code: i32,
	#[serde(default)]
	msg: String,
	#[serde(default)]
	data: Option<T>,
}

/// One work in a list. The home carries `id`; `/api/cate` and `/api/rank` carry `url`
/// (`/comic/<id>`); `/api/search` carries both. The cover is `pic`, except in search, where
/// it is `cover`.
#[derive(Deserialize, Default)]
pub struct Entry {
	#[serde(default)]
	id: Option<i64>,
	#[serde(default)]
	url: Option<String>,
	#[serde(default)]
	title: String,
	#[serde(default)]
	pic: Option<String>,
	#[serde(default)]
	cover: Option<String>,
	/// Comma-separated tag names.
	#[serde(default)]
	tags: Option<String>,
	/// Comma-separated author names; only `/api/search` and `/api/home` carry it.
	#[serde(default)]
	author: Option<String>,
}

/// `data` of `/api/cate`, `/api/rank` and `/api/search`.
#[derive(Deserialize)]
pub struct EntryList {
	#[serde(default)]
	total: i64,
	#[serde(default)]
	list: Vec<Entry>,
}

/// `data` of `/api/home`: one list per row. `type` and `pageSize` do not change what the
/// site returns, so each list always holds 6 works (2026-11-05).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeData {
	#[serde(default)]
	pub comic_list: Vec<Entry>,
	#[serde(default)]
	pub gufeng_list: Vec<Entry>,
	#[serde(default)]
	pub vip_list: Vec<Entry>,
	#[serde(default)]
	pub xuanhuan_list: Vec<Entry>,
	#[serde(default)]
	pub xiaoyuan_list: Vec<Entry>,
}

#[derive(Deserialize)]
struct Image {
	#[serde(default)]
	url: String,
}

#[derive(Deserialize)]
struct Pagination {
	#[serde(default)]
	total: i64,
	#[serde(default)]
	total_pages: i32,
}

/// `data` of `/api/comic/image/<chapter>`.
#[derive(Deserialize)]
struct ImageData {
	#[serde(default)]
	images: Vec<Image>,
	#[serde(default)]
	pagination: Option<Pagination>,
}

/// An API GET. Carries the account's token when one is stored; reads defaults only.
pub fn api_get(path: &str) -> Result<Request> {
	let mut request = Request::get(format!("{}{path}", base_url()))?;
	if let Some(bearer) = auth::bearer() {
		request = request.header("Authorization", &bearer);
	}
	Ok(request)
}

/// The `data` of an API answer, or an error when the body is not one or its `code` is not
/// 200.
fn parse_api<T: DeserializeOwned>(text: &str, what: &str) -> Result<T> {
	let Ok(envelope) = serde_json::from_str::<Envelope<T>>(text) else {
		bail!("[manwa] unreadable answer for {what}");
	};
	if envelope.code != 200 {
		bail!(
			"[manwa] code {} for {what}: {}",
			envelope.code,
			envelope.msg
		);
	}
	let Some(data) = envelope.data else {
		bail!("[manwa] no data for {what}");
	};
	Ok(data)
}

/// `/api/cate` browse conditions. Every value is the site's own number, as written in
/// `filters.json`.
#[derive(Default)]
pub struct Browse {
	pub sort: Option<String>,
	pub status: Option<String>,
	pub kind: Option<String>,
	pub area: Option<String>,
	pub level: Option<String>,
	pub day: Option<String>,
	/// Tag names. The site reads them from the path, joined with `-` (the works that carry
	/// every one of them, 2026-11-05: `manhwa-bl` and `bl-manhwa` both gave 36); a tag in
	/// the body does nothing.
	pub tags: Vec<String>,
}

/// The path segment for a set of tags: each name encoded, then joined with `-`. `None` when
/// there is no tag.
pub fn tags_path(tags: &[String]) -> Option<String> {
	let encoded: Vec<String> = tags
		.iter()
		.map(|t: &String| t.trim())
		.filter(|t: &&str| !t.is_empty())
		.map(encode_uri_component)
		.collect();
	(!encoded.is_empty()).then(|| encoded.join("-"))
}

fn number_or(value: Option<&str>, default: i32) -> i32 {
	value
		.and_then(|v: &str| v.trim().parse::<i32>().ok())
		.unwrap_or(default)
}

/// `POST /api/cate[/<tag>]`. All six conditions combine (2026-11-05). The content type
/// (`kind`) is not a body field: the site reads it from a `typeId` cookie, so it is sent
/// as one on this request only.
pub fn cate_request(browse: &Browse, page: i32) -> Result<Request> {
	let mut path = String::from("/api/cate");
	if let Some(tags) = tags_path(&browse.tags) {
		path.push('/');
		path.push_str(&tags);
	}
	let body = format!(
		"{{\"page\":{{\"page\":{page},\"pageSize\":{CATE_PAGE_SIZE}}},\"category\":\"comic\",\"sort\":{},\"comic\":{{\"status\":{},\"day\":{},\"tag\":\"\",\"level\":{},\"areaId\":{}}}}}",
		number_or(browse.sort.as_deref(), 0),
		number_or(browse.status.as_deref(), -1),
		number_or(browse.day.as_deref(), 0),
		number_or(browse.level.as_deref(), 0),
		number_or(browse.area.as_deref(), 0),
	);
	let mut request = Request::post(format!("{}{path}", base_url()))?
		.header("Content-Type", "application/json")
		.body(body.as_bytes());
	if let Some(bearer) = auth::bearer() {
		request = request.header("Authorization", &bearer);
	}
	let kind = number_or(browse.kind.as_deref(), 0);
	if kind != 0 {
		request = request.header("Cookie", &format!("typeId={kind}"));
	}
	Ok(request)
}

/// `GET /api/rank`. `sort` is the board: 0 人氣榜, 1 完結榜, 2 新番榜.
pub fn rank_request(sort: i32, page: i32) -> Result<Request> {
	api_get(&format!(
		"/api/rank?page={page}&pageSize={RANK_PAGE_SIZE}&sort={sort}&timeType=0"
	))
}

/// `GET /api/search`. `type` must be `mh`. A keyword matches titles and authors; `author`
/// matches author names loosely (see `docs/sources/zh.manwa.md`).
pub fn search_request(keyword: Option<&str>, author: Option<&str>, page: i32) -> Result<Request> {
	let mut path = format!("/api/search?type=mh&page={page}&pageSize={SEARCH_PAGE_SIZE}");
	if let Some(keyword) = keyword {
		path.push_str(&format!(
			"&keyword={}",
			encode_uri_component(keyword.trim())
		));
	} else if let Some(author) = author {
		path.push_str(&format!("&author={}", encode_uri_component(author.trim())));
	}
	api_get(&path)
}

pub fn home_request() -> Result<Request> {
	api_get("/api/home?page=1&flag=true")
}

/// What a listing id stands for. `None` for an id `source.json` does not list.
pub fn listing_request(id: &str, page: i32) -> Option<Result<Request>> {
	let browse = |sort: &str, level: &str| Browse {
		sort: Some(String::from(sort)),
		level: Some(String::from(level)),
		..Default::default()
	};
	Some(match id {
		"update" => cate_request(&browse("0", "0"), page),
		"new" => cate_request(&browse("1", "0"), page),
		"hot" => cate_request(&browse("3", "0"), page),
		"vip" => cate_request(&browse("0", "2"), page),
		"rank_hot" => rank_request(0, page),
		"rank_end" => rank_request(1, page),
		"rank_new" => rank_request(2, page),
		_ => return None,
	})
}

/// Whether one of the work's authors is exactly `name`. The `author` field lists the
/// authors comma-separated, as the details page links them one by one. Both sides are
/// entity-decoded and trimmed.
pub fn has_author(field: Option<&str>, name: &str) -> bool {
	let name = decode_entities(name.trim());
	field.is_some_and(|field: &str| {
		field
			.split([',', '，'])
			.any(|author: &str| decode_entities(author.trim()) == name)
	})
}

/// `only_author` is set for a tapped author. The site's `author=` matches part of a name
/// (`나쵸소년` also returns works by `소년`, `칸` and `이소나`: 3 of 378 results were the
/// author's own, 2026-11-05) and ranks the exact matches first. The user chose to keep
/// only the works whose author field has the tapped name, so a page with none left has no
/// later page either.
pub fn parse_list(
	text: &str,
	what: &str,
	page: i32,
	page_size: i32,
	only_author: Option<&str>,
) -> Result<MangaPageResult> {
	let list: EntryList = parse_api(text, what)?;
	let entries: Vec<Manga> = list
		.list
		.into_iter()
		.filter(|e: &Entry| {
			only_author.is_none_or(|name: &str| has_author(e.author.as_deref(), name))
		})
		.filter_map(manga_from_entry)
		.collect();
	let has_next_page = !entries.is_empty() && i64::from(page) * i64::from(page_size) < list.total;
	Ok(MangaPageResult {
		entries,
		has_next_page,
	})
}

/// Sends a list request and reads the answer.
pub fn fetch_list(
	request: Request,
	what: &str,
	page: i32,
	page_size: i32,
	only_author: Option<&str>,
) -> Result<MangaPageResult> {
	let response = request.send()?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[manwa] HTTP {status} for {what}");
	}
	parse_list(&response.get_string()?, what, page, page_size, only_author)
}

/// Reads the home answer.
pub fn read_home(response: Response) -> Result<HomeData> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[manwa] HTTP {status} for /api/home");
	}
	parse_home(&response.get_string()?)
}

pub fn parse_home(text: &str) -> Result<HomeData> {
	parse_api(text, "/api/home")
}

// ---------------------------------------------------------------------------
// Works from the API
// ---------------------------------------------------------------------------

/// The numbers in the first `/comic/<id>[/<chapter>]` found in a URL or path.
pub fn comic_path(url: &str) -> Option<(String, Option<String>)> {
	let rest = &url[url.find("/comic/")? + "/comic/".len()..];
	let rest = rest.split(['?', '#']).next().unwrap_or(rest);
	let mut parts = rest.split('/');
	let manga = parts.next().filter(|p: &&str| is_id(p))?;
	let chapter = parts.next().filter(|p: &&str| is_id(p));
	Some((String::from(manga), chapter.map(String::from)))
}

pub fn is_id(value: &str) -> bool {
	!value.is_empty() && value.chars().all(|c: char| c.is_ascii_digit())
}

pub fn manga_url(key: &str) -> String {
	format!("{}/comic/{key}", base_url())
}

pub fn chapter_url(manga_key: &str, chapter_key: &str) -> String {
	format!("{}/comic/{manga_key}/{chapter_key}", base_url())
}

/// The API writes titles with HTML entities (`&lt;大胸系列2&gt;`, 2026-11-05).
fn decode_entities(text: &str) -> String {
	text.replace("&lt;", "<")
		.replace("&gt;", ">")
		.replace("&quot;", "\"")
		.replace("&#39;", "'")
		.replace("&#x27;", "'")
		.replace("&amp;", "&")
}

/// The works of one home row.
pub fn home_entries(entries: Vec<Entry>) -> Vec<Manga> {
	entries.into_iter().filter_map(manga_from_entry).collect()
}

fn manga_from_entry(entry: Entry) -> Option<Manga> {
	let key = match entry.id {
		Some(id) => format!("{id}"),
		None => comic_path(entry.url.as_deref()?)?.0,
	};
	let title = decode_entities(entry.title.trim());
	if title.is_empty() {
		return None;
	}
	let tags: Vec<&str> = entry
		.tags
		.as_deref()
		.map(|t: &str| t.split(',').map(str::trim).collect())
		.unwrap_or_default();
	Some(Manga {
		url: Some(manga_url(&key)),
		cover: entry
			.pic
			.or(entry.cover)
			.filter(|c: &String| c.starts_with("http")),
		content_rating: content_rating(&title, &tags),
		key,
		title,
		..Default::default()
	})
}

/// Tags that mark an adult work: `19r` (the site's 19+ mark), `无码` (uncensored) and
/// `禁漫` (its adult comic type). The site does not say which works are adult any other
/// way. A title marked `【无码】` is the uncensored edition of a work whose tags may not
/// say so. Everything else is `Safe`: the site lists a few adult works among many
/// ordinary ones, so the whole source is not marked suggestive (see `CLAUDE.md`).
const ADULT_TAGS: [&str; 3] = ["19r", "无码", "禁漫"];
const ADULT_TITLE_MARK: &str = "【无码】";

pub fn content_rating(title: &str, tags: &[&str]) -> ContentRating {
	let adult_tag = tags.iter().any(|tag: &&str| {
		ADULT_TAGS
			.iter()
			.any(|adult: &&str| tag.eq_ignore_ascii_case(adult))
	});
	if adult_tag || title.contains(ADULT_TITLE_MARK) {
		ContentRating::NSFW
	} else {
		ContentRating::Safe
	}
}

// ---------------------------------------------------------------------------
// Details page
// ---------------------------------------------------------------------------

/// The details page of a work. A page without a title is an error, so a maintenance or
/// error page is not read as a work.
pub fn fetch_details_page(key: &str) -> Result<Document> {
	let url = manga_url(key);
	let response = Request::get(&url)?.send()?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[manwa] HTTP {status} for {url}");
	}
	let html = response.get_html()?;
	if html.select_first("#page-title").is_none() {
		bail!("[manwa] no details on {url}");
	}
	Ok(html)
}

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first("#page-title")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first("img.comic-cover")
		.and_then(|el: Element| el.attr("data-src"))
		.filter(|src: &String| src.starts_with("http"));
	let authors: Vec<String> = html
		.select("#author-container a")
		.into_iter()
		.flatten()
		.map(|a: Element| text_of(&a))
		.filter(|name: &String| !name.is_empty() && name != "N/A")
		.collect();
	// The detail page is the only place that tells ongoing from finished: the lists'
	// `status` is the reverse of the filter (a finished-only filter returns `status:0`).
	let status = match html
		.select_first("#status")
		.map(|el: Element| text_of(&el))
		.as_deref()
	{
		Some(s) if s.contains("完结") || s.contains("完結") => MangaStatus::Completed,
		Some(s) if s.contains("连载") || s.contains("連載") => MangaStatus::Ongoing,
		_ => MangaStatus::Unknown,
	};
	let mut tags: Vec<String> = Vec::new();
	for tag in html
		.select("#tagsContainer a span.tag")
		.into_iter()
		.flatten()
	{
		let name = text_of(&tag);
		if !name.is_empty() && !tags.contains(&name) {
			tags.push(name);
		}
	}
	let description = html
		.select_first("p.comic-desc")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		cover,
		content_rating: content_rating(&title, &tag_refs),
		authors: (!authors.is_empty()).then_some(authors),
		description,
		tags: (!tags.is_empty()).then_some(tags),
		status,
		title,
		..Default::default()
	})
}

/// `第158话` → 158, `72话 大结局` → 72, `176.拉扯` → 176. `新年番外` gets none.
pub fn chapter_number(title: &str) -> Option<f32> {
	let title = title.trim();
	let rest = title.strip_prefix('第').unwrap_or(title);
	let end = rest
		.char_indices()
		.find(|&(_, c): &(usize, char)| !(c.is_ascii_digit() || c == '.'))
		.map_or(rest.len(), |(i, _): (usize, char)| i);
	let number = rest[..end].trim_end_matches('.');
	if number.is_empty() {
		return None;
	}
	let next = rest[end..].chars().next();
	let marked = matches!(next, Some('话' | '話' | '回' | '章' | '卷' | '集'));
	// A bare number counts only when it leads the title (`780 大只佬`, `176.拉扯`);
	// `第` without a unit after it (`第2季`) does not.
	if title.starts_with('第') && !marked {
		return None;
	}
	number.parse::<f32>().ok()
}

/// Every chapter, newest first. The page lists them oldest first. A VIP early-access
/// chapter (`data-locked="true"`) is locked and marked unless the account is a VIP. The
/// site shows no coin price anywhere (the chapter API's `coin` was 0 for all 179
/// chapters sampled, 2026-11-05), so VIP is the only kind of paid chapter.
///
/// `Ok(None)` when the page lists no chapters; an error when the list is not there.
pub fn parse_chapters(html: &Document, manga_key: &str, vip: bool) -> Result<Option<Vec<Chapter>>> {
	let Some(grid) = html.select_first("#chapter-grid-container") else {
		bail!("[manwa] no chapter list for {manga_key}");
	};
	let mut chapters: Vec<Chapter> = Vec::new();
	for item in grid.select("a.chapter-item").into_iter().flatten() {
		let Some(key) = item
			.attr("href")
			.and_then(|href: String| comic_path(&href))
			.and_then(|(_, chapter)| chapter)
		else {
			bail!("[manwa] a chapter of {manga_key} has no id");
		};
		let title = item
			.attr("data-title")
			.map(|t: String| String::from(t.trim()))
			.filter(|t: &String| !t.is_empty());
		let date_uploaded = item
			.select(".chapter-meta span")
			.and_then(|mut spans| spans.next_back())
			.and_then(|span: Element| span.text())
			// The page gives a date with no time. The time is added because the parser the
			// test runner uses rejects a format without one.
			.and_then(|date: String| {
				parse_date_with_options(
					format!("{} 00:00:00", date.trim()),
					"yyyy-MM-dd HH:mm:ss",
					"zh_TW",
					SITE_TIMEZONE,
				)
			});
		let early_access = item.attr("data-locked").as_deref() == Some("true");
		let locked = early_access && !vip;
		let chapter_number = title.as_deref().and_then(chapter_number);
		let title = if locked {
			Some(match title {
				Some(title) => format!("{title}{VIP_MARK}"),
				None => String::from(VIP_MARK_ALONE),
			})
		} else {
			title
		};
		chapters.push(Chapter {
			url: Some(chapter_url(manga_key, &key)),
			chapter_number,
			title,
			date_uploaded,
			locked,
			key,
			..Default::default()
		});
	}
	if chapters.is_empty() {
		return Ok(None);
	}
	chapters.reverse();
	Ok(Some(chapters))
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Collects a chapter's images page by page. A page with no images, a page count that is
/// not plausible, or a total that does not match the images collected fails the chapter,
/// so the reader never gets pages missing from the middle.
pub struct ImageCollector {
	urls: Vec<String>,
	total_pages: i32,
	expected: i64,
	page: i32,
}

impl ImageCollector {
	pub fn new() -> Self {
		Self {
			urls: Vec::new(),
			total_pages: 1,
			expected: 0,
			page: 1,
		}
	}

	/// The page the next answer must be for.
	pub fn page(&self) -> i32 {
		self.page
	}

	/// Takes one page's answer. `true` when another page is still to be fetched.
	pub fn accept(&mut self, text: &str, what: &str) -> Result<bool> {
		let data: ImageData = parse_api(text, what)?;
		if self.page == 1 {
			let Some(pagination) = data.pagination else {
				bail!("[manwa] no pagination for {what}");
			};
			self.total_pages = pagination.total_pages.max(1);
			self.expected = pagination.total;
			if self.total_pages > MAX_IMAGE_PAGES {
				bail!("[manwa] {} pages for {what}", self.total_pages);
			}
		}
		if data.images.is_empty() {
			bail!("[manwa] no images for {what}");
		}
		for image in data.images {
			if !image.url.starts_with("http") {
				bail!("[manwa] an image without an address in {what}");
			}
			self.urls.push(image.url);
		}
		if self.page < self.total_pages {
			self.page += 1;
			return Ok(true);
		}
		Ok(false)
	}

	pub fn finish(self, chapter_key: &str) -> Result<Vec<String>> {
		if i64::try_from(self.urls.len()).ok() != Some(self.expected) {
			bail!(
				"[manwa] chapter {chapter_key} has {} images, the site says {}",
				self.urls.len(),
				self.expected
			);
		}
		Ok(self.urls)
	}
}

impl Default for ImageCollector {
	fn default() -> Self {
		Self::new()
	}
}

/// Every image of a chapter, in order, fetched one page after another.
pub fn fetch_page_urls(chapter_key: &str) -> Result<Vec<String>> {
	let mut collector = ImageCollector::new();
	loop {
		let page = collector.page();
		let what = format!("the images of chapter {chapter_key}, page {page}");
		let response = api_get(&format!(
			"/api/comic/image/{chapter_key}?page={page}&page_size={IMAGE_PAGE_SIZE}"
		))?
		.send()?;
		let status = response.status_code();
		if !(200..300).contains(&status) {
			bail!("[manwa] HTTP {status} for {what}");
		}
		if !collector.accept(&response.get_string()?, &what)? {
			break;
		}
	}
	collector.finish(chapter_key)
}

pub fn pages_from_urls(urls: Vec<String>) -> Vec<Page> {
	urls.into_iter()
		.map(|url: String| Page {
			content: PageContent::url(url),
			..Default::default()
		})
		.collect()
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn normalizes_typed_base_urls() {
		assert_eq!(
			normalize_base_url(" manwabb.cc/ ").as_deref(),
			Some("https://manwabb.cc")
		);
		assert_eq!(
			normalize_base_url("http://manwayy.cc").as_deref(),
			Some("http://manwayy.cc")
		);
		assert_eq!(normalize_base_url("   "), None);
	}

	#[aidoku_test]
	fn reads_comic_and_chapter_ids_from_any_mirror() {
		assert_eq!(
			comic_path("https://manwacn.cc/comic/94789"),
			Some((String::from("94789"), None))
		);
		assert_eq!(
			comic_path("https://mwmwff.cc/comic/94789/1361313?from=x#a"),
			Some((String::from("94789"), Some(String::from("1361313"))))
		);
		assert_eq!(comic_path("/comic/136679/1968223").unwrap().0, "136679");
		assert_eq!(comic_path("https://manwacn.cc/cate/bl"), None);
		assert_eq!(comic_path("https://manwacn.cc/comic/abc"), None);
	}

	#[aidoku_test]
	fn builds_urls_from_the_given_ids() {
		assert_eq!(manga_url("94789"), format!("{BASE_URL}/comic/94789"));
		assert_eq!(
			chapter_url("94789", "1361313"),
			format!("{BASE_URL}/comic/94789/1361313")
		);
	}

	#[aidoku_test]
	fn only_adult_tags_and_marked_titles_are_nsfw() {
		assert_eq!(
			content_rating("Mr. A's Farm (台版)", &["韩漫", "19r"]),
			ContentRating::NSFW
		);
		assert_eq!(content_rating("某作品", &["无码"]), ContentRating::NSFW);
		assert_eq!(content_rating("某作品", &["禁漫"]), ContentRating::NSFW);
		assert_eq!(
			content_rating("STAGE BEHIND【无码】", &[]),
			ContentRating::NSFW
		);
		assert_eq!(
			content_rating("暴君少女尼禄的叛乱", &["异世界", "奇幻", "魔法", "中文"]),
			ContentRating::Safe
		);
		// A tag that merely contains the mark is not the mark.
		assert_eq!(content_rating("某作品", &["19r以上"]), ContentRating::Safe);
	}

	#[aidoku_test]
	fn decodes_html_entities_in_titles() {
		assert_eq!(
			decode_entities("&lt;大胸系列2&gt;Love &amp; Marriage"),
			"<大胸系列2>Love & Marriage"
		);
	}

	#[aidoku_test]
	fn numbers_chapters_from_their_titles() {
		assert_eq!(chapter_number("第158话"), Some(158.0));
		assert_eq!(chapter_number("72话 大结局"), Some(72.0));
		assert_eq!(chapter_number("176.拉扯"), Some(176.0));
		assert_eq!(chapter_number("第2季"), None);
		assert_eq!(chapter_number("新年番外"), None);
	}

	#[aidoku_test]
	fn joins_tags_into_one_path_segment() {
		let tags =
			|names: &[&str]| -> Vec<String> { names.iter().map(|n| String::from(*n)).collect() };
		assert_eq!(tags_path(&[]), None);
		assert_eq!(tags_path(&tags(&["", " "])), None);
		assert_eq!(tags_path(&tags(&["manhwa"])).as_deref(), Some("manhwa"));
		assert_eq!(
			tags_path(&tags(&["韩漫", "BL"])).as_deref(),
			Some("%E9%9F%A9%E6%BC%AB-BL")
		);
	}

	#[aidoku_test]
	fn matches_authors_by_whole_name() {
		assert!(has_author(Some("아페르타, 라펫, tomato, Studio"), "tomato"));
		assert!(has_author(
			Some("Yang Pyung/Painkiller, Saint Nine"),
			" Saint Nine "
		));
		assert!(has_author(
			Some("Yang Pyung/Painkiller"),
			"Yang Pyung/Painkiller"
		));
		assert!(has_author(Some("A &amp; B"), "A & B"));
		assert!(!has_author(Some("소년"), "나쵸소년"));
		assert!(!has_author(Some("tomatoes"), "tomato"));
		assert!(!has_author(None, "tomato"));
	}

	#[aidoku_test]
	fn numbers_default_to_the_unfiltered_values() {
		assert_eq!(number_or(None, -1), -1);
		assert_eq!(number_or(Some("3"), 0), 3);
		assert_eq!(number_or(Some("x"), 0), 0);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
