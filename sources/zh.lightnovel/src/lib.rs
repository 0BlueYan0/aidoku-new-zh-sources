#![no_std]

use aidoku::{
	alloc::{format, string::ToString, vec, String, Vec},
	imports::{
		defaults::defaults_get,
		net::Request,
		std::{current_date, parse_date, send_partial_result},
	},
	prelude::*,
	BasicLoginHandler, Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings,
	FilterValue, GroupSetting, Home, HomeComponent, HomeComponentValue, HomeLayout,
	HomePartialResult, ImageRequestProvider, Link, Listing, ListingProvider, Manga, MangaPageResult, MangaStatus,
	NotificationHandler, Page, PageContent, PageContext, Result, Setting, Source, Viewer,
};
use serde_json::{json, Value};

mod auth;
mod content;
mod font;
mod glyph_table;
mod hub;
mod sign_in;
mod vertical;

use hub::{Call, Reply};

const SITE_URL: &str = "https://www.lightnovel.app";
const PAGE_SIZE: i64 = 24;
const HOME_SIZE: i64 = 12;
/// `GetComicContent` hands out at most 12 images per call.
const COMIC_TAKE: i64 = 12;
/// Placeholder for a comic page whose URL is fetched on demand: `lnc://<cid>/<index>`.
const COMIC_SCHEME: &str = "lnc://";

struct LightnovelSource;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// `Convert` for `GetNovelContent`: the site converts the text itself before it
/// obfuscates it. Setting `convert`: `s2t` (default), `t2s` or `none`.
fn convert_setting() -> Value {
	match defaults_get::<String>("convert").as_deref() {
		Some("none") => Value::Null,
		Some("t2s") => json!("t2s"),
		_ => json!("s2t"),
	}
}

/// Setting `layout`: `vertical` draws novel chapters as vertical page images.
fn vertical_layout() -> bool {
	defaults_get::<String>("layout").as_deref() == Some("vertical")
}

/// Comics and vertical novel pages read right to left; horizontal text scrolls.
fn viewer_for(comic: bool) -> Viewer {
	if comic || vertical_layout() {
		Viewer::RightToLeft
	} else {
		Viewer::Vertical
	}
}

fn with_ignore_flags(mut args: Value) -> Value {
	args["IgnoreJapanese"] = json!(defaults_get::<bool>("ignoreJapanese").unwrap_or(false));
	args["IgnoreAI"] = json!(defaults_get::<bool>("ignoreAI").unwrap_or(false));
	args
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

// List endpoints answer in PascalCase (`Id`, `Title`), `GetBookInfo` and the comic
// lists in camelCase, so every field is read under both spellings.
fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
	let mut pascal = String::with_capacity(name.len());
	let mut chars = name.chars();
	if let Some(first) = chars.next() {
		pascal.extend(first.to_uppercase());
		pascal.extend(chars);
	}
	value.get(name).or_else(|| value.get(pascal.as_str())).filter(|v: &&Value| !v.is_null())
}

fn str_field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
	field(value, name).and_then(Value::as_str).filter(|s: &&str| !s.is_empty())
}

fn int_field(value: &Value, name: &str) -> Option<i64> {
	field(value, name).and_then(Value::as_i64)
}

/// `2026-09-30T03:40:39.059488Z`, as the site sends `LastUpdatedAt` (probed 2026-09-30):
/// UTC, which is the zone `parse_date` reads in, so the fraction and the `Z` can go.
/// The test runner rejects a quoted `'T'` in the format, hence the space.
fn parse_iso(date: &str) -> Option<i64> {
	let head = date.get(..19)?;
	parse_date(head.replace('T', " "), "yyyy-MM-dd HH:mm:ss")
}

/// `Type` is `"Novel"`/`"Comic"` over the JSON protocol; Novella also accepts 0/1,
/// which MessagePack produces from the same enum.
fn is_comic(value: &Value) -> bool {
	match field(value, "type") {
		Some(Value::String(t)) => t == "Comic",
		Some(Value::Number(n)) => n.as_i64() == Some(1),
		_ => false,
	}
}

fn manga_url(id: i64, comic: bool) -> String {
	if comic {
		format!("{SITE_URL}/manga/{id}")
	} else {
		format!("{SITE_URL}/book/info/{id}")
	}
}

