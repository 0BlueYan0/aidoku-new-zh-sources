#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::std::send_partial_result,
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga, MangaPageResult,
	MangaStatus, Page, PageContent, Result, Source,
};

mod helper;
mod tags;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 3] = [("最近更新", "update"), ("人氣排行", "hits"), ("最新上架", "new")];

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
					// `filters.json` supplies `ids`, so `value` is the site's value.
					"order" => order = Some(value),
					"status" if value != "0" => status = Some(value),
					"tag" if value != "all" => tag = Some(value),
					// A tapped tag sends its name.
					"genre" if !value.is_empty() => match tags::tag_id(value.trim()) {
						Some(id) => tag = Some(String::from(id)),
						None => {
							println!("[wmh1234] unknown tag {value}");
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
			println!("[wmh1234] {} has been removed by the site", manga.key);
			if needs_details {
				manga.status = MangaStatus::Cancelled;
				manga.description = Some(String::from(DELISTED_NOTE));
				send_partial_result(&manga);
			}
			return Ok(manga);
		};

		if needs_details {
			match parse_details(&html, &manga.key) {
				Some(details) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				None => println!("[wmh1234] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			// Some works list no chapters at all; an empty list must not empty the
			// chapters the library already has.
			let chapters = parse_chapters(&html);
			if chapters.is_empty() {
				println!("[wmh1234] no chapters on the page of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		// A chapter the site is still copying answers 503 "内容准备中"; `fetch_html` fails on it.
		let pages = parse_pages(&fetch_html(&reader_url(&chapter.key))?);
		if pages.is_empty() {
			bail!("[wmh1234] no images for chapter {}", chapter.key);
		}
		Ok(pages
			.into_iter()
			.map(|url: String| Page {
				content: PageContent::url(url),
				..Default::default()
			})
			.collect())
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

		for (title, id) in HOME_ROWS {
			// A row that fails stays empty; the others still load.
			let entries = match listing_url(id, 1).map(|url: String| fetch_html(&url)) {
				Some(Ok(html)) => parse_manga_list(&html).entries,
				_ => {
					println!("[wmh1234] ERROR loading the {id} row");
					continue;
				}
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
		if let Some(key) = chapter_token(&url) {
			// The token names its work, so no request is needed.
			if let Some(manga_key) = token_comic_id(&key) {
				return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
			}
			return Ok(None);
		}
		if let Some(key) = comic_id(&url) {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		Ok(None)
	}
}

register_source!(Wmh1234Source, ListingProvider, Home, DeepLinkHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		// A Windows checkout may turn the file's line endings into CRLF.
		let json = include_str!("../res/source.json").replace("\r\n", "\n");
		for (title, id) in HOME_ROWS {
			assert!(json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{title}\"")), "{id}");
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
			assert_eq!(tags::TAGS.iter().filter(|(_, n)| *n == name).count(), 1, "{name}");
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
					key: String::from("NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh")
				})
			);
		}
		assert_eq!(
			source
				.handle_deep_link(String::from("https://m.wmh1234.com/category/tags/17"))
				.unwrap(),
			None
		);
	}
}
