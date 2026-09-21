//! API models, URL building and the shared request helper.
//!
//! Everything CCC serves comes from a JSON API, so the response shapes are modelled
//! with serde rather than scanned by hand: the payloads nest several levels deep
//! (`author[]`, `tags[]`, `proportion[]`) and hand-rolled scanning would be brittle.

use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::parse_date_with_options},
	prelude::*,
	ContentRating, Manga, MangaStatus, Result, UpdateStrategy, Viewer,
};
use serde::Deserialize;

use crate::auth;

pub const BASE_URL: &str = "https://www.creative-comic.tw";
pub const API_URL: &str = "https://api.creative-comic.tw";

/// The API rejects anything but `web_mobile` for this header.
pub const DEVICE: &str = "web_mobile";
pub const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

/// Matches the page size the website itself requests.
pub const PAGE_SIZE: i32 = 24;

/// 1 is the low-resolution variant, 2 the one the web reader defaults to.
pub const IMAGE_QUALITY: i32 = 2;

const DATE_FORMAT: &str = "yyyy-MM-dd HH:mm:ss";
const TIMEZONE: &str = "Asia/Taipei";

// ---------------------------------------------------------------------------
// Response models
// ---------------------------------------------------------------------------

/// Every endpoint wraps its payload in `{code, message, data}`. `code` is 0 on success;
/// the transport status is 500 for API-level errors, so `code` is what matters.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
pub struct Envelope<T> {
	#[serde(default)]
	pub code: i32,
	#[serde(default)]
	pub message: String,
	#[serde(default)]
	pub data: Option<T>,
}

#[derive(Deserialize)]
pub struct Paged {
	#[serde(default)]
	pub total: i32,
	#[serde(default)]
	pub data: Vec<Book>,
}