/// A list entry. `comic` is known for the comic endpoints; book lists carry `Type`.
fn parse_entry(value: &Value, comic: bool) -> Option<Manga> {
	let id = int_field(value, "id")?;
	let comic = comic || is_comic(value);
	let mut tags = Vec::new();
	if let Some(category) = field(value, "category").and_then(|c: &Value| str_field(c, "name")) {
		tags.push(category.to_string());
	}
	// Books above the account's reading level answer 403; show the level up front.
	if let Some(level) = int_field(value, "level").filter(|l: &i64| *l > 0) {
		tags.push(format!("Lv.{level}"));
	}
	Some(Manga {
		key: format!("{id}"),
		title: str_field(value, "title").unwrap_or_default().to_string(),
		cover: str_field(value, "cover").map(content::image_url),
		url: Some(manga_url(id, comic)),
		tags: (!tags.is_empty()).then_some(tags),
		viewer: viewer_for(comic),
		..Default::default()
	})
}

/// `{page, size, total, totalPages, hasMore, data: [...]}` (what `GetLatestBookList`
/// answered on 2026-09-30) or a bare array (`GetRank`). `hasMore` decides the next page;
/// without it, `page < totalPages` does, which is all Novella's envelope carries.
fn parse_page(value: &Value, comic: bool) -> MangaPageResult {
	let (items, has_next) = match value {
		Value::Array(items) => (items.as_slice(), false),
		_ => (
			field(value, "data").and_then(Value::as_array).map_or(&[][..], |a: &Vec<Value>| a.as_slice()),
			field(value, "hasMore").and_then(Value::as_bool).unwrap_or_else(|| {
				matches!((int_field(value, "page"), int_field(value, "totalPages")), (Some(p), Some(t)) if p < t)
			}),
		),
	};
	let entries: Vec<Manga> = items.iter().filter_map(|v: &Value| parse_entry(v, comic)).collect();
	MangaPageResult {
		has_next_page: has_next && !entries.is_empty(),
		entries,
	}
}

/// Novel category names as `GetBookCategories {Type: "Novel"}` returned them on
/// 2026-09-29; they reach the tags as the site spells them.
fn novel_category_id(name: &str) -> Option<i64> {
	Some(match name {
		"录入完成" => 1,
		"翻译完成" => 2,
		"翻译中" => 3,
		"录入中" => 4,
		"转载" => 5,
		"原创" => 7,
		"AI翻译" => 8,
		_ => return None,
	})
}

fn status_for(category: &str) -> MangaStatus {
	match category {
		"连载" | "翻译中" | "录入中" => MangaStatus::Ongoing,
		"完结" | "翻译完成" | "录入完成" => MangaStatus::Completed,
		_ => MangaStatus::Unknown,
	}
}

/// "阅读等级不足，需要达到 Lv.4 才能阅读该书籍" -> 4.
fn required_level(msg: &str) -> Option<u32> {
	let after = &msg[msg.find("Lv.")? + 3..];
	let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
	digits.parse().ok()
}

// ---------------------------------------------------------------------------
// Hub helpers
// ---------------------------------------------------------------------------

/// The token for this entry point, or an error that says why there is none.
fn require_token() -> Result<String> {
	match auth::access_token() {
		Some(token) => Ok(token),
		None if auth::is_logged_in() => bail!("[lightnovel] login could not be renewed"),
		None => bail!("[lightnovel] login required"),
	}
}

fn is_unauthorized(reply: &Reply) -> bool {
	matches!(reply, Reply::Error(msg) if msg.contains("unauthorized"))
}

/// Run `calls` signed in. Calls made within a second or so of a login came back
/// unauthorized on the device (2026-09-29) while the same token worked a moment later,
/// so an unauthorized reply renews the token and runs the batch once more, as Novella
/// does. Renewal goes through auth's own gates, so a dead login costs one extra refresh
/// and then trips the needs-relogin brake instead of retrying forever.
///
/// When the day's sign-in is due, `SignIn` goes first in the same batch (so a
/// `GetMyInfo` behind it already sees the result) and its reply is taken back out.
fn invoke_signed_in(calls: &[Call]) -> Result<Vec<Reply>> {
	let token = require_token()?;
	let now = current_date();
	let with_sign_in = sign_in::claim_if_due(now);
	let batch: Vec<Call>;
	let calls: &[Call] = if with_sign_in {
		batch = core::iter::once(sign_in::call()).chain(calls.iter().cloned()).collect();
		&batch
	} else {
		calls
	};
	let mut replies = hub::invoke_all(Some(&token), calls)?;
	if replies.iter().any(is_unauthorized) {
		println!("[lightnovel] hub said unauthorized, renewing the token once");
		if let Some(token) = auth::renew() {
			replies = hub::invoke_all(Some(&token), calls)?;
		}
	}
	if with_sign_in && !replies.is_empty() {
		sign_in::record(&replies.remove(0), now);
	}
	Ok(replies)
}

