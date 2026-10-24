#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga, MangaPageResult,
	MangaStatus, MigrationHandler, Page, Result, Source,
};

mod helper;
mod tags;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 3] = [
	("最近更新", "update"),
	("人氣排行", "hits"),
	("最新上架", "new"),
];

/// Requests per `send_all`: the app runs at most 5 at once, and more in one call can hang (docs/aidoku-rs-api.md).
const HOME_BATCH: usize = 5;

/// Shown as the description of a work the site has removed.
const DELISTED_NOTE: &str = "站方已下架這部作品。";

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

struct Wmh1234Source;

impl Source for Wmh1234Source {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut author: Option<String> = None;
		let mut tag: Option<String> = None;
		let mut unknown_tag = false;
		let mut status: Option<String> = None;
		let mut order: Option<String> = None;
		for filter in filters {
			match filter {
				// A tapped author. The site's search also matches author names.
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				FilterValue::Select { id, value } => match id.as_str() {
					// `filters.json` supplies `ids`, so `value` is the site's value. The app
					// only sends a select whose value is not the default ("0", "all").
					"order" => order = Some(value),
					"status" => status = Some(value),
					"tag" => tag = Some(value),
					// A tapped tag sends its name.
					"genre" => match tags::tag_id(value.trim()) {
						Some(id) => tag = Some(String::from(id)),
						None => {
							unknown_tag = true;
						}
					},
					_ => {}
				},
				_ => {}
			}
		}

		let query = query
			.filter(|q: &String| !q.trim().is_empty())
			.or(author)
			.filter(|q: &String| !q.trim().is_empty());
		if let Some(query) = query {
			return Ok(parse_manga_list(&fetch_html(&search_url(&query, page))?));
		}
		if unknown_tag {
			return Ok(empty_page());
		}
		Ok(parse_manga_list(&fetch_html(&browse_url(
			tag.as_deref(),
			status.as_deref(),
			order.as_deref(),
			page,
		))?))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		// Details and chapters are on the same page.
		let Some(html) = fetch_details_page(&manga.key)? else {
			// Keep any chapters the library already has.
			if needs_details {
				manga.status = MangaStatus::Cancelled;
				manga.description = Some(String::from(DELISTED_NOTE));
				send_partial_result(&manga);
			}
			return Ok(manga);
		};

		if needs_details {
			if let Some(details) = parse_details(&html, &manga.key) {
				let chapters = manga.chapters.take();
				manga = details;
				manga.chapters = chapters;
				send_partial_result(&manga);
			}
		}

		if needs_chapters {
			// Some works list no chapters at all; an empty list must not empty the
			// chapters the library already has.
			let chapters = parse_chapters(&html);
			if !chapters.is_empty() {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		// Chapter keys are chapter ids since version 4. A token key is one the app kept
		// when the user declined (or failed) the key migration; it opens the reader as before.
		let reader = if is_id(&chapter.key) {
			fetch_reader_url(&manga.key, &chapter.key)?
		} else {
			reader_url(&chapter.key)
		};
		// A chapter the site is still copying answers 503 "内容准备中"; `fetch_html` fails on it.
		let pages = parse_pages(&fetch_html(&reader)?)?;
		if pages.is_empty() {
			bail!("[wmh1234] no images for chapter {}", chapter.key);
		}
		Ok(pages)
	}
}

impl ListingProvider for Wmh1234Source {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some(url) = listing_url(&listing.id, page) else {
			bail!("Unknown listing: {}", listing.id);
		};
		Ok(parse_manga_list(&fetch_html(&url)?))
	}
}

impl Home for Wmh1234Source {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately.
		let components: Vec<HomeComponent> = HOME_ROWS
			.iter()
			.map(|(title, _)| HomeComponent {
				title: Some(String::from(*title)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			})
			.collect();
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		// Rows go out in parallel so the home screen, and the covers queued behind this call,
		// are not held up by three requests in a row.
		// A row whose request cannot be built is left out and stays empty; the others still load.
		let mut rows = Vec::new();
		let mut requests: Vec<Request> = Vec::new();
		for (title, id) in HOME_ROWS {
			let Some((url, Ok(request))) = listing_url(id, 1).map(|url: String| {
				let request = request(&url);
				(url, request)
			}) else {
				continue;
			};
			rows.push((title, id, url));
			requests.push(request);
		}
		let mut responses = Vec::new();
		let mut requests = requests.into_iter().peekable();
		while requests.peek().is_some() {
			let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
			responses.extend(Request::send_all(batch));
		}

		// Drawn one at a time so a short response list leaves the trailing rows empty instead
		// of shifting results onto the wrong titles.
		let mut responses = responses.into_iter();
		for (title, id, url) in rows {
			// A row that fails stays empty; the others still load. `read_html` keeps
			// `fetch_html`'s non-2xx check, so an error page is not read as an empty row.
			let entries = match responses.next().map(|response| read_html(response?, &url)) {
				Some(Ok(html)) => parse_manga_list(&html).entries,
				_ => continue,
			};
			if entries.is_empty() {
				continue;
			}
			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: entries.into_iter().map(Link::from).collect(),
					listing: Some(Listing {
						id: String::from(id),
						name: String::from(title),
						..Default::default()
					}),
				},
			}));
		}

		Ok(HomeLayout::default())
	}
}

