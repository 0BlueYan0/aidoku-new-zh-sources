//! Response shapes and their conversion into Aidoku models.

use crate::net;
use aidoku::{
	alloc::{string::ToString, String, Vec},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Viewer,
};
use serde::Deserialize;

/// Works are shown on the PicaWeb site, which is reachable where the official site is not.
const WORK_URL: &str = "https://picawang.com/comic/";

#[derive(Deserialize)]
pub struct Thumb {
	#[serde(rename = "fileServer")]
	pub file_server: String,
	pub path: String,
}

#[derive(Deserialize)]
pub struct ComicItem {
	pub _id: String,
	pub title: String,
	#[serde(default)]
	pub author: String,
	#[serde(default)]
	pub description: Option<String>,
	pub thumb: Thumb,
	#[serde(default)]
	pub categories: Vec<String>,
	#[serde(default)]
	pub tags: Option<Vec<String>>,
	#[serde(default)]
	pub finished: bool,
	#[serde(rename = "pagesCount", default)]
	pub pages_count: Option<i32>,
	#[serde(rename = "likesCount", default)]
	pub likes_count: Option<i32>,
	#[serde(rename = "totalLikes", default)]
	pub total_likes: Option<i32>,
}

#[derive(Deserialize)]
pub struct ComicResponse {
	pub data: ComicData,
}

#[derive(Deserialize)]
pub struct ComicData {
	pub comic: ComicItem,
}

#[derive(Deserialize)]
pub struct ExploreResponse {
	pub data: ExploreData,
}

#[derive(Deserialize)]
pub struct ExploreData {
	pub comics: ComicsData,
}

#[derive(Deserialize)]
pub struct ComicsData {
	pub docs: Vec<ComicItem>,
	pub page: i32,
	pub pages: i32,
}

#[derive(Deserialize)]
pub struct RankResponse {
	pub data: RankData,
}

#[derive(Deserialize)]
pub struct RankData {
	pub comics: Vec<ComicItem>,
}

#[derive(Deserialize)]
pub struct ChapterItem {
	pub order: i32,
	pub title: String,
	#[serde(default)]
	pub updated_at: Option<String>,
}

#[derive(Deserialize)]
pub struct ChapterResponse {
	pub data: ChapterData,
}

#[derive(Deserialize)]
pub struct ChapterData {
	pub eps: EpsData,
}

#[derive(Deserialize)]
pub struct EpsData {
	pub docs: Vec<ChapterItem>,
	pub pages: i32,
}

#[derive(Deserialize)]
pub struct Media {
	#[serde(rename = "fileServer")]
	pub file_server: String,
	pub path: String,
}

#[derive(Deserialize)]
pub struct PageItem {
	pub media: Media,
}

#[derive(Deserialize)]
pub struct PageResponse {
	pub data: PageData,
}

#[derive(Deserialize)]
pub struct PageData {
	pub pages: PagesData,
}

#[derive(Deserialize)]
pub struct PagesData {
	pub docs: Vec<PageItem>,
	pub pages: i32,
}

impl ComicItem {
	/// `line` is the selected API line, which decides the image host of the cover.
	pub fn into_manga(self, line: &str) -> Manga {
		let authors: Vec<String> = self
			.author
			.split('&')
			.map(|name| name.trim())
			.filter(|name| !name.is_empty())
			.map(String::from)
			.collect();
		let viewer = if self.categories.iter().any(|tag| tag.contains("WEBTOON")) {
			Viewer::Webtoon
		} else {
			Viewer::RightToLeft
		};

		let mut description = Vec::new();
		if let Some(count) = self.total_likes.or(self.likes_count) {
			description.push(format!("{count} 個愛心"));
		}
		if let Some(count) = self.pages_count {
			description.push(format!("頁數：{count}P"));
		}
		if let Some(text) = self.description.as_deref().map(str::trim) {
			if !text.is_empty() {
				description.push(format!("簡介：{text}"));
			}
		}

		let mut tags = self.tags.unwrap_or_default();
		tags.extend(self.categories);

		Manga {
			cover: Some(net::image_url(
				&self.thumb.file_server,
				&self.thumb.path,
				line,
			)),
			title: self.title,
			authors: if authors.is_empty() {
				None
			} else {
				Some(authors)
			},
			description: if description.is_empty() {
				None
			} else {
				Some(description.join("  \n"))
			},
			url: Some(format!("{WORK_URL}{}", self._id)),
			tags: if tags.is_empty() { None } else { Some(tags) },
			status: if self.finished {
				MangaStatus::Completed
			} else {
				MangaStatus::Ongoing
			},
			// The site is an adult-oriented doujinshi site and gives no rating per work.
			content_rating: ContentRating::NSFW,
			viewer,
			key: self._id,
			..Default::default()
		}
	}
}