fn invoke_one(target: &'static str, args: Value) -> Result<Reply> {
	let mut replies = invoke_signed_in(&[Call::new(target, args)])?;
	Ok(replies.pop().unwrap_or(Reply::Error(String::new())))
}

fn call_one(target: &'static str, args: Value) -> Result<Value> {
	invoke_one(target, args)?.into_result()
}

fn listing_call(id: &str, page: i32) -> Option<(Call, bool)> {
	let page = i64::from(page);
	let rank = |days: i64| Call::new("GetRank", with_ignore_flags(json!({ "Days": days })));
	Some(match id {
		"latest" => (
			Call::new("GetBookList", with_ignore_flags(json!({ "Page": page, "Size": PAGE_SIZE, "Order": "latest" }))),
			false,
		),
		"daily" => (rank(1), false),
		"weekly" => (rank(7), false),
		// The site's own monthly board asks for 31 days.
		"monthly" => (rank(31), false),
		"comic" => (Call::new("GetComicList", json!({ "Page": page, "Size": PAGE_SIZE })), true),
		_ => return None,
	})
}

fn comic_args(cid: i64, skip: i64) -> Value {
	json!({ "Cid": cid, "Skip": skip, "Take": COMIC_TAKE })
}

/// (`Total`, first page index, image URLs) of a `GetComicContent` reply. The reply's own
/// `Skip` says where the batch starts, and Novella indexes by it rather than by the
/// value it asked for; `requested` stands in when the reply has none.
fn comic_batch(value: &Value, requested: i64) -> (i64, i64, Vec<String>) {
	let Some(ch) = field(value, "chapter") else {
		return (0, requested, Vec::new());
	};
	let urls = field(ch, "images")
		.and_then(Value::as_array)
		.map(|list: &Vec<Value>| list.iter().filter_map(Value::as_str).map(content::image_url).collect())
		.unwrap_or_default();
	(int_field(ch, "total").unwrap_or(0), int_field(ch, "skip").unwrap_or(requested), urls)
}

/// Batches of comic image URLs already paid for, so the other pages of a batch need no
/// second call. Only the most recent few are kept, and a chapter's batches are dropped
/// when it is opened again: the URLs carry a `t=` signature whose lifetime is unknown,
/// and a page the site already charged today is not charged again when refetched.
mod comic_cache {
	use aidoku::{
		alloc::{string::ToString, String, Vec},
		imports::defaults::{defaults_get, defaults_set, DefaultValue},
	};
	use serde_json::{json, Value};

	const KEY: &str = "comic_batches";
	const KEEP: usize = 8;

	fn load() -> Vec<Value> {
		defaults_get::<String>(KEY)
			.and_then(|s: String| serde_json::from_str::<Value>(&s).ok())
			.and_then(|v: Value| v.as_array().cloned())
			.unwrap_or_default()
	}

	fn save(entries: Vec<Value>) {
		defaults_set(KEY, DefaultValue::String(Value::Array(entries).to_string()));
	}

	fn urls_of(entry: &Value) -> Vec<String> {
		entry["urls"]
			.as_array()
			.map(|a: &Vec<Value>| a.iter().filter_map(Value::as_str).map(String::from).collect())
			.unwrap_or_default()
	}

	/// The cached batch holding page `index` of chapter `cid`: (its first index, URLs).
	pub fn get(cid: i64, index: i64) -> Option<(i64, Vec<String>)> {
		load().into_iter().find_map(|entry: Value| {
			if entry["cid"].as_i64() != Some(cid) {
				return None;
			}
			let skip = entry["skip"].as_i64()?;
			let urls = urls_of(&entry);
			(skip <= index && index < skip + urls.len() as i64).then_some((skip, urls))
		})
	}

	/// Keep a batch. With `fresh`, every other batch of the chapter goes first.
	pub fn store(cid: i64, skip: i64, urls: &[String], fresh: bool) {
		let mut entries: Vec<Value> = load()
			.into_iter()
			.filter(|e: &Value| e["cid"].as_i64() != Some(cid) || (!fresh && e["skip"].as_i64() != Some(skip)))
			.collect();
		if !urls.is_empty() {
			entries.push(json!({ "cid": cid, "skip": skip, "urls": urls }));
		}
		while entries.len() > KEEP {
			entries.remove(0);
		}
		save(entries);
	}
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

impl Source for LightnovelSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut comic = false;
		let mut mode = String::from("fuzzy");
		let mut sort = String::from("latest");
		let mut category = String::new();
		let mut author: Option<String> = None;
		let mut tag: Option<String> = None;
		for filter in filters {
			match filter {
				FilterValue::Select { id, value } => match id.as_str() {
					"type" => comic = value == "comic",
					"mode" if !value.is_empty() => mode = value,
					"sort" if !value.is_empty() => sort = value,
					"category" => category = value,
					// A tapped tag.
					"genre" => tag = Some(value),
					_ => {}
				},
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				_ => {}
			}
		}

