#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, ImageRequestProvider, Link, Listing, ListingProvider, Manga,
	MangaPageResult, Page, PageContext, Result, Source,
};

mod helper;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 4] = [
	("最新", "latest"),
	("最多觀看", "views"),
	("最高評分", "rating"),
	("最多評論", "comments"),
];

struct Ho5hoSource;

impl Source for Ho5hoSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut search = Search {
			query: query.unwrap_or_default(),
			// `res/filters.json`'s default.
			match_all: true,
			..Default::default()
		};
		for filter in filters {
			match filter {
				FilterValue::Text { id, value } if id == "author" => search.author = Some(value),
				// `filters.json` supplies `ids`, so `value` is already the query value.
				FilterValue::Select { id, value } => match id.as_str() {
					"sort" => search.order = Some(value),
					"match" => search.match_all = value != "any",
					// A tapped tag. Categories can be searched; a tag has only its own page,
					// which takes no other condition.
					"genre" if !value.is_empty() => {
						if CATEGORIES.contains(&value.as_str()) {
							if !search.categories.contains(&value) {
								search.categories.push(value);
							}
						} else {
							return Ok(parse_manga_list(&fetch_html(&tag_url(&value, page))?));
						}
					}
					_ => {}
				},
				FilterValue::MultiSelect { id, included, .. } if id == "category" => {
					for name in included {
						if !search.categories.contains(&name) {
							search.categories.push(name);
						}
					}
				}
				_ => {}
			}
		}
		Ok(parse_manga_list(&fetch_html(&search_url(&search, page))?))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		// Details and chapters are on the same page.
		let html = fetch_html(&manga_url(&manga.key))?;

		if needs_details {
			match parse_details(&html, &manga.key) {
				Some(details) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				None => println!("[ho5ho] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			// One unreadable title must not abort a whole library refresh.
			let chapters = parse_chapters(&html, &manga.key);
			if chapters.is_empty() {
				println!("[ho5ho] ERROR no chapters on the page of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let pages = parse_pages(&fetch_html(&chapter_url(&manga.key, &chapter.key))?);
		if pages.is_empty() {
			bail!("[ho5ho] no images for {}/{}", manga.key, chapter.key);
		}
		Ok(pages)
	}
}

impl ListingProvider for Ho5hoSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some(url) = listing_url(&listing.id, page) else {
			bail!("Unknown listing: {}", listing.id);
		};
		Ok(parse_manga_list(&fetch_html(&url)?))
	}
}

impl Home for Ho5hoSource {
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
			let Some(url) = listing_url(id, 1) else {
				continue;
			};
			// A row that fails stays empty; the others still load.
			let entries = match fetch_html(&url) {
				Ok(html) => parse_manga_list(&html).entries,
				Err(_) => {
					println!("[ho5ho] ERROR loading the {id} row");
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

impl ImageRequestProvider for Ho5hoSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		Ok(Request::get(&url)?.header("Referer", REFERER))
	}
}

impl DeepLinkHandler for Ho5hoSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		Ok(work_keys(&url).map(|(manga_key, chapter)| match chapter {
			Some(key) => DeepLinkResult::Chapter { manga_key, key },
			None => DeepLinkResult::Manga { key: manga_key },
		}))
	}
}

register_source!(
	Ho5hoSource,
	ListingProvider,
	Home,
	ImageRequestProvider,
	DeepLinkHandler
);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		let json = include_str!("../res/source.json");
		for (title, id) in HOME_ROWS {
			assert!(json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{title}\"")), "{id}");
			assert!(listing_url(id, 1).is_some(), "{id}");
		}
	}

	#[aidoku_test]
	fn filters_match_filters_json() {
		let json = include_str!("../res/filters.json");
		for name in CATEGORIES {
			assert!(json.contains(&format!("\t\t\t\"{name}\",")) || json.contains(&format!("\t\t\t\"{name}\"\n")), "{name}");
		}
		for order in ORDERS {
			assert!(json.contains(&format!("\"{order}\"")), "{order}");
		}
		assert!(json.contains("\"all\"") && json.contains("\"any\""));
	}

	#[aidoku_test]
	fn links_open_works_and_chapters() {
		let source = Ho5hoSource::new();
		let work = "%e7%84%a1%e4%ba%ba%e5%b3%b6%e9%81%87%e9%9b%a3%e7%9a%84%e4%b8%89%e4%bd%8d%e8%be%a3%e5%a6%b9%e5%be%8c%e5%ae%ae";
		assert_eq!(
			source
				.handle_deep_link(format!("https://www.ho5ho.com/中字h漫/{work}/"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from(work)
			})
		);
		assert_eq!(
			source
				.handle_deep_link(format!("https://www.ho5ho.com/中字h漫/{work}/server-1/"))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from(work),
				key: String::from("server-1")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.ho5ho.com/?m_orderby=views"))
				.unwrap(),
			None
		);
	}
}
