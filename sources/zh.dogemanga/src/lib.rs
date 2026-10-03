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
const HOME_ROWS: [(&str, &str); 2] = [("熱門排行", "hot"), ("最新連載", "latest")];

struct DogemangaSource;

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

/// One page of a listing. Page 1 has a fixed address; later pages use the cursor kept
/// from the page before.
fn listing_page(listing: &str, page: i32) -> Result<MangaPageResult> {
	let url = if page <= 1 {
		listing_url(listing)
	} else {
		next_page_url(listing, page)
	};
	let Some(url) = url else {
		println!("[dogemanga] no address for {listing} page {page}");
		return Ok(empty_page());
	};
	let (result, next) = parse_manga_list(fetch_html(&url)?);
	if let Some(next) = next {
		remember_next_page(listing, page.max(1) + 1, &next);
	}
	Ok(result)
}

impl Source for DogemangaSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		// The site's own author links are plain searches for the name.
		let author = filters.into_iter().find_map(|filter| match filter {
			FilterValue::Text { id, value } if id == "author" => Some(value),
			_ => None,
		});
		let query = query
			.filter(|q: &String| !q.trim().is_empty())
			.or(author)
			.filter(|q: &String| !q.trim().is_empty());
		match query {
			Some(query) => Ok(parse_manga_list(fetch_html(&search_url(&query, page))?).0),
			None => listing_page("hot", page),
		}
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
			match parse_details(&html) {
				Some(details) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				None => println!("[dogemanga] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			// One unreadable title must not abort a whole library refresh.
			let chapters = parse_chapters(&html);
			if chapters.is_empty() {
				println!("[dogemanga] ERROR no chapters on the page of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let pages = parse_pages(&fetch_html(&reader_url(&chapter.key))?);
		if pages.is_empty() {
			bail!("[dogemanga] no images for {}", chapter.key);
		}
		Ok(pages)
	}
}

impl ListingProvider for DogemangaSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		if listing_url(&listing.id).is_none() {
			bail!("Unknown listing: {}", listing.id);
		}
		listing_page(&listing.id, page)
	}
}

impl Home for DogemangaSource {
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
			// A row that fails stays empty; the other still loads.
			let entries = match listing_page(id, 1) {
				Ok(result) => result.entries,
				Err(_) => {
					println!("[dogemanga] ERROR loading the {id} row");
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

impl ImageRequestProvider for DogemangaSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		Ok(Request::get(&url)?.header("Referer", REFERER))
	}
}

impl DeepLinkHandler for DogemangaSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(key) = id_after(&url, "/m/") {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		// A reader link can point at any page; the page names its work and first page.
		if let Some(id) = id_after(&url, "/p/") {
			if let Some((manga_key, key)) = reader_keys(&fetch_html(&reader_url(&id))?) {
				return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
			}
		}
		Ok(None)
	}
}

register_source!(
	DogemangaSource,
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
			assert!(listing_url(id).is_some(), "{id}");
		}
	}

	#[aidoku_test]
	fn work_links_open_the_work() {
		assert_eq!(
			DogemangaSource::new()
				.handle_deep_link(String::from("https://dogemanga.com/m/zw7V_YJF"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("zw7V_YJF")
			})
		);
		assert_eq!(
			DogemangaSource::new()
				.handle_deep_link(String::from("https://dogemanga.com/?s=1"))
				.unwrap(),
			None
		);
	}
}
