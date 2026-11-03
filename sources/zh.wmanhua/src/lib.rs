#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga, MangaPageResult,
	MangaStatus, Page, Result, Source,
};

mod helper;
mod tags;

use helper::*;

/// The home screen's rows, in order. The first four come from the front page, the last
/// three from `/hot`; the streamed components are matched to the skeleton by title.
const FEATURED: &str = "今日推薦";
const POPULAR: &str = "熱門漫畫";
const RANKING: &str = "本週排行";
const LATEST: &str = "最近更新";
/// `/hot` tab panels and their titles.
const HOT_BOARDS: [(&str, &str); 3] = [("人氣榜", "hot"), ("新作榜", "new"), ("收藏榜", "fav")];

/// Shown as the description of a work the site no longer has.
const DELISTED_NOTE: &str = "站方已下架這部作品。";

fn component(title: &str, value: HomeComponentValue) -> HomeComponent {
	HomeComponent {
		title: Some(String::from(title)),
		subtitle: None,
		value,
	}
}

fn links(entries: Vec<Manga>) -> Vec<Link> {
	entries.into_iter().map(Link::from).collect()
}

/// Sends one row unless it is empty; an empty row keeps its skeleton.
fn send_row(title: &str, entries: Vec<Manga>, make: fn(Vec<Manga>) -> HomeComponentValue) {
	if !entries.is_empty() {
		send_partial_result(&HomePartialResult::Component(component(
			title,
			make(entries),
		)));
	}
}

struct WmanhuaSource;

impl Source for WmanhuaSource {
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
		let mut category: Option<String> = None;
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
					// only sends a select whose value is not the default ("all", "desc").
					"order" => order = Some(value),
					"status" => status = Some(value),
					"category" => category = Some(value),
					"tag" => tag = Some(value),
					// A tapped tag sends its name.
					"genre" => match tags::tag_id(value.trim()) {
						Some(id) => tag = Some(String::from(id)),
						None => unknown_tag = true,
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
			return fetch_list(&search_url(&query, page), page);
		}
		if unknown_tag {
			return Ok(empty_page());
		}
		fetch_list(
			&browse_url(
				category.as_deref(),
				tag.as_deref(),
				status.as_deref(),
				order.as_deref(),
				page,
			),
			page,
		)
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		if needs_details {
			let Some(html) = fetch_details_page(&manga.key)? else {
				// Keep any chapters the library already has.
				manga.status = MangaStatus::Cancelled;
				manga.description = Some(String::from(DELISTED_NOTE));
				send_partial_result(&manga);
				return Ok(manga);
			};
			if let Some(details) = parse_details(&html, &manga.key) {
				let chapters = manga.chapters.take();
				manga = details;
				manga.chapters = chapters;
				send_partial_result(&manga);
			}
		}

		if needs_chapters {
			// The details page shows only the newest 12, so every chapter comes from the
			// chapter list. An empty list leaves the library's chapters as they are; a
			// failed one is an error, which does the same.
			if let Some(chapters) = fetch_chapters(&manga.key)? {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let url = chapter_url(&manga.key, &chapter.key);
		let response = request(&url)?.send()?;
		let status = response.status_code();
		if !(200..300).contains(&status) {
			bail!("[wmanhua] HTTP {status} for {url}");
		}
		parse_pages(&response.get_string()?)
	}
}

impl ListingProvider for WmanhuaSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some(url) = listing_url(&listing.id, page) else {
			bail!("Unknown listing: {}", listing.id);
		};
		fetch_list(&url, page)
	}
}

impl Home for WmanhuaSource {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately.
		let mut components = Vec::from([
			component(FEATURED, HomeComponentValue::empty_big_scroller()),
			component(POPULAR, HomeComponentValue::empty_scroller()),
			component(RANKING, HomeComponentValue::empty_manga_list()),
			component(LATEST, HomeComponentValue::empty_scroller()),
		]);
		for (title, _) in HOT_BOARDS {
			components.push(component(title, HomeComponentValue::empty_scroller()));
		}
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		// The front page and `/hot` go out together so the home screen, and the covers
		// queued behind this call, wait for one round trip instead of two.
		let front_url = format!("{BASE_URL}/");
		let hot_url = format!("{BASE_URL}/hot");
		let mut requests: Vec<Request> = Vec::new();
		let mut urls: Vec<&str> = Vec::new();
		for url in [front_url.as_str(), hot_url.as_str()] {
			// A page whose request cannot be built is left out; its rows stay empty.
			if let Ok(request) = request(url) {
				requests.push(request);
				urls.push(url);
			}
		}
		let mut responses = Request::send_all(requests).into_iter();

