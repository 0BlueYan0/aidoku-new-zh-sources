use aidoku::{
	alloc::{collections::BTreeMap, string::ToString, vec, String, Vec},
	prelude::{bail, format},
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Result, Viewer,
};
use serde::Deserialize;

use crate::block::BlockState;
use crate::helpers::{format_names, get_img_base, parse_chapter_name, split_tags};
use crate::WEB_URL;

const PAGE_SIZE: i32 = 20;
const DELETED_TITLE: &str = "已刪除的内容";

/// Every answer the source reads carries `"status":"ok"`, empty pages included
/// (`{"info":[],"len":65890,"status":"ok"}`, checked 2026-11-22). Anything else is an error
/// answer, and parsing it as an empty page would empty the library on a refresh.
fn check_status(status: Option<&str>) -> Result<()> {
	if status == Some("ok") {
		Ok(())
	} else {
		bail!("[noyacg2] the site answered with status {status:?}")
	}
}

#[derive(Deserialize)]
pub struct SigninRecordResp {
	pub today: Option<bool>,
	pub continuous: Option<i64>,
}

/// `POST /api/v3/userinfo`.
#[derive(Deserialize)]
pub struct UserInfoResp {
	pub userinfo: Option<UserInfo>,
}

#[derive(Deserialize)]
pub struct UserInfo {
	#[serde(rename = "Username")]
	pub username: Option<String>,
	/// Points, named 積分 on the site.
	#[serde(rename = "Integral")]
	pub integral: Option<i64>,
}

#[derive(Deserialize)]
pub struct ListingResp {
	status: Option<String>,
	#[serde(alias = "info")]
	data: Option<Vec<ListingManga>>,
	#[serde(alias = "len")]
	count: Option<i32>,
}

impl ListingResp {
	pub fn into_page_result(self, page: i32, block: &BlockState) -> Result<MangaPageResult> {
		check_status(self.status.as_deref())?;
		let img_base = get_img_base();
		let raw = self.data.unwrap_or_default();
		// Decided on the unfiltered page, so a page whose works are all blocked does not end
		// the list. `len` ignores some filters (e.g. `finished=false`), so also stop at an
		// empty page.
		let has_next_page = !raw.is_empty() && page * PAGE_SIZE < self.count.unwrap_or(0);
		Ok(MangaPageResult {
			entries: visible(raw, block)
				.map(|m| m.into_basic_manga(&img_base))
				.collect(),
			has_next_page,
		})
	}
}

/// Drops the works the block list hides.
pub fn visible<'a>(
	entries: Vec<ListingManga>,
	block: &'a BlockState,
) -> impl Iterator<Item = ListingManga> + 'a {
	entries.into_iter().filter(move |m| !m.is_blocked(block))
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HomeResp {
	status: Option<String>,
	pub tags: Vec<String>,
	pub read_day: Vec<ListingManga>,
	pub fav_day: Vec<ListingManga>,
	pub proportion: Vec<ListingManga>,
	pub fs: Vec<ListingManga>,
}