		let page_n = i64::from(page);
		let base = with_ignore_flags(json!({ "Page": page_n, "Size": PAGE_SIZE }));
		let with_keywords = |keywords: &str| {
			let mut args = base.clone();
			args["KeyWords"] = json!(keywords);
			args
		};
		let query = query.map(|q: String| q.trim().to_string()).filter(|q: &String| !q.is_empty());
		let author = author.map(|a: String| a.trim().to_string()).filter(|a: &String| !a.is_empty());
		let tag = tag.map(|t: String| t.trim().to_string()).filter(|t: &String| !t.is_empty());

		let (call, comic) = if let Some(author) = author {
			// Author and tag searches come from tapping a book's details, which are
			// always the book's own kind; both kinds are searched through the book lists.
			(Call::new("GetBookListByAuthor", with_keywords(&author)), false)
		} else if let Some(tag) = tag {
			if tag.starts_with("Lv.") {
				// The reading level this source adds as a tag; no book is tagged with it.
				return Ok(MangaPageResult::default());
			}
			match novel_category_id(&tag) {
				// A tapped category lists that category instead of searching tags.
				Some(id) => {
					let mut args = base.clone();
					args["Order"] = json!("latest");
					args["CategoryId"] = json!(id);
					(Call::new("GetBookList", args), false)
				}
				None => (Call::new("GetBookListByTags", with_keywords(&tag)), false),
			}
		} else if let Some(query) = query {
			if comic {
				let mut args = with_keywords(&query);
				args["Mode"] = json!(mode);
				(Call::new("SearchComicSeries", args), true)
			} else {
				let call = match mode.as_str() {
					"exact" => Call::new("GetBookList", with_keywords(&format!("\"{query}\""))),
					"title" => Call::new("GetBookListByTitle", with_keywords(&query)),
					"author" => Call::new("GetBookListByAuthor", with_keywords(&query)),
					"name" => Call::new("GetBookListByName", with_keywords(&query)),
					"tags" => Call::new("GetBookListByTags", with_keywords(&query)),
					_ => Call::new("GetBookList", with_keywords(&query)),
				};
				(call, false)
			}
		} else if comic {
			(Call::new("GetComicList", json!({ "Page": page_n, "Size": PAGE_SIZE, "Order": sort })), true)
		} else {
			let mut args = base.clone();
			args["Order"] = json!(sort);
			if let Ok(id) = category.parse::<i64>() {
				args["CategoryId"] = json!(id);
			}
			(Call::new("GetBookList", args), false)
		};

		let value = call_one(call.target, call.args)?;
		Ok(parse_page(&value, comic))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let id: i64 = manga.key.parse().map_err(|_| error!("[lightnovel] bad key {}", manga.key))?;
		let reply = invoke_one("GetBookInfo", json!({ "Id": id }))?;
		let info = match reply {
			Reply::Ok(value) => value,
			Reply::Refused(_, msg) if needs_details && required_level(&msg).is_some() => {
				// Nothing else is served for such a book; say what it takes instead of
				// failing with the app's generic error.
				let level = required_level(&msg).unwrap_or(0);
				manga.description = Some(format!("閱讀等級不足，需要達到 Lv.{level} 才能閱讀這本書。"));
				return Ok(manga);
			}
			other => return Err(other.into_result().unwrap_err()),
		};
		// Novella accepts the book both under `Book` and as the response itself.
		let book = field(&info, "book").unwrap_or(&info);
		if int_field(book, "id").is_none() {
			bail!("[lightnovel] no book in GetBookInfo");
		}
		let comic = is_comic(book);

		if needs_details {
			let category = field(book, "category").and_then(|c: &Value| str_field(c, "name")).unwrap_or("");
			let mut tags: Vec<String> = Vec::new();
			if !category.is_empty() {
				tags.push(category.to_string());
			}
			if let Some(list) = field(book, "extra")
				.and_then(|e: &Value| field(e, "classification"))
				.and_then(|c: &Value| field(c, "tags"))
				.and_then(Value::as_array)
			{
				tags.extend(list.iter().filter_map(Value::as_str).map(String::from));
			}
			// Keep the level tag a list entry put there; the details do not carry it.
			if let Some(old) = &manga.tags {
				tags.extend(old.iter().filter(|t: &&String| t.starts_with("Lv.")).cloned());
			}
			manga.title = str_field(book, "title").unwrap_or(&manga.title).to_string();
			manga.cover = str_field(book, "cover").map(content::image_url).or(manga.cover);
			manga.authors = str_field(book, "author").map(|a: &str| vec![a.to_string()]);
			manga.description = str_field(book, "introduction").map(content::plain_text);
			manga.url = Some(manga_url(id, comic));
			manga.tags = (!tags.is_empty()).then_some(tags);
			manga.status = status_for(category);
			manga.viewer = viewer_for(comic);
		}