		// Both pages are parsed before any row is sent: 本週排行 takes its missing covers
		// from the `/hot` boards.
		let mut front: Option<HomeRows> = None;
		let mut hot: Option<Vec<(&str, Vec<Manga>)>> = None;
		for url in urls {
			// A page that fails leaves its rows empty; the other page still loads.
			// `read_html` rejects non-2xx answers, so an error page is not read as empty rows.
			let Some(Ok(html)) = responses.next().map(|response| read_html(response?, url)) else {
				continue;
			};
			if url == front_url {
				front = Some(parse_home(&html));
			} else {
				hot = Some(
					HOT_BOARDS
						.iter()
						.map(|(title, panel)| (*title, parse_hot(&html, panel)))
						.collect(),
				);
			}
		}

		if let Some(mut rows) = front {
			if let Some(boards) = &hot {
				let sources: Vec<&[Manga]> = boards
					.iter()
					.map(|(_, entries)| entries.as_slice())
					.collect();
				fill_covers(&mut rows.ranking, &sources);
			}
			send_row(FEATURED, rows.featured, |entries| {
				HomeComponentValue::BigScroller {
					entries,
					auto_scroll_interval: Some(5.0),
				}
			});
			send_row(POPULAR, rows.popular, |entries| {
				HomeComponentValue::Scroller {
					entries: links(entries),
					listing: None,
				}
			});
			send_row(RANKING, rows.ranking, |entries| {
				HomeComponentValue::MangaList {
					ranking: true,
					page_size: None,
					entries: links(entries),
					listing: None,
				}
			});
			send_row(LATEST, rows.latest, |entries| {
				HomeComponentValue::Scroller {
					entries: links(entries),
					listing: Some(Listing {
						id: String::from(UPDATE_LISTING),
						name: String::from(LATEST),
						..Default::default()
					}),
				}
			});
		}
		for (title, entries) in hot.into_iter().flatten() {
			send_row(title, entries, |entries| HomeComponentValue::Scroller {
				entries: links(entries),
				listing: None,
			});
		}

		Ok(HomeLayout::default())
	}
}

impl DeepLinkHandler for WmanhuaSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some((manga_key, key)) = chapter_path(&url) {
			return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
		}
		if let Some(key) = comic_id(&url) {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}
		Ok(None)
	}
}

register_source!(WmanhuaSource, ListingProvider, Home, DeepLinkHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		// A Windows checkout may turn the file's line endings into CRLF.
		let json = include_str!("../res/source.json").replace("\r\n", "\n");
		assert!(json.contains(&format!(
			"\"id\": \"{UPDATE_LISTING}\",\n\t\t\t\"name\": \"{LATEST}\""
		)));
		assert_eq!(json.matches("\"id\": ").count(), 2);
		assert!(listing_url(UPDATE_LISTING, 1).is_some());
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
		}
		assert!(json.contains(&format!("[\n\t\t\t{names}\n\t\t]")));
		assert!(json.contains(&format!("[\n\t\t\t{ids}\n\t\t]")));
		assert!(json.contains("[\n\t\t\t\"desc\",\n\t\t\t\"asc\"\n\t\t]"));
		assert!(json.contains("[\n\t\t\t\"all\",\n\t\t\t\"1\",\n\t\t\t\"2\"\n\t\t]"));
		assert!(json.contains(
			"[\n\t\t\t\"all\",\n\t\t\t\"1\",\n\t\t\t\"2\",\n\t\t\t\"3\",\n\t\t\t\"4\"\n\t\t]"
		));
		assert_eq!(tags::tag_id("穿越"), Some("23"));
	}

	#[aidoku_test]
	fn links_open_without_a_request() {
		let source = WmanhuaSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.wmanhua.com/comic/5342.html"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("5342")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from(
					"https://www.wmanhua.com/chapter/5342-1035448.html"
				))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("5342"),
				key: String::from("1035448")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.wmanhua.com/sort?tag=15"))
				.unwrap(),
			None
		);
	}
}