/// `{id, name}` shows up for authors, tags, genres and publishers alike.
#[derive(Deserialize, Default)]
pub struct Named {
	#[serde(default)]
	pub name: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Book {
	pub id: i64,
	#[serde(default)]
	pub name: Option<String>,
	#[serde(default)]
	pub description: Option<String>,
	#[serde(default)]
	pub image1: Option<String>,
	/// Present on `/book` and `/book/{id}/info`, absent from the ranking payload.
	#[serde(default, rename = "type")]
	pub genre: Option<Named>,
	#[serde(default)]
	pub author: Vec<Named>,
	#[serde(default)]
	pub tags: Vec<Named>,
	/// 0 = still running, 1 = finished. The sibling `serial` field contradicts itself
	/// (books marked "suspended" report `completed = 1`), so only this one is trusted.
	#[serde(default)]
	pub completed: i32,
	/// 3 = vertical scroll, 2 = page-by-page. Absent from list responses.
	#[serde(default)]
	pub class: i32,
}

#[derive(Deserialize)]
pub struct ChapterList {
	#[serde(default)]
	pub chapters: Vec<ChapterEntry>,
}

#[derive(Deserialize, Default)]
pub struct ChapterEntry {
	pub id: i64,
	#[serde(default)]
	pub name: Option<String>,
	/// The site's own numbering label, for example the string for episode 1.
	#[serde(default)]
	pub vol_name: Option<String>,
	#[serde(default)]
	pub idx: i32,
	#[serde(default)]
	pub online_at: Option<String>,
	#[serde(default)]
	pub buy_coin: i32,
	#[serde(default)]
	pub buy_point: i32,
	#[serde(default)]
	pub rent_coin: i32,
	#[serde(default)]
	pub rent_point: i32,
	#[serde(default)]
	pub is_buy: i32,
	#[serde(default)]
	pub is_rent: i32,
	/// A JSON array rendered as a string, holding the window when the chapter unlocks.
	#[serde(default)]
	pub free_date: Option<String>,
}

#[derive(Deserialize)]
pub struct ChapterContent {
	#[serde(default)]
	pub chapter: Option<ChapterDetail>,
}

#[derive(Deserialize)]
pub struct ChapterDetail {
	/// The owning book id, needed to resolve reader deep links.
	#[serde(default)]
	pub book: i64,
	/// One entry per page, in reading order.
	#[serde(default)]
	pub proportion: Vec<Proportion>,
}

#[derive(Deserialize)]
pub struct Proportion {
	pub id: i64,
}

#[derive(Deserialize)]
pub struct ImageKey {
	pub key: String,
}

#[derive(Deserialize)]
pub struct HomeV2 {
	#[serde(default)]
	pub banner: Vec<Banner>,
	#[serde(default)]
	pub rank: Option<Rank>,
}

#[derive(Deserialize)]
pub struct Banner {
	#[serde(default)]
	pub title: Option<String>,
	#[serde(default)]
	pub image1: Option<String>,
	/// One of "book", "announcement" or "url"; only "book" can open a manga.
	#[serde(default, rename = "type")]
	pub kind: Option<String>,
	#[serde(default)]
	pub value: Option<String>,
}

#[derive(Deserialize)]
pub struct Rank {
	#[serde(default)]
	pub read: Vec<RankEntry>,
}

/// The ranking payload is shaped differently from `Book` - `type` is an integer here
/// and the book id lives in `book`, not `id` - so it gets its own model.
#[derive(Deserialize)]
pub struct RankEntry {
	#[serde(default)]
	pub book: i64,
	#[serde(default)]
	pub name: Option<String>,
	#[serde(default)]
	pub description: Option<String>,
	#[serde(default)]
	pub image1: Option<String>,
	#[serde(default)]
	pub author: Vec<Named>,
	#[serde(default)]
	pub class: i32,
	#[serde(default)]
	pub completed: i32,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Build an authenticated GET against the API host.
pub fn api_request(path: &str) -> Result<Request> {
	let url = format!("{API_URL}{path}");
	let request = Request::get(&url)?
		.header("User-Agent", USER_AGENT)
		.header("device", DEVICE)
		.header("Accept-Language", "zh");
	Ok(auth::authorize(request))
}

/// Send a request and unwrap the `{code, message, data}` envelope.
pub fn read_envelope<T: serde::de::DeserializeOwned>(request: Request) -> Result<T> {
	let envelope: Envelope<T> = request.json_owned()?;
	if envelope.code != 0 {
		bail!("API error {}: {}", envelope.code, envelope.message);
	}
	envelope
		.data
		.ok_or_else(|| error!("API returned an empty payload"))
}

pub fn api_get<T: serde::de::DeserializeOwned>(path: &str) -> Result<T> {
	read_envelope(api_request(path)?)
}

// ---------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------

pub fn manga_url(id: &str) -> String {
	format!("{BASE_URL}/book/{id}/info")
}

pub fn chapter_url(id: &str) -> String {
	format!("{BASE_URL}/reader_comic/{id}")
}

/// The encrypted page image. Served without any headers at all.
pub fn page_image_url(page_id: i64) -> String {
	format!("{BASE_URL}/fs/chapter_content/encrypt/{page_id}/{IMAGE_QUALITY}")
}

/// Build the `/book` query for a listing, search or filtered browse.
pub fn book_list_path(
	page: i32,
	keyword: Option<&str>,
	sort_by: &str,
	genre: Option<&str>,
	updated_at: Option<&str>,
) -> String {
	let mut path = format!("/book?page={page}&rows_per_page={PAGE_SIZE}&sort_by={sort_by}");
	if let Some(keyword) = keyword.map(str::trim).filter(|value| !value.is_empty()) {
		path.push_str("&keyword=");
		path.push_str(&encode_component(keyword));
	}
	if let Some(genre) = genre.filter(|value| !value.is_empty()) {
		path.push_str("&type=");
		path.push_str(genre);
	}
	if let Some(window) = updated_at.filter(|value| !value.is_empty()) {
		path.push_str("&updated_at=");
		path.push_str(window);
	}
	path
}

/// Percent-encode everything outside the unreserved set, so CJK keywords survive.
pub fn encode_component(value: &str) -> String {
	const HEX: &[u8; 16] = b"0123456789ABCDEF";
	let mut out = String::new();
	for byte in value.as_bytes() {
		let unreserved = byte.is_ascii_alphanumeric()
			|| matches!(byte, b'-' | b'_' | b'.' | b'~');
		if unreserved {
			out.push(*byte as char);
		} else {
			out.push('%');
			out.push(HEX[(byte >> 4) as usize] as char);
			out.push(HEX[(byte & 0x0f) as usize] as char);
		}
	}
	out
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// 3 is vertical scroll; everything else on CCC is a page-by-page Taiwanese comic,
/// which reads left to right.
fn viewer_for(class: i32) -> Viewer {
	if class == 3 {
		Viewer::Webtoon
	} else {
		Viewer::LeftToRight
	}
}

fn status_for(completed: i32) -> MangaStatus {
	if completed == 1 {
		MangaStatus::Completed
	} else {
		MangaStatus::Ongoing
	}
}

fn names(list: &[Named]) -> Vec<String> {
	list.iter()
		.filter_map(|item| item.name.clone())
		.filter(|name| !name.is_empty())
		.collect()
}

impl Book {
	pub fn into_manga(self) -> Manga {
		let key = format!("{}", self.id);
		let authors = names(&self.author);

		// The genre doubles as a tag so it is searchable alongside the free-form ones.
		let mut tags = Vec::new();
		if let Some(name) = self.genre.as_ref().and_then(|genre| genre.name.clone()) {
			tags.push(name);
		}
		tags.extend(names(&self.tags));

		Manga {
			key: key.clone(),
			title: self.name.unwrap_or_default(),
			cover: self.image1,
			authors: (!authors.is_empty()).then_some(authors),
			description: self.description,
			url: Some(manga_url(&key)),
			tags: (!tags.is_empty()).then_some(tags),
			status: status_for(self.completed),
			content_rating: ContentRating::Safe,
			viewer: viewer_for(self.class),
			update_strategy: UpdateStrategy::Always,
			..Default::default()
		}
	}
}

impl RankEntry {
	pub fn into_manga(self) -> Manga {
		let key = format!("{}", self.book);
		let authors = names(&self.author);
		Manga {
			key: key.clone(),
			title: self.name.unwrap_or_default(),
			cover: self.image1,
			authors: (!authors.is_empty()).then_some(authors),
			description: self.description,
			url: Some(manga_url(&key)),
			status: status_for(self.completed),
			content_rating: ContentRating::Safe,
			viewer: viewer_for(self.class),
			update_strategy: UpdateStrategy::Always,
			..Default::default()
		}
	}
}

/// Pull a chapter number out of the site's own label, falling back to the sequence
/// index for prologues and extras that carry no number.
pub fn chapter_number(vol_name: Option<&str>, idx: i32) -> Option<f32> {
	if let Some(label) = vol_name {
		let mut digits = String::new();
		let mut seen_dot = false;
		for character in label.chars() {
			if character.is_ascii_digit() {
				digits.push(character);
			} else if character == '.' && !digits.is_empty() && !seen_dot {
				seen_dot = true;
				digits.push(character);
			} else if !digits.is_empty() {
				break;
			}
		}
		if let Ok(value) = digits.trim_end_matches('.').parse::<f32>() {
			return Some(value);
		}
	}
	(idx > 0).then_some(idx as f32)
}

/// The first timestamp inside the `free_date` blob, shortened to month/day.
fn free_date_label(free_date: Option<&str>) -> Option<String> {
	let raw = free_date?;
	let start = raw.find(|c: char| c.is_ascii_digit())?;
	let date = raw.get(start..start + 10)?;
	let mut parts = date.split('-');
	let _year = parts.next()?;
	let month = parts.next()?.trim_start_matches('0');
	let day = parts.next()?.trim_start_matches('0');
	if month.is_empty() || day.is_empty() {
		return None;
	}
	Some(format!("{month}/{day}"))
}

/// Build the chapter title, appending the cost and unlock date when the chapter is
/// paid. Aidoku shows no source-supplied error text when a chapter fails to open, and
/// marking it `locked` would make it untappable even after purchase, so the price is
/// surfaced here where the reader can see it before tapping.
pub fn chapter_title(entry: &ChapterEntry) -> Option<String> {
	let base = entry
		.name
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(String::from);

	// Already owned, or free to begin with.
	let owned = entry.is_buy == 1 || entry.is_rent == 1;
	let coins = entry.buy_coin.max(entry.rent_coin);
	let points = entry.buy_point.max(entry.rent_point);
	if owned || (coins == 0 && points == 0) {
		return base;
	}

	let mut note = if coins > 0 {
		format!("{coins}金幣")
	} else {
		format!("{points}點")
	};
	if let Some(date) = free_date_label(entry.free_date.as_deref()) {
		note.push_str(&format!("・{date}免費"));
	}

	Some(match base {
		Some(title) => format!("{title}・{note}"),
		None => note,
	})
}

pub fn parse_timestamp(value: Option<&str>) -> Option<i64> {
	let raw = value?.trim();
	if raw.is_empty() {
		return None;
	}
	parse_date_with_options(raw, DATE_FORMAT, "zh_TW", TIMEZONE)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn reads_the_number_out_of_the_sites_label() {
		assert_eq!(chapter_number(Some("第 12 話"), 30), Some(12.0));
		assert_eq!(chapter_number(Some("第4话"), 30), Some(4.0));
		assert_eq!(chapter_number(Some("第 0.6 話"), 30), Some(0.6));
	}

	/// Prologues and extras carry no digits, so the sequence index stands in.
	#[aidoku_test]
	fn falls_back_to_the_sequence_index() {
		assert_eq!(chapter_number(Some("序"), 1), Some(1.0));
		assert_eq!(chapter_number(None, 7), Some(7.0));
		assert_eq!(chapter_number(Some("番外"), 0), None);
	}

	#[aidoku_test]
	fn leaves_free_chapters_untouched() {
		let entry = ChapterEntry {
			name: Some(String::from("十年之後")),
			..Default::default()
		};
		assert_eq!(chapter_title(&entry), Some(String::from("十年之後")));
	}

	#[aidoku_test]
	fn annotates_paid_chapters_with_cost_and_unlock_date() {
		let entry = ChapterEntry {
			name: Some(String::from("男性凝視")),
			buy_coin: 6,
			rent_point: 600,
			free_date: Some(String::from(
				"[[\"2026-10-05 20:00:00\", \"4000-01-01 00:00:00\"]]",
			)),
			..Default::default()
		};
		assert_eq!(
			chapter_title(&entry),
			Some(String::from("男性凝視・6金幣・10/5免費"))
		);
	}

	/// A chapter the reader already bought should not keep advertising its price.
	#[aidoku_test]
	fn drops_the_note_once_the_chapter_is_owned() {
		let entry = ChapterEntry {
			name: Some(String::from("男性凝視")),
			buy_coin: 6,
			is_buy: 1,
			..Default::default()
		};
		assert_eq!(chapter_title(&entry), Some(String::from("男性凝視")));
	}

	#[aidoku_test]
	fn percent_encodes_cjk_keywords() {
		assert_eq!(encode_component("鬼"), "%E9%AC%BC");
		assert_eq!(encode_component("a-b_c.d~e"), "a-b_c.d~e");
		assert_eq!(encode_component("a b"), "a%20b");
	}

	#[aidoku_test]
	fn builds_a_book_query() {
		let path = book_list_path(2, Some("鬼"), "read_count", Some("6"), None);
		assert!(path.starts_with("/book?page=2&rows_per_page=24&sort_by=read_count"));
		assert!(path.contains("&keyword=%E9%AC%BC"));
		assert!(path.contains("&type=6"));
		assert!(!path.contains("updated_at"));
	}
}