		if needs_chapters {
			let list = field(book, "chapters")
				.or_else(|| field(book, "chapter"))
				.and_then(Value::as_array)
				.cloned()
				.unwrap_or_default();
			let mut chapters: Vec<Chapter> = list
				.iter()
				.enumerate()
				.filter_map(|(i, c): (usize, &Value)| {
					// Novella numbers a chapter by its position when SortNum is missing.
					let sort = int_field(c, "sortNum").unwrap_or(i as i64 + 1);
					let key = if comic {
						// Page count rides in the key: page lists need it to split the
						// 12-image calls, and GetComicContent is only reachable by id.
						format!("c{}.{}", int_field(c, "id")?, int_field(c, "pageCount").unwrap_or(0))
					} else {
						format!("n{sort}")
					};
					let url = if comic {
						format!("{SITE_URL}/manga/{id}/read/{}", int_field(c, "id")?)
					} else {
						format!("{SITE_URL}/read/{id}/{sort}")
					};
					Some(Chapter {
						key,
						title: str_field(c, "title").map(String::from),
						chapter_number: Some(sort as f32),
						date_uploaded: str_field(c, "updatedAt").or_else(|| str_field(c, "createdAt")).and_then(parse_iso),
						url: Some(url),
						..Default::default()
					})
				})
				.collect();
			chapters.reverse();
			manga.chapters = Some(chapters);
		}
		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let bid: i64 = manga.key.parse().map_err(|_| error!("[lightnovel] bad key {}", manga.key))?;

		if let Some(rest) = chapter.key.strip_prefix('c') {
			let (cid, count) = rest.split_once('.').unwrap_or((rest, "0"));
			let cid: i64 = cid.parse().map_err(|_| error!("[lightnovel] bad chapter {}", chapter.key))?;
			let listed: i64 = count.parse().unwrap_or(0);
			// Every image URL handed out costs one page of the daily comic quota, so only
			// the first batch is fetched here; later pages are placeholders that
			// get_image_request resolves a batch at a time as the reader reaches them.
			let first = call_one("GetComicContent", comic_args(cid, 0))?;
			let (total, skip, urls) = comic_batch(&first, 0);
			println!("[lightnovel] comic {cid}: asked skip 0, reply skip {skip}, total {total}, {} images", urls.len());
			let total = total.max(listed).max(skip + urls.len() as i64);
			comic_cache::store(cid, skip, &urls, true);
			let pages = (0..total)
				.map(|i: i64| {
					let url = usize::try_from(i - skip)
						.ok()
						.and_then(|at: usize| urls.get(at))
						.cloned()
						.unwrap_or_else(|| format!("{COMIC_SCHEME}{cid}/{i}"));
					Page {
						content: PageContent::url(url),
						..Default::default()
					}
				})
				.collect();
			return Ok(pages);
		}

		let sort: i64 = chapter
			.key
			.strip_prefix('n')
			.and_then(|s: &str| s.parse().ok())
			.ok_or_else(|| error!("[lightnovel] bad chapter {}", chapter.key))?;
		let value = call_one(
			"GetNovelContent",
			json!({ "Bid": bid, "SortNum": sort, "Convert": convert_setting() }),
		)?;
		let ch = field(&value, "chapter").ok_or_else(|| error!("[lightnovel] no chapter"))?;
		let html = str_field(ch, "content").unwrap_or("");
		let map = match str_field(ch, "font") {
			Some(path) => Some(font::map_for(path)?),
			None => None,
		};
		let markdown = content::chapter_markdown(html, map.as_ref());
		if vertical_layout() {
			return Ok(vertical::pages(&markdown, vertical::size_for(defaults_get::<String>("verticalSize").as_deref())));
		}
		Ok(vec![Page {
			content: PageContent::text(markdown),
			..Default::default()
		}])
	}
}