impl DeepLinkHandler for Wmh1234Source {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some((manga_key, key)) = chapter_path(&url) {
			return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
		}
		if let Some(token) = chapter_token(&url) {
			// The token names its work and chapter, so no request is needed.
			return Ok(token_ids(&token)
				.map(|(manga_key, key)| DeepLinkResult::Chapter { manga_key, key }));
		}
		if let Some(key) = comic_id(&url) {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		Ok(None)
	}
}

/// Version 4 changed chapter keys from the site's `/go/` token to the chapter id, after the
/// site stopped linking tokens (2026-10-24). `breakingChangeVersion` in `source.json` makes
/// the app offer this migration, which keeps reading history. Work keys did not change.
impl MigrationHandler for Wmh1234Source {
	fn handle_manga_migration(&self, key: String) -> Result<String> {
		Ok(key)
	}

	fn handle_chapter_migration(&self, _manga_key: String, chapter_key: String) -> Result<String> {
		if is_id(&chapter_key) {
			return Ok(chapter_key);
		}
		match token_ids(&chapter_key) {
			Some((_, chapter_id)) => Ok(chapter_id),
			// The app keeps the old key, which still opens through the reader.
			None => bail!("[wmh1234] cannot read chapter key {chapter_key}"),
		}
	}
}

register_source!(
	Wmh1234Source,
	ListingProvider,
	Home,
	DeepLinkHandler,
	MigrationHandler
);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		// A Windows checkout may turn the file's line endings into CRLF.
		let json = include_str!("../res/source.json").replace("\r\n", "\n");
		for (title, id) in HOME_ROWS {
			assert!(
				json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{title}\"")),
				"{id}"
			);
			assert!(listing_url(id, 1).is_some(), "{id}");
		}
		assert_eq!(json.matches("\"id\": ").count(), HOME_ROWS.len() + 1);
	}

	#[aidoku_test]
	fn tags_match_filters_json() {
		// A Windows checkout may turn the file's line endings into CRLF.
		let json = include_str!("../res/filters.json").replace("\r\n", "\n");
		// The select lists "全部"/"all" first, then every tag in table order.
		let mut names = String::from("\"全部\"");
		let mut ids = String::from("\"all\"");
		for (id, name) in tags::TAGS {
			names.push_str(&format!(",\n\t\t\t\"{name}\""));
			ids.push_str(&format!(",\n\t\t\t\"{id}\""));
			assert_eq!(tags::tag_id(name), Some(id), "{name}");
			assert_eq!(
				tags::TAGS.iter().filter(|(_, n)| *n == name).count(),
				1,
				"{name}"
			);
		}
		assert!(json.contains(&format!("[\n\t\t\t{names}\n\t\t]")));
		assert!(json.contains(&format!("[\n\t\t\t{ids}\n\t\t]")));
		assert!(json.contains("[\n\t\t\t\"hits\",\n\t\t\t\"addtime\",\n\t\t\t\"id\"\n\t\t]"));
		assert!(json.contains("[\n\t\t\t\"0\",\n\t\t\t\"1\",\n\t\t\t\"2\"\n\t\t]"));
	}

	#[aidoku_test]
	fn links_open_without_a_request() {
		let source = Wmh1234Source::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://m.wmh1234.com/comic/47315.html"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("47315")
			})
		);
		for url in [
			"https://m.wmh1234.com/go/NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh",
			"https://reader.hqread.cc/r/NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh",
		] {
			assert_eq!(
				source.handle_deep_link(String::from(url)).unwrap(),
				Some(DeepLinkResult::Chapter {
					manga_key: String::from("47315"),
					key: String::from("3926106")
				})
			);
		}
		assert_eq!(
			source
				.handle_deep_link(String::from(
					"https://m.wmh1234.com/comic/47315/3954661.html"
				))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("47315"),
				key: String::from("3954661")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://m.wmh1234.com/go/NDczMTU="))
				.unwrap(),
			None
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://m.wmh1234.com/category/tags/17"))
				.unwrap(),
			None
		);
	}

	#[aidoku_test]
	fn migrates_token_keys_to_chapter_ids() {
		let source = Wmh1234Source::new();
		let migrate =
			|key: &str| source.handle_chapter_migration(String::from("47315"), String::from(key));
		assert_eq!(
			migrate("NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh").ok().as_deref(),
			Some("3926106")
		);
		// Unpadded token.
		assert_eq!(
			migrate("MTY4MzgtNjMzNzk3LTdlN2RlMDIyMGI").ok().as_deref(),
			Some("633797")
		);
		assert_eq!(migrate("3954661").ok().as_deref(), Some("3954661"));
		assert!(migrate("not base64!").is_err());
		assert_eq!(
			source
				.handle_manga_migration(String::from("47315"))
				.ok()
				.as_deref(),
			Some("47315")
		);
	}
}
