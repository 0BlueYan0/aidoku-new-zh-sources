#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::std::send_partial_result,
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga, MangaPageResult, Page,
	PageContent, Result, Source,
};

mod decoder;
mod helper;

use helper::*;

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

/// A page of `/data/sort` or `/data/rank`. The site answers -1 past page 5.
fn api_page(path: &str, number: &str, page: i32) -> Result<MangaPageResult> {
	if page > MAX_PAGES {
		return Ok(empty_page());
	}
	Ok(match post_list(path, &sort_form(number, page))? {
		Some(data) => page_result(&data, page),
		None => empty_page(),
	})
}

struct YumanhuaSource;

impl Source for YumanhuaSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut category: Option<String> = None;
		let mut unknown_tag = false;
		for filter in filters {
			if let FilterValue::Select { id, value } = filter {
				if id != "genre" || value.is_empty() {
					continue;
				}
				// The select sends its `ids`; a tapped tag sends the site's name instead.
				if value.bytes().all(|b: u8| b.is_ascii_digit()) {
					category = Some(value);
				} else if let Some(id) = category_id(value.trim()) {
					category = Some(String::from(id));
				} else {
					println!("[yumanhua] unknown tag {value}");
					unknown_tag = true;
				}
			}
		}

		if let Some(query) = query.filter(|q: &String| !q.trim().is_empty()) {
			// One page of results, matched on titles only.
			if page > 1 {
				return Ok(empty_page());
			}
			let entries = post_list("/s", &search_form(&query))?
				.map(|data: String| parse_json_list(&data))
				.unwrap_or_default();
			return Ok(MangaPageResult {
				entries,
				has_next_page: false,
			});
		}
		if unknown_tag {
			return Ok(empty_page());
		}
		match category {
			Some(category) => api_page("/data/sort", &category, page),
			// No category: the first ranking, which pages like a category.
			None => api_page("/data/rank", "1", page),
		}
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		// Details and the first 20 chapters are on the same page.
		let html = fetch_html(&manga_url(&manga.key))?;

		if needs_details {
			match parse_details(&html, &manga.key) {
				Some(details) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				None => println!("[yumanhua] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			let mut chapters = parse_page_chapters(&html, &manga.key);
			let mut complete = true;
			if has_more_chapters(&html) {
				match post_list("/morechapter", &format!("id={}", manga.key)) {
					Ok(Some(data)) => {
						let before = chapters.len();
						append_more_chapters(&mut chapters, &data, &manga.key);
						// An answer that adds nothing is as incomplete as -1.
						complete = chapters.len() > before;
					}
					// Works with 20 chapters or fewer have no button (2026-10-13), so -1 here
					// means the rest failed to load, not that there is no rest.
					Ok(None) => complete = false,
					Err(error) => {
						println!("[yumanhua] ERROR more chapters of {}: {error:?}", manga.key);
						complete = false;
					}
				}
			}
			// A partial or empty list would make the app drop the library's chapters.
			if !complete || chapters.is_empty() {
				println!("[yumanhua] keeping the stored chapters of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let html = fetch_html(&chapter_url(&manga.key, &chapter.key))?;
		let (scripts, key_index) = reader_parts(&html);
		let Some(pages) = key_index.and_then(|i: usize| decoder::decode_pages(&scripts, i)) else {
			bail!(
				"[yumanhua] no images for {}/{} (data-id {key_index:?}, {} scripts)",
				manga.key,
				chapter.key,
				scripts.len()
			);
		};
		Ok(pages
			.into_iter()
			.map(|url: String| Page {
				content: PageContent::url(url),
				..Default::default()
			})
			.collect())
	}
}

impl ListingProvider for YumanhuaSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some((_, number)) = LISTINGS
			.iter()
			.find(|(id, _): &&(&str, &str)| *id == listing.id)
		else {
			bail!("Unknown listing: {}", listing.id);
		};
		if HTML_RANKS.contains(number) {
			// One page of 50 works.
			if page > 1 {
				return Ok(empty_page());
			}
			let entries = parse_rank_html(&fetch_html(&format!("{BASE_URL}/rank/{number}"))?);
			return Ok(MangaPageResult {
				entries,
				has_next_page: false,
			});
		}
		api_page("/data/rank", number, page)
	}
}

impl Home for YumanhuaSource {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately.
		let components: Vec<HomeComponent> = HOME_TITLES
			.iter()
			.map(|(_, title): &(&str, &str)| HomeComponent {
				title: Some(String::from(*title)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			})
			.collect();
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		// Every section is on the one home page. The site's sections have no paged list
		// behind them, so none links to a listing.
		for (title, entries) in parse_home(&fetch_html(BASE_URL)?) {
			if entries.is_empty() {
				continue;
			}
			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: Some(title),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: entries.into_iter().map(Link::from).collect(),
					listing: None,
				},
			}));
		}

		Ok(HomeLayout::default())
	}
}

impl DeepLinkHandler for YumanhuaSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some((manga_key, key)) = chapter_keys(&url) {
			return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
		}
		if let Some(key) = manga_key(&url) {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		Ok(None)
	}
}

register_source!(YumanhuaSource, ListingProvider, Home, DeepLinkHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listings_match_source_json() {
		// A Windows checkout may turn the file's line endings into CRLF.
		let json = include_str!("../res/source.json").replace("\r\n", "\n");
		for (id, _) in LISTINGS {
			assert!(json.contains(&format!("\"id\": \"{id}\",")), "{id}");
		}
		assert_eq!(json.matches("\"id\": ").count(), LISTINGS.len() + 1);
	}

	#[aidoku_test]
	fn categories_match_filters_json() {
		let json = include_str!("../res/filters.json").replace("\r\n", "\n");
		// The select lists "全部" first, then every category in table order.
		let mut names = String::from("\"全部\"");
		let mut ids = String::from("\"\"");
		for (name, id) in CATEGORIES {
			names.push_str(&format!(",\n\t\t\t\"{name}\""));
			ids.push_str(&format!(",\n\t\t\t\"{id}\""));
			assert_eq!(category_id(name), Some(id), "{name}");
		}
		assert!(json.contains(&format!("[\n\t\t\t{names}\n\t\t]")));
		assert!(json.contains(&format!("[\n\t\t\t{ids}\n\t\t]")));
		assert!(json.contains("\"id\": \"genre\""));
		assert!(json.contains("\"isGenre\": true"));
	}

	#[aidoku_test]
	fn links_open_without_a_request() {
		let source = YumanhuaSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("http://yumanhua.com/trsKtGi/"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("trsKtGi")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("http://www.yumanhua.com/trsKtGi/IVpdIdWF.html"))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("trsKtGi"),
				key: String::from("IVpdIdWF")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("http://yumanhua.com/rank/1"))
				.unwrap(),
			None
		);
	}
}
