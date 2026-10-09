#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, ImageRequestProvider, Link, Listing, ListingProvider, Manga,
	MangaPageResult, MangaStatus, Page, PageContent, PageContext, Result, Source,
};

mod crypto;
mod helper;
mod tags;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 5] = [
	("最近更新", "update"),
	("總點擊", "hot"),
	("月點擊", "month"),
	("週點擊", "week"),
	("日點擊", "day"),
];

/// The only image list this source reads. The site's script fetches `source_id` 12
/// lists from another host and decrypts each image; none turned up in 18 sampled works.
const PLAIN_SOURCE_ID: &str = "15";

/// Shown as the description of a work the site has removed.
const DELISTED_NOTE: &str = "站方已下架這部作品。";

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

struct ManwangSource;

impl Source for ManwangSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		if let Some(query) = query.filter(|q: &String| !q.trim().is_empty()) {
			// The search answers one page; its pager links have no page number.
			if page > 1 {
				return Ok(empty_page());
			}
			return Ok(parse_manga_list(&fetch_html(&search_url(&query))?, page));
		}

		let mut tag: Option<String> = None;
		let mut by_update = false;
		for filter in filters {
			if let FilterValue::Select { id, value } = filter {
				match id.as_str() {
					"order" => by_update = value == "addtime",
					// `filters.json` supplies `ids`, so `value` is already the tag id.
					"tag" if value != "all" => tag = Some(value),
					// A tapped tag sends its name.
					"genre" if !value.is_empty() => match tags::tag_id(value.trim()) {
						Some(id) => tag = Some(String::from(id)),
						None => {
							println!("[manwang] unknown tag {value}");
							return Ok(empty_page());
						}
					},
					_ => {}
				}
			}
		}
		Ok(parse_manga_list(
			&fetch_html(&browse_url(tag.as_deref(), by_update, page))?,
			page,
		))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		// Details and chapters are on the same page.
		let Some(html) = fetch_details_page(&manga.key)? else {
			// The boards still list removed works; say so instead of an empty page, and
			// keep any chapters the library already has.
			println!("[manwang] {} has been removed by the site", manga.key);
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
				None => println!("[manwang] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			// An unreadable page must not empty the chapters the library already has.
			let chapters = parse_chapters(&html, &manga.key);
			if chapters.is_empty() {
				println!("[manwang] ERROR no chapters on the page of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let html = fetch_text(&chapter_url(&manga.key, &chapter.key))?;
		let Some(list) = crypto::image_list(&html) else {
			bail!("[manwang] no image list for chapter {}", chapter.key);
		};
		if list.source_id != PLAIN_SOURCE_ID {
			println!(
				"[manwang] ERROR chapter {} uses source_id {}",
				chapter.key, list.source_id
			);
			bail!("[manwang] unsupported source_id {}", list.source_id);
		}
		if list.images.is_empty() {
			bail!("[manwang] no images for chapter {}", chapter.key);
		}
		Ok(list
			.images
			.into_iter()
			.map(|url: String| Page {
				content: PageContent::url(url),
				..Default::default()
			})
			.collect())
	}
}

impl ListingProvider for ManwangSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some(url) = board_url(&listing.id) else {
			bail!("Unknown listing: {}", listing.id);
		};
		// Every board is a single page.
		if page > 1 {
			return Ok(empty_page());
		}
		Ok(parse_manga_list(&fetch_html(&url)?, page))
	}
}

impl Home for ManwangSource {
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
			let entries = match board_url(id).map(|url: String| fetch_html(&url)) {
				Some(Ok(html)) => parse_manga_list(&html, 1).entries,
				_ => {
					println!("[manwang] ERROR loading the {id} row");
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

impl ImageRequestProvider for ManwangSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		Ok(Request::get(&url)?.header("Referer", REFERER))
	}
}

impl DeepLinkHandler for ManwangSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some((manga_key, key)) = chapter_keys(&url) {
			return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
		}
		if let Some(key) = id_after(&url, "book") {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		Ok(None)
	}
}

register_source!(
	ManwangSource,
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
			assert!(board_url(id).is_some(), "{id}");
		}
	}

	#[aidoku_test]
	fn tags_match_filters_json() {
		let json = include_str!("../res/filters.json");
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
		assert!(json.contains("\"hits\"") && json.contains("\"addtime\""));
	}

	#[aidoku_test]
	fn links_open_without_a_request() {
		let source = ManwangSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.manwang.net/book/503735"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("503735")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.manwang.net/chapter/503735-185808"))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("503735"),
				key: String::from("185808")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.manwang.net/category/tags/2571"))
				.unwrap(),
			None
		);
	}
}