impl HomeResp {
	pub fn check(&self) -> Result<()> {
		check_status(self.status.as_deref())
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListingManga {
	#[serde(rename = "Bid")]
	id: i64,
	mode: i32,
	#[serde(rename = "Bookname")]
	name: String,
	description: String,
	author: String,
	ptag: String,
	otag: String,
	time: i64,
	len: i32,
	status: i32,
	#[serde(default)]
	adult: i32,
	/// Characters, space separated like the tags.
	#[serde(default)]
	pname: String,
}

impl ListingManga {
	pub fn is_deleted(&self) -> bool {
		self.name == DELETED_TITLE
	}

	pub fn is_blocked(&self, block: &BlockState) -> bool {
		if block.is_empty() {
			return false;
		}
		block.blocks(
			&self.id.to_string(),
			[
				self.name.as_str(),
				self.author.as_str(),
				self.ptag.as_str(),
				self.otag.as_str(),
				self.pname.as_str(),
			],
		)
	}

	pub fn into_basic_manga(self, img_base: &str) -> Manga {
		let key = self.id.to_string();
		Manga {
			cover: Some(format!("{img_base}/{key}/m1.webp")),
			status: manga_status(self.mode, self.status),
			content_rating: content_rating(self.adult),
			title: self.name,
			key,
			..Default::default()
		}
	}

	fn tags(&self) -> Vec<String> {
		let mut tags = split_tags(&self.ptag);
		for tag in split_tags(&self.otag) {
			if !tags.contains(&tag) {
				tags.push(tag);
			}
		}
		tags
	}
}

#[derive(Deserialize)]
pub struct SearchResp {
	status: Option<String>,
	data: Option<Vec<SearchManga>>,
	count: Option<i32>,
}

impl SearchResp {
	pub fn into_page_result(self, page: i32, block: &BlockState) -> Result<MangaPageResult> {
		check_status(self.status.as_deref())?;
		let img_base = get_img_base();
		Ok(MangaPageResult {
			entries: self
				.data
				.unwrap_or_default()
				.into_iter()
				.filter(|m| !m.is_blocked(block))
				.map(|m| m.into_basic_manga(&img_base))
				.collect(),
			has_next_page: page * PAGE_SIZE < self.count.unwrap_or(0),
		})
	}
}

#[derive(Deserialize)]
struct SearchManga {
	id: i64,
	name: String,
	mode: Option<i32>,
	status: Option<i32>,
	#[serde(default)]
	adult: Option<i32>,
	#[serde(default)]
	author: Option<String>,
	#[serde(default)]
	tags: Vec<String>,
	#[serde(default)]
	otag: Vec<String>,
	#[serde(default)]
	pname: Vec<String>,
}

impl SearchManga {
	fn is_blocked(&self, block: &BlockState) -> bool {
		if block.is_empty() {
			return false;
		}
		block.blocks(
			&self.id.to_string(),
			[self.name.as_str()]
				.into_iter()
				.chain(self.author.as_deref())
				.chain(self.tags.iter().map(String::as_str))
				.chain(self.otag.iter().map(String::as_str))
				.chain(self.pname.iter().map(String::as_str)),
		)
	}

	fn into_basic_manga(self, img_base: &str) -> Manga {
		let key = self.id.to_string();
		Manga {
			cover: Some(format!("{img_base}/{key}/m1.webp")),
			title: self.name,
			status: manga_status(self.mode.unwrap_or(1), self.status.unwrap_or(0)),
			content_rating: content_rating(self.adult.unwrap_or(0)),
			key,
			..Default::default()
		}
	}
}

#[derive(Deserialize)]
pub struct BookDetailResp {
	status: Option<String>,
	pub book: Option<BookWrapper>,
	chapters: Option<ChaptersWrapper>,
}

#[derive(Deserialize)]
pub struct BookWrapper {
	pub info: Option<ListingManga>,
}

#[derive(Deserialize)]
struct ChaptersWrapper {
	categories: Option<Vec<Category>>,
	data: Option<BTreeMap<i64, Vec<ChapterEntry>>>,
}

#[derive(Deserialize)]
struct Category {
	id: i64,
	name: String,
}

#[derive(Deserialize)]
struct ChapterEntry {
	id: i64,
	name: String,
	#[serde(default)]
	count: i32,
	created_at: Option<i64>,
}

impl BookDetailResp {
	/// Only the status: an unknown id answers `ok` with the deleted placeholder book.
	pub fn check_status(&self) -> Result<()> {
		check_status(self.status.as_deref())
	}

	/// An error answer, or one without the book, must not reach the library: an empty
	/// chapter list would replace the stored chapters on a refresh.
	pub fn check(&self) -> Result<()> {
		self.check_status()?;
		if self.book.as_ref().and_then(|b| b.info.as_ref()).is_none() {
			bail!("[noyacg2] the book answer has no book info");
		}
		Ok(())
	}

	pub fn into_manga(self, key: &str) -> Option<Manga> {
		let m = self.book?.info?;
		let tags = m.tags();
		Some(Manga {
			key: key.into(),
			cover: Some(format!("{}/{key}/m1.webp", get_img_base())),
			url: Some(format!("{WEB_URL}/manga/{key}")),
			authors: format_names(&m.author).map(|a| vec![a]),
			description: (!m.description.is_empty()).then_some(m.description),
			tags: (!tags.is_empty()).then_some(tags),
			status: manga_status(m.mode, m.status),
			content_rating: content_rating(m.adult),
			// api has no reading direction field; site only hosts manga so RTL is assumed
			viewer: Viewer::RightToLeft,
			title: m.name,
			..Default::default()
		})
	}

	pub fn take_chapters(&mut self, manga_key: &str) -> Vec<Chapter> {
		if let Some(ChaptersWrapper {
			categories: Some(categories),
			data: Some(mut data),
		}) = self.chapters.take()
		{
			if categories.is_empty() {
				return self.single_chapter(manga_key);
			}
			return categories
				.iter()
				.rev()
				.flat_map(|category| {
					// extras are numbered on their own, so their numbers would clash with the main chapters
					let is_extra = category.name.contains("番外");
					data.remove(&category.id)
						.unwrap_or_default()
						.into_iter()
						.rev()
						.map(move |entry| {
							let (chapter_number, volume_number, title) = if is_extra {
								(None, None, Some(entry.name))
							} else {
								parse_chapter_name(entry.name)
							};
							Chapter {
								key: format!("{manga_key}/{}", entry.id),
								title,
								chapter_number,
								volume_number,
								scanlators: Some(vec![category.name.clone()]),
								date_uploaded: entry.created_at,
								..Default::default()
							}
						})
				})
				.collect();
		}
		self.single_chapter(manga_key)
	}

	/// A book without chapter categories is read as one chapter keyed by the book id.
	fn single_chapter(&self, manga_key: &str) -> Vec<Chapter> {
		let Some(info) = self.book.as_ref().and_then(|b| b.info.as_ref()) else {
			return Vec::new();
		};
		vec![Chapter {
			key: manga_key.into(),
			chapter_number: Some(1.0),
			date_uploaded: Some(info.time),
			..Default::default()
		}]
	}

	pub fn page_count(&self, chapter_id: Option<&str>) -> Option<i32> {
		match chapter_id {
			Some(id) => {
				let id: i64 = id.parse().ok()?;
				self.chapters
					.as_ref()?
					.data
					.as_ref()?
					.values()
					.flatten()
					.find(|entry| entry.id == id)
					.map(|entry| entry.count)
			}
			None => self.book.as_ref()?.info.as_ref().map(|info| info.len),
		}
	}
}

fn manga_status(mode: i32, status: i32) -> MangaStatus {
	if mode == 0 || status == 1 {
		MangaStatus::Completed
	} else {
		MangaStatus::Ongoing
	}
}

fn content_rating(adult: i32) -> ContentRating {
	if adult == 1 {
		ContentRating::NSFW
	} else {
		ContentRating::Suggestive
	}
}