/// Whether any tag of the work is one of the blocked keywords.
pub fn hit_blocklist(manga: &Manga, blocklist: &[String]) -> bool {
	manga
		.tags
		.as_ref()
		.is_some_and(|tags| tags.iter().any(|tag| blocklist.contains(tag)))
}

fn visible(docs: Vec<ComicItem>, line: &str, blocklist: &[String]) -> Vec<Manga> {
	docs.into_iter()
		.map(|item| item.into_manga(line))
		.filter(|manga| !hit_blocklist(manga, blocklist))
		.collect()
}

impl ExploreData {
	pub fn into_page(self, line: &str, blocklist: &[String]) -> MangaPageResult {
		let has_next_page = self.comics.page < self.comics.pages;
		MangaPageResult {
			entries: visible(self.comics.docs, line, blocklist),
			has_next_page,
		}
	}
}

impl RankData {
	pub fn into_page(self, line: &str, blocklist: &[String]) -> MangaPageResult {
		MangaPageResult {
			entries: visible(self.comics, line, blocklist),
			has_next_page: false,
		}
	}
}

impl ChapterItem {
	pub fn into_chapter(self) -> Chapter {
		Chapter {
			key: self.order.to_string(),
			title: Some(self.title).filter(|title| !title.trim().is_empty()),
			chapter_number: Some(self.order as f32),
			date_uploaded: self.updated_at.as_deref().and_then(parse_iso8601),
			..Default::default()
		}
	}
}

impl PageItem {
	pub fn into_page(self, line: &str) -> Page {
		Page {
			content: PageContent::url(net::image_url(
				&self.media.file_server,
				&self.media.path,
				line,
			)),
			..Default::default()
		}
	}
}

/// Unix seconds of `YYYY-MM-DDTHH:MM:SS[.fff]Z`. Any other shape, including an offset, is
/// not read.
pub fn parse_iso8601(text: &str) -> Option<i64> {
	let bytes = text.as_bytes();
	if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
		return None;
	}
	if bytes[13] != b':' || bytes[16] != b':' {
		return None;
	}
	let number = |range: core::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
	let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
	let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
	let rest = &text[19..];
	let zone_ok = rest == "Z"
		|| rest
			.strip_prefix('.')
			.and_then(|fraction| fraction.strip_suffix('Z'))
			.is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
	if !zone_ok
		|| !(1..=12).contains(&month)
		|| !(1..=31).contains(&day)
		|| hour > 23
		|| minute > 59
		|| second > 60
	{
		return None;
	}
	Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
	let year = if month <= 2 { year - 1 } else { year };
	let era = year.div_euclid(400);
	let year_of_era = year - era * 400;
	let month_index = (month + 9) % 12;
	let day_of_year = (153 * month_index + 2) / 5 + day - 1;
	let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
	era * 146097 + day_of_era - 719468
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn iso_dates() {
		assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
		assert_eq!(parse_iso8601("2026-01-01T00:00:00.000Z"), Some(1767225600));
		assert_eq!(parse_iso8601("2026-11-07T11:59:03.219Z"), Some(1794052743));
		assert_eq!(parse_iso8601("2024-02-29T00:00:00Z"), Some(1709164800));
		assert_eq!(parse_iso8601("2026-11-07T11:59:03+08:00"), None);
		assert_eq!(parse_iso8601("2026-11-07"), None);
		assert_eq!(parse_iso8601("2026-13-07T11:59:03Z"), None);
		assert_eq!(parse_iso8601(""), None);
	}

	#[aidoku_test]
	fn blocklist_matches_whole_tags() {
		let manga = Manga {
			tags: Some(Vec::from([String::from("全彩"), String::from("巨乳")])),
			..Default::default()
		};
		assert!(hit_blocklist(&manga, &[String::from("巨乳")]));
		assert!(!hit_blocklist(&manga, &[String::from("乳")]));
		assert!(!hit_blocklist(&manga, &[]));
		assert!(!hit_blocklist(&Manga::default(), &[String::from("巨乳")]));
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/json.rs"]
mod fixture_tests;