impl ListingProvider for LightnovelSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some((call, comic)) = listing_call(&listing.id, page) else {
			bail!("[lightnovel] unknown listing {}", listing.id);
		};
		// Rankings are one fixed list.
		if call.target == "GetRank" && page > 1 {
			return Ok(MangaPageResult::default());
		}
		let value = call_one(call.target, call.args)?;
		Ok(parse_page(&value, comic))
	}
}

/// Home rows: (heading, listing id and name). Names must match `res/source.json`.
const HOME_ROWS: [(&str, &str, &str); 4] = [
	("最近更新", "latest", "最近更新"),
	("日榜", "daily", "日榜"),
	("週榜", "weekly", "週榜"),
	("漫畫", "comic", "漫畫"),
];

impl Home for LightnovelSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let token = auth::access_token();
		let rows: &[(&str, &str, &str)] = if token.is_some() { &HOME_ROWS } else { &HOME_ROWS[..1] };
		send_partial_result(&HomePartialResult::Layout(HomeLayout {
			components: rows
				.iter()
				.map(|(title, _, _)| HomeComponent {
					title: Some(String::from(*title)),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				})
				.collect(),
		}));

		// One connection carries every row. Signed out, only the latest updates are
		// served, and through the one method that needs no login.
		let calls: Vec<Call> = if token.is_some() {
			rows.iter()
				.filter_map(|(_, id, _)| {
					let (mut call, _) = listing_call(id, 1)?;
					call.args["Size"] = json!(HOME_SIZE);
					Some(call)
				})
				.collect()
		} else {
			vec![Call::new("GetLatestBookList", with_ignore_flags(json!({ "Page": 1, "Size": HOME_SIZE })))]
		};
		let replies = if token.is_some() { invoke_signed_in(&calls)? } else { hub::invoke_all(None, &calls)? };

		for ((title, id, name), reply) in rows.iter().zip(replies) {
			let Ok(value) = reply.into_result() else {
				continue;
			};
			let mut entries = parse_page(&value, *id == "comic").entries;
			entries.truncate(HOME_SIZE as usize);
			if entries.is_empty() {
				continue;
			}
			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: Some(String::from(*title)),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: entries.into_iter().map(Link::from).collect(),
					// The listings need a login; signed out the row is all there is.
					listing: token.as_ref().map(|_| Listing {
						id: String::from(*id),
						name: String::from(*name),
						..Default::default()
					}),
				},
			}));
		}
		Ok(HomeLayout::default())
	}
}

impl ImageRequestProvider for LightnovelSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		let Some(rest) = url.strip_prefix(COMIC_SCHEME) else {
			return Ok(Request::get(url)?);
		};
		let (cid, index) = rest.split_once('/').ok_or_else(|| error!("[lightnovel] bad page {url}"))?;
		let cid: i64 = cid.parse().map_err(|_| error!("[lightnovel] bad page {url}"))?;
		let index: i64 = index.parse().map_err(|_| error!("[lightnovel] bad page {url}"))?;
		let (skip, urls) = match comic_cache::get(cid, index) {
			Some(batch) => {
				println!("[lightnovel] comic {cid}: page {index} from cached batch {}", batch.0);
				batch
			}
			None => {
				// Reached when the reader lands in a batch nobody fetched: resuming a
				// chapter at a saved page, or jumping with the slider. The app then asks
				// for its whole preload window, but it runs these calls one after another
				// (device, 2026-09-30: jumping to page 12 fetched the batch once and pages
				// 13-16 hit the cache within 10 ms of it), so a batch is fetched once. No
				// gate is needed: a caller that misses has nothing else to return anyway.
				let requested = index / COMIC_TAKE * COMIC_TAKE;
				let value = call_one("GetComicContent", comic_args(cid, requested))?;
				let (total, skip, urls) = comic_batch(&value, requested);
				println!(
					"[lightnovel] comic {cid}: page {index} asked skip {requested}, reply skip {skip}, total {total}, {} images",
					urls.len()
				);
				comic_cache::store(cid, skip, &urls, false);
				(skip, urls)
			}
		};
		let real = usize::try_from(index - skip)
			.ok()
			.and_then(|at: usize| urls.get(at))
			.ok_or_else(|| {
				println!("[lightnovel] comic {cid}: page {index} is not in batch {skip} of {} images", urls.len());
				error!("[lightnovel] no image {index} in batch {skip} of {cid}")
			})?;
		Ok(Request::get(real)?)
	}
}

