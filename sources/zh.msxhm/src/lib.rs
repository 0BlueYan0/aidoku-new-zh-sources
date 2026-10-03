#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::std::send_partial_result,
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga, MangaPageResult, Page,
	Result, Source,
};

mod helper;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 5] = [
	("最近更新", "update"),
	("人氣榜", "rank_hot"),
	("新書榜", "rank_new"),
	("完結榜", "rank_end"),
	("推薦榜", "rank_rec"),
];

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

fn rank_heading(id: &str) -> Option<&'static str> {
	RANKS.iter().find(|(rank, _)| *rank == id).map(|(_, heading)| *heading)
}

struct MsxhmSource;

impl Source for MsxhmSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let base = base_url();
		let mut browse = Browse::default();
		let mut author = None;
		for filter in filters {
			match filter {
				// The site's search also matches author names.
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				// `filters.json` supplies `ids`, so `value` is already the query value.
				FilterValue::Select { id, value } => match id.as_str() {
					"tag" if value != "全部" => browse.tag = Some(value),
					// A tapped tag: the site's own name for it, any of which the catalogue takes.
					"genre" if !value.is_empty() => browse.tag = Some(value),
					"area" => browse.area = value,
					"end" => browse.end = value,
					_ => {}
				},
				_ => {}
			}
		}
		let query = query
			.filter(|q: &String| !q.trim().is_empty())
			.or(author)
			.filter(|q: &String| !q.trim().is_empty());
		match query {
			// The search has one page of 20 and ignores `page`; a second page would repeat it.
			Some(_) if page > 1 => Ok(empty_page()),
			Some(query) => Ok(parse_manga_list(&fetch_html(&search_url(&base, &query))?, &base)),
			None => Ok(parse_manga_list(&fetch_html(&browse_url(&base, &browse, page))?, &base)),
		}
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let base = base_url();
		// Details and chapters are on the same page.
		let html = fetch_html(&manga_url(&base, &manga.key))?;

		if needs_details {
			match parse_details(&html, &manga.key, &base) {
				Some(details) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				None => println!("[msxhm] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			// One unreadable title must not abort a whole library refresh.
			let chapters = parse_chapters(&html, &base);
			if chapters.is_empty() {
				println!("[msxhm] ERROR no chapters on the page of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let base = base_url();
		let pages = parse_pages(&fetch_html(&chapter_url(&base, &chapter.key))?, &base);
		if pages.is_empty() {
			bail!("[msxhm] no images for chapter {}", chapter.key);
		}
		Ok(pages)
	}
}

impl ListingProvider for MsxhmSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let base = base_url();
		let Some(url) = listing_url(&base, &listing.id, page) else {
			// Past the rank boards' single page.
			if rank_heading(&listing.id).is_some() {
				return Ok(empty_page());
			}
			bail!("Unknown listing: {}", listing.id);
		};
		let html = fetch_html(&url)?;
		Ok(match rank_heading(&listing.id) {
			Some(heading) => parse_rank(&html, heading, &base),
			None => parse_manga_list(&html, &base),
		})
	}
}

impl Home for MsxhmSource {
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

		let base = base_url();
		// The four boards share one page, so it is fetched once.
		let rank = listing_url(&base, "rank_hot", 1).and_then(|url: String| fetch_html(&url).ok());
		for (title, id) in HOME_ROWS {
			// A row that fails stays empty; the others still load.
			let entries = match rank_heading(id) {
				Some(heading) => match &rank {
					Some(html) => parse_rank(html, heading, &base).entries,
					None => continue,
				},
				None => match listing_url(&base, id, 1).map(|url: String| fetch_html(&url)) {
					Some(Ok(html)) => parse_manga_list(&html, &base).entries,
					_ => {
						println!("[msxhm] ERROR loading the {id} row");
						continue;
					}
				},
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

impl DeepLinkHandler for MsxhmSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(key) = id_after(&url, "book") {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		// A chapter link does not name its work; the reader page links back to it.
		if let Some(key) = id_after(&url, "chapter") {
			let html = fetch_html(&chapter_url(&base_url(), &key))?;
			if let Some(manga_key) = reader_manga_key(&html) {
				return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
			}
		}
		Ok(None)
	}
}

register_source!(MsxhmSource, ListingProvider, Home, DeepLinkHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		let json = include_str!("../res/source.json");
		for (title, id) in HOME_ROWS {
			assert!(json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{title}\"")), "{id}");
			assert!(listing_url(BASE_URL, id, 1).is_some(), "{id}");
		}
		for id in ["latest", "ongoing", "completed"] {
			assert!(json.contains(&format!("\"id\": \"{id}\"")), "{id}");
			assert!(listing_url(BASE_URL, id, 1).is_some(), "{id}");
		}
	}

	#[aidoku_test]
	fn filter_ids_are_site_values() {
		let json = include_str!("../res/filters.json");
		for id in ["\"end\"", "\"tag\"", "\"area\"", "\"长腿\"", "\"都市\"", "\"-1\""] {
			assert!(json.contains(id), "{id}");
		}
	}

	#[aidoku_test]
	fn book_links_open_without_a_request() {
		let source = MsxhmSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.mxs13.cc/book/1224"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("1224")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.jjmhw9.top/booklist?tag=都市"))
				.unwrap(),
			None
		);
	}
}
