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
	/// 1 when the chapter is free to read right now, whatever `buy_coin` says.
	#[serde(default)]
	pub is_free: i32,
	// Which purchase routes the site actually offers. The amounts above stay populated
	// even for routes that are switched off, so these flags decide what to show.
	#[serde(default)]
	pub is_coin_buy: i32,
	#[serde(default)]
	pub is_point_buy: i32,
	#[serde(default)]
	pub is_coin_rent: i32,
	#[serde(default)]
	pub is_point_rent: i32,
	/// Days until the chapter becomes free, counted by the server. 0 or absent means
	/// there is no upcoming unlock.
	#[serde(default)]
	pub free_day: Option<i32>,
	/// A JSON array rendered as a string, holding the window when the chapter unlocks.
	/// It keeps windows that have already passed, so it is only read when `free_day`
	/// says an unlock is still ahead.
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
	/// 1400x758. Preferred over `image1`, which is 1200x400 and overflows the screen.
	#[serde(default)]
	pub image2: Option<String>,
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
		let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
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

fn names(list: Vec<Named>) -> Vec<String> {
	list.into_iter()
		.filter_map(|item| item.name)
		.filter(|name| !name.is_empty())
		.collect()
}

impl Book {
	pub fn into_manga(self) -> Manga {
		let key = format!("{}", self.id);
		let authors = names(self.author);

		// The genre doubles as a tag so it is searchable alongside the free-form ones.
		let mut tags = Vec::new();
		if let Some(name) = self.genre.and_then(|genre| genre.name) {
			tags.push(name);
		}
		tags.extend(names(self.tags));

		Manga {
			url: Some(manga_url(&key)),
			key,
			title: self.name.unwrap_or_default(),
			cover: self.image1,
			authors: (!authors.is_empty()).then_some(authors),
			description: self.description,
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
		let authors = names(self.author);
		Manga {
			url: Some(manga_url(&key)),
			key,
			title: self.name.unwrap_or_default(),
			cover: self.image1,
			authors: (!authors.is_empty()).then_some(authors),
			description: self.description,
			status: status_for(self.completed),
			content_rating: ContentRating::Safe,
			viewer: viewer_for(self.class),
			update_strategy: UpdateStrategy::Always,
			..Default::default()
		}
	}
}

/// Read a Chinese numeral up to 99 (`三`, `十二`, `二十`, `二十三`).
fn chinese_number(text: &str) -> Option<f32> {
	let digit = |c: char| "零一二三四五六七八九".chars().position(|d: char| d == c);
	let chars: Vec<char> = text.chars().collect();
	match chars.as_slice() {
		['十'] => Some(10.0),
		[c] => digit(*c).map(|d| d as f32),
		['十', ones] => digit(*ones).map(|d| 10.0 + d as f32),
		[tens, '十'] => digit(*tens).map(|d| d as f32 * 10.0),
		[tens, '十', ones] => Some((digit(*tens)? * 10 + digit(*ones)?) as f32),
		_ => None,
	}
}

#[derive(PartialEq)]
enum Unit {
	Chapter,
	Volume,
	None,
}

/// Classify a number by the English word right before it. `Season`/`S` mark a volume,
/// `Ep`/`Episode` mark a chapter, as the official multi.webtoon parser reads them.
fn english_unit(before: &[char]) -> Unit {
	let mut end = before.len();
	while end > 0 && (before[end - 1].is_whitespace() || before[end - 1] == '.') {
		end -= 1;
	}
	let mut start = end;
	while start > 0 && before[start - 1].is_ascii_alphabetic() {
		start -= 1;
	}
	let word: String = before[start..end]
		.iter()
		.map(|c| c.to_ascii_lowercase())
		.collect();
	match word.as_str() {
		"season" | "s" => Unit::Volume,
		"ep" | "episode" => Unit::Chapter,
		_ => Unit::None,
	}
}

/// Pull the chapter and volume numbers out of the site's own label, as
/// `(chapter_number, volume_number)`.
///
/// A number followed by a chapter unit (話 话 回 集 章 篇) is the chapter number and wins
/// over any number earlier in the label. A number followed by 卷 部 季 冊 册 is the
/// volume number, so "第1季第3話" reads as volume 1, chapter 3, the way the official
/// zh.copymanga reads it, and "第1季" alone is volume 1 with no chapter. When no number
/// carries a unit the first plain digit run is the chapter number, which covers labels
/// like "EP.5". Chinese numerals ("第十二話") only count when they carry a unit, so a word
/// like "一個瞬間" is not read as a number.
///
/// English labels count too: `Season N` is the volume and `Ep N` the chapter, so
/// "Season2．Ep5.5" reads as volume 2, chapter 5.5.
///
/// A label with no number in it (announcements, extras, the prologue) gives `None`;
/// `build_chapters` only numbers those by place when no label in the whole list has a
/// chapter number.
#[cfg(test)]
pub fn numbers(vol_name: Option<&str>) -> (Option<f32>, Option<f32>) {
	let (chapter, volume, _) = labelled_numbers(vol_name);
	(chapter, volume)
}

/// `numbers()` plus whether the chapter number had a unit (`話`, `Ep`) behind or in front
/// of it. A bare number is weaker evidence: the site sometimes puts the book's own name in
/// the label, as book 341 「16647」 does for its 上線預告 entry, so `build_chapters` drops a
/// bare chapter number when other labels in the list carry a unit.
pub fn labelled_numbers(vol_name: Option<&str>) -> (Option<f32>, Option<f32>, bool) {
	/// Units that mark the number in front of them as the chapter number.
	const CHAPTER_UNITS: [char; 6] = ['話', '话', '回', '集', '章', '篇'];
	/// Units that mark the number in front of them as the volume number.
	const VOLUME_UNITS: [char; 5] = ['卷', '部', '季', '冊', '册'];
	const CHINESE_DIGITS: &str = "零一二三四五六七八九十";

	let (label, characters): (&str, Vec<char>) = match vol_name {
		Some(label) => (label, label.chars().collect()),
		None => return (None, None, false),
	};
	let mut numbers: Vec<(f32, Unit)> = Vec::new();
	let mut index = 0;

	while index < characters.len() {
		let numeral = CHINESE_DIGITS.contains(characters[index]);
		if !characters[index].is_ascii_digit() && !numeral {
			index += 1;
			continue;
		}

		let start = index;
		let value = if numeral {
			while index < characters.len() && CHINESE_DIGITS.contains(characters[index]) {
				index += 1;
			}
			let text: String = characters[start..index].iter().collect();
			chinese_number(&text)
		} else {
			// Walk one run of digits, allowing a single decimal point that has another
			// digit behind it, so a trailing full stop is not swallowed.
			let mut seen_dot = false;
			while index < characters.len() {
				let character = characters[index];
				if character.is_ascii_digit() {
					index += 1;
				} else if character == '.'
					&& !seen_dot && characters
					.get(index + 1)
					.is_some_and(char::is_ascii_digit)
				{
					seen_dot = true;
					index += 1;
				} else {
					break;
				}
			}
			let digits: String = characters[start..index].iter().collect();
			digits.parse::<f32>().ok()
		};

		let unit = match characters[index..]
			.iter()
			.find(|character| !character.is_whitespace())
		{
			Some(character) if CHAPTER_UNITS.contains(character) => Unit::Chapter,
			Some(character) if VOLUME_UNITS.contains(character) => Unit::Volume,
			_ => Unit::None,
		};
		// English labels put the word in front: "Season2", "Ep5.5", "Episode 3".
		let unit = if unit == Unit::None && !numeral {
			english_unit(&characters[..start])
		} else {
			unit
		};
		// A Chinese numeral without a unit is part of a word, not a number.
		if let Some(value) = value.filter(|_| !numeral || unit != Unit::None) {
			numbers.push((value, unit));
		}
	}

	let find = |wanted: Unit| {
		numbers
			.iter()
			.find(|(_, unit)| *unit == wanted)
			.map(|(value, _)| *value)
	};
	let unit_chapter = find(Unit::Chapter);
	let chapter = unit_chapter.or_else(|| find(Unit::None));
	// A 卷 label followed by text that is not a chapter ("第3卷 後記", "06卷番外", "第3卷
	// 新的旅程") is an extra of that volume: it would share the volume's number with "第3卷"
	// itself, so it gets neither number, as W8's `numbers()` and the official zh.zaimanhua
	// (`is_extra`) do.
	let appendix = chapter.is_none()
		&& label
			.split_once('卷')
			.is_some_and(|(_, after)| !after.trim().is_empty());
	(
		chapter,
		find(Unit::Volume).filter(|_| !appendix),
		unit_chapter.is_some(),
	)
}

/// Words in an arc label that mark a special issue rather than the next arc. Book 276
/// numbers its 特刊 from 001 again, so their numbers would repeat the first arcs'.
const EXTRA_ARC_WORDS: [&str; 4] = ["特刊", "番外", "特別", "外傳"];

/// Whether this entry's `vol_name` names a story arc: more than one chapter shares it,
/// it holds a number, and that number is not a chapter number (`特別篇` shared by
/// several extras has no number, so it stays an ordinary label).
pub fn is_arc(entry: &ChapterEntry, all: &[ChapterEntry]) -> bool {
	let Some(label) = entry.vol_name.as_deref() else {
		return false;
	};
	let (chapter, volume, has_unit) = labelled_numbers(Some(label));
	!has_unit
		&& (chapter.is_some() || volume.is_some())
		&& all
			.iter()
			.filter(|other| other.vol_name.as_deref() == Some(label))
			.nth(1)
			.is_some()
}

/// Numbers for a chapter inside a story arc: the arc's number is the volume and the
/// `第N話` in `name` is the chapter. A special-issue arc (`【特刊001】`) gets neither, so it
/// never shares numbers with arc 001.
pub fn arc_numbers(entry: &ChapterEntry) -> (Option<f32>, Option<f32>, bool) {
	let label = entry.vol_name.as_deref().unwrap_or_default();
	if EXTRA_ARC_WORDS.iter().any(|word| label.contains(word)) {
		return (None, None, true);
	}
	let (bare, volume, _) = labelled_numbers(Some(label));
	let (chapter, _, has_unit) = labelled_numbers(entry.name.as_deref());
	(chapter.filter(|_| has_unit), volume.or(bare), true)
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

/// The purchase routes the site offers for a chapter this account can not read yet.
///
/// Empty when the chapter is already owned, rented or free, and also when the site
/// offers no way to pay for it (announcements and the like), because then there is
/// nothing the reader could do to unlock it.
fn open_prices(entry: &ChapterEntry) -> Vec<String> {
	// Already owned, or already free. `is_free` is what decides this: most back
	// catalogue chapters keep a non-zero `buy_coin` long after their free window opened,
	// so pricing off the coin fields alone marks free chapters as paid.
	if entry.is_buy == 1 || entry.is_rent == 1 || entry.is_free == 1 {
		return Vec::new();
	}

	// A paid chapter usually offers more than one route - buying outright with coins and
	// renting with points are priced separately - so every enabled one is listed.
	let mut prices: Vec<String> = Vec::new();
	let mut push = |amount: i32, unit: &str| {
		if amount > 0 {
			let label = format!("{amount}{unit}");
			if !prices.contains(&label) {
				prices.push(label);
			}
		}
	};
	if entry.is_coin_buy == 1 {
		push(entry.buy_coin, "金幣");
	}
	if entry.is_coin_rent == 1 {
		push(entry.rent_coin, "金幣");
	}
	if entry.is_point_buy == 1 {
		push(entry.buy_point, "點");
	}
	if entry.is_point_rent == 1 {
		push(entry.rent_point, "點");
	}
	prices
}

/// Whether this account can not read the chapter right now. The chapter list is
/// requested with the account's own credential, so `is_buy`, `is_rent` and `is_free`
/// already describe this account. Purchasing or renting on the site and refreshing the
/// manga page unlocks the chapter, so `locked` does not make it permanently unreadable.
pub fn chapter_locked(entry: &ChapterEntry) -> bool {
	!open_prices(entry).is_empty()
}

/// Build the chapter title, appending the cost and unlock date when the chapter is
/// paid. Aidoku shows no source-supplied error text when a chapter fails to open, so
/// the price is surfaced here where the reader can see it before tapping.
pub fn chapter_title(entry: &ChapterEntry) -> Option<String> {
	let base = entry
		.name
		.as_deref()
		.map(str::trim)
		.filter(|value| !value.is_empty())
		.map(String::from);

	let prices = open_prices(entry);
	if prices.is_empty() {
		return base;
	}

	let mut note = prices.join("／");
	// `free_date` also lists windows that already opened, so the server's countdown is
	// what decides whether there is a future unlock worth mentioning.
	if entry.free_day.unwrap_or(0) > 0 {
		if let Some(date) = free_date_label(entry.free_date.as_deref()) {
			note.push_str(&format!("・{date}免費"));
		}
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
		assert_eq!(numbers(Some("第 12 話")).0, Some(12.0));
		assert_eq!(numbers(Some("第4话")).0, Some(4.0));
		assert_eq!(numbers(Some("第 0.6 話")).0, Some(0.6));
	}

	/// A label that numbers a season as well as the chapter reports the season as the volume.
	/// CCC has not been seen using this form, so this pins the behaviour rather than
	/// recording an observation.
	#[aidoku_test]
	fn the_number_carrying_a_chapter_unit_wins() {
		assert_eq!(numbers(Some("第1季第3話")), (Some(3.0), Some(1.0)));
		assert_eq!(numbers(Some("第二季 第十二話")), (Some(12.0), Some(2.0)));
		// A season on its own is a volume, not chapter 1.
		assert_eq!(numbers(Some("第2季")), (None, Some(2.0)));
		assert_eq!(numbers(Some("第3卷")), (None, Some(3.0)));
		// A Chinese numeral without a unit is part of a word.
		assert_eq!(numbers(Some("一個瞬間")), (None, None));
		assert_eq!(numbers(Some("2026 新年特別篇 第 7 回")).0, Some(7.0));
		assert_eq!(numbers(Some("EP.5")).0, Some(5.0));
	}

	/// A trailing full stop is punctuation, not a decimal point.
	#[aidoku_test]
	fn a_trailing_dot_is_not_part_of_the_number() {
		assert_eq!(numbers(Some("第 3 話.")).0, Some(3.0));
		assert_eq!(numbers(Some("12.")).0, Some(12.0));
	}

	/// `numbers()` itself gives `None` for a label with no number in it; `build_chapters`
	/// then numbers such an entry by its place in the list.
	#[aidoku_test]
	fn labels_without_a_number_give_none() {
		assert_eq!(numbers(Some("公告")).0, None);
		assert_eq!(numbers(Some("序")).0, None);
		assert_eq!(numbers(Some("番外篇②")).0, None);
		assert_eq!(numbers(None).0, None);
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
			is_coin_buy: 1,
			rent_point: 600,
			is_point_rent: 1,
			free_day: Some(13),
			free_date: Some(String::from(
				"[[\"2026-10-05 20:00:00\", \"4000-01-01 00:00:00\"]]",
			)),
			..Default::default()
		};
		assert_eq!(
			chapter_title(&entry),
			Some(String::from("男性凝視・6金幣／600點・10/5免費"))
		);
	}

	/// Book 512 keeps `buy_coin: 6` on 41 back-catalogue chapters that are already free,
	/// each still carrying the long-past date its free window opened. Pricing off the
	/// coin fields alone labelled every one of them as paid.
	#[aidoku_test]
	fn already_free_chapters_carry_no_price() {
		let entry = ChapterEntry {
			name: Some(String::from("十年之後")),
			buy_coin: 6,
			is_free: 1,
			free_day: Some(0),
			free_date: Some(String::from(
				"[[\"2025-11-24 20:00:00\", \"4000-01-01 00:00:00\"]]",
			)),
			..Default::default()
		};
		assert_eq!(chapter_title(&entry), Some(String::from("十年之後")));
	}

	/// A paid chapter with no upcoming unlock must not advertise a date that has passed.
	#[aidoku_test]
	fn a_past_free_window_is_not_advertised() {
		let entry = ChapterEntry {
			name: Some(String::from("某話")),
			buy_coin: 6,
			is_coin_buy: 1,
			free_day: None,
			free_date: Some(String::from(
				"[[\"2025-11-24 20:00:00\", \"4000-01-01 00:00:00\"]]",
			)),
			..Default::default()
		};
		assert_eq!(chapter_title(&entry), Some(String::from("某話・6金幣")));
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

	/// The site leaves amounts populated on routes it has switched off, so only the
	/// enabled ones may be shown.
	#[aidoku_test]
	fn only_enabled_purchase_routes_are_listed() {
		let entry = ChapterEntry {
			name: Some(String::from("某話")),
			buy_coin: 6,
			is_coin_buy: 1,
			// Priced, but the site does not offer renting with points here.
			rent_point: 600,
			is_point_rent: 0,
			..Default::default()
		};
		assert_eq!(chapter_title(&entry), Some(String::from("某話・6金幣")));
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

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