impl DeepLinkHandler for LightnovelSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let path = url.split_once("://").map_or(url.as_str(), |(_, rest): (&str, &str)| rest);
		let path = path.split(['?', '#']).next().unwrap_or(path);
		let parts: Vec<&str> = path.split('/').skip(1).filter(|p: &&str| !p.is_empty()).collect();
		let number = |s: &str| s.parse::<i64>().ok().map(|n: i64| format!("{n}"));
		Ok(match parts.as_slice() {
			["book", "info", bid, ..] => number(bid).map(|key: String| DeepLinkResult::Manga { key }),
			["read", bid, sort, ..] => match (number(bid), number(sort)) {
				(Some(manga_key), Some(sort)) => Some(DeepLinkResult::Chapter {
					manga_key,
					key: format!("n{sort}"),
				}),
				_ => None,
			},
			// The reader link has no page count; open the book instead of a chapter.
			["manga", bid, ..] => number(bid).map(|key: String| DeepLinkResult::Manga { key }),
			_ => None,
		})
	}
}

impl BasicLoginHandler for LightnovelSource {
	fn handle_basic_login(&self, _key: String, username: String, password: String) -> Result<bool> {
		match auth::login(username.trim(), &auth::password_hash(&password)) {
			auth::LoginOutcome::LoggedIn => {
				auth::mark_just_logged_in();
				println!("[lightnovel] login successful");
				Ok(true)
			}
			// The app shows its own message either way (CLAUDE.md), so no Err for text.
			auth::LoginOutcome::Rejected => {
				println!("[lightnovel] login rejected");
				Ok(false)
			}
			auth::LoginOutcome::Unreachable => {
				println!("[lightnovel] login failed: site unreachable");
				Ok(false)
			}
		}
	}
}

impl NotificationHandler for LightnovelSource {
	fn handle_notification(&self, notification: String) {
		if notification == "login" {
			// Fired for both logging in and logging out; see auth::is_just_logged_in.
			if auth::is_just_logged_in() {
				println!("[lightnovel] login notification after a login");
				auth::clear_just_logged_in();
			} else {
				println!("[lightnovel] login notification without a recent login: logging out");
				auth::clear_auth();
				sign_in::clear();
			}
		}
	}
}

const SIGNED_OUT_HINT: &str = "尚未登入。登入後才能看書籍資訊、章節內容、搜尋與排行，這裡也會顯示閱讀等級與今日漫畫額度。";
const RELOGIN_HINT: &str = "登入已失效或密碼已變更，請先登出再重新登入。";

fn account_footer() -> String {
	let Ok(info) = call_one("GetMyInfo", json!({})) else {
		return String::from("無法取得帳號資訊，請檢查網路後重新開啟這一頁。");
	};
	let name = str_field(&info, "userName").unwrap_or("");
	let growth = field(&info, "growth");
	// Novella reads these under `Growth`; the top level is kept as a fallback.
	let stat = |key: &str| growth.and_then(|g: &Value| field(g, key)).or_else(|| field(&info, key));
	let level = stat("level").and_then(Value::as_i64).unwrap_or(0);
	let mut lines = vec![format!("已登入：{name}"), format!("閱讀等級：Lv.{level}")];
	if let (Some(exp), Some(next)) = (
		stat("exp").and_then(Value::as_i64),
		stat("nextLevelExp").and_then(Value::as_i64),
	) {
		lines.push(format!("經驗值：{exp} / {next}"));
	}
	if let Some(quota) = stat("comicQuotaToday").and_then(Value::as_i64) {
		lines.push(format!("今日漫畫額度剩餘：{quota} 頁"));
	}
	if let Some(coin) = stat("coin").and_then(Value::as_i64) {
		lines.push(format!("金幣：{coin}"));
	}
	if let Some(signed) = stat("todaySigned").and_then(Value::as_bool) {
		lines.push(String::from(if signed { "今日已簽到" } else { "今日尚未簽到" }));
	}
	if let Some(streak) = stat("signStreak").and_then(Value::as_i64) {
		lines.push(format!("連續簽到：{streak} 天"));
	}
	if let Some(error) = sign_in::last_error() {
		lines.push(format!("自動簽到失敗：{error}"));
	}
	lines.join("\n")
}

impl DynamicSettings for LightnovelSource {
	/// Never an empty list: a source that returns one gets a settings screen whose
	/// buttons stop responding (CLAUDE.md). Signed out, no request is made.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let footer = if auth::needs_relogin() {
			String::from(RELOGIN_HINT)
		} else if auth::is_logged_in() {
			account_footer()
		} else {
			String::from(SIGNED_OUT_HINT)
		};
		Ok(vec![GroupSetting {
			key: "accountInfo".into(),
			title: "帳號資訊".into(),
			items: Vec::new(),
			footer: Some(footer.into()),
			..Default::default()
		}
		.into()])
	}
}

register_source!(
	LightnovelSource,
	ListingProvider,
	Home,
	DeepLinkHandler,
	ImageRequestProvider,
	BasicLoginHandler,
	NotificationHandler,
	DynamicSettings
);

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn reads_both_casings() {
		let pascal = json!({ "Id": 7, "Title": "書", "Level": 4, "Type": "Novel", "Category": { "Name": "原创" } });
		let manga = parse_entry(&pascal, false).expect("entry");
		assert_eq!(manga.key, "7");
		assert_eq!(manga.tags, Some(vec![String::from("原创"), String::from("Lv.4")]));
		let camel = json!({ "id": 8, "title": "漫" });
		let manga = parse_entry(&camel, true).expect("entry");
		assert_eq!(manga.url.as_deref(), Some("https://www.lightnovel.app/manga/8"));
	}

	#[aidoku_test]
	fn level_tag_search_is_empty() {
		// Signed out on purpose: an empty page, not "login required", shows no request ran.
		let result = LightnovelSource::new()
			.get_search_manga_list(
				None,
				1,
				vec![FilterValue::Select {
					id: String::from("genre"),
					value: String::from("Lv.4"),
				}],
			)
			.expect("search");
		assert!(result.entries.is_empty() && !result.has_next_page);
		assert_eq!(novel_category_id("原创"), Some(7));
		assert_eq!(novel_category_id("百合"), None);
	}

	#[aidoku_test]
	fn next_page_from_envelope() {
		let book = json!({ "id": 1, "title": "書" });
		let page = |extra: Value| {
			let mut v = json!({ "data": [book] });
			for (k, val) in extra.as_object().expect("object") {
				v[k] = val.clone();
			}
			parse_page(&v, false).has_next_page
		};
		assert!(page(json!({ "hasMore": true, "page": 3, "totalPages": 3 })));
		assert!(!page(json!({ "hasMore": false, "page": 1, "totalPages": 3 })));
		assert!(page(json!({ "page": 1, "totalPages": 3 })));
		assert!(!page(json!({ "Page": 3, "TotalPages": 3 })));
		assert!(!page(json!({})));
		assert!(!parse_page(&json!({ "hasMore": true, "data": [] }), false).has_next_page);
		assert!(!parse_page(&json!([book]), false).has_next_page);
	}

	#[aidoku_test]
	fn iso_dates_are_utc() {
		assert_eq!(parse_iso("2026-09-30T03:40:39.059488Z"), Some(1_790_739_639));
		assert_eq!(parse_iso("2026-09-30T03:40:39Z"), Some(1_790_739_639));
		assert_eq!(parse_iso("2026-09-30"), None);
	}

	#[aidoku_test]
	fn comic_batches_by_reply_skip() {
		let reply = json!({ "chapter": { "total": 30, "skip": 12, "images": ["a.jpg?t=1", "b.jpg?t=2"] } });
		let (total, skip, urls) = comic_batch(&reply, 12);
		assert_eq!((total, skip, urls.len()), (30, 12, 2));
		let (_, skip, _) = comic_batch(&json!({ "Chapter": { "Images": [] } }), 24);
		assert_eq!(skip, 24);
		comic_cache::store(7, 12, &urls, true);
		assert_eq!(comic_cache::get(7, 13).map(|(s, u): (i64, Vec<String>)| (s, u.len())), Some((12, 2)));
		assert!(comic_cache::get(7, 14).is_none());
		assert!(comic_cache::get(8, 12).is_none());
		// Opening the chapter afresh drops what was cached for it.
		comic_cache::store(7, 0, &urls, true);
		assert!(comic_cache::get(7, 12).is_none());
		assert!(comic_cache::get(7, 0).is_some());
	}

	#[aidoku_test]
	fn level_from_message() {
		assert_eq!(required_level("阅读等级不足，需要达到 Lv.4 才能阅读该书籍"), Some(4));
		assert_eq!(required_level("无"), None);
	}

	#[aidoku_test]
	fn deep_links() {
		let source = LightnovelSource::new();
		let link = |url: &str| source.handle_deep_link(String::from(url)).expect("deep link");
		assert!(matches!(link("https://www.lightnovel.app/book/info/19367"), Some(DeepLinkResult::Manga { key }) if key == "19367"));
		assert!(matches!(link("https://www.lightnovel.app/read/19367/3"), Some(DeepLinkResult::Chapter { manga_key, key }) if manga_key == "19367" && key == "n3"));
		assert!(matches!(link("https://www.lightnovel.app/manga/18027/read/275368"), Some(DeepLinkResult::Manga { key }) if key == "18027"));
		assert!(link("https://www.lightnovel.app/home").is_none());
	}
}
