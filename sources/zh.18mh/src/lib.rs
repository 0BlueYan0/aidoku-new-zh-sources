#![no_std]

use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{html::Element, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HashMap, HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga, MangaPageResult,
	Page, Result, Source, WebLoginHandler,
};

mod helper;

use helper::*;

/// The home page's rows in its own order: heading and the listing behind it. The
/// headings are the site's; the streamed components are matched to the skeleton by title.
/// `近期更新` has no page of its own. The other three follow it as `.hometitle` +
/// `.cardlist` pairs in this order, and their names must match `res/source.json`.
const RECENT_TITLE: &str = "近期更新";
const HOME_ROWS: [(&str, &str); 3] = [
	("熱門更新", "dayup"),
	("人氣排行", "hots"),
	("最新上架", "newss"),
];

struct Mh18Source;

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

impl Source for Mh18Source {
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
			let url = format!(
				"{BASE_URL}/s/{}?page={page}",
				encode_uri_component(query.trim())
			);
			return Ok(parse_manga_list(fetch_html(&url)?));
		}

		let mut path = "manga";
		for filter in filters {
			match filter {
				FilterValue::Text { id, value } if id == "author" => {
					// Only authors of works opened in the app are known by name.
					return match author_slug(&value) {
						Some(slug) => Ok(parse_manga_list(fetch_html(&list_url(
							&format!("manga-author/{slug}"),
							page,
						))?)),
						None => Ok(empty_page()),
					};
				}
				FilterValue::Select { id, value } if id == "genre" && !value.is_empty() => {
					// Paths can't be combined, so there is only the one select. A name that is
					// not on it gets no results rather than some other list.
					match category_path(&value) {
						Some(p) => path = p,
						None => return Ok(empty_page()),
					}
				}
				_ => {}
			}
		}
		Ok(parse_manga_list(fetch_html(&list_url(path, page))?))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let mut mid = if needs_details {
			None
		} else {
			cached_mid(&manga.key)
		};

		if needs_details || (needs_chapters && mid.is_none()) {
			let html = fetch_html(&manga_url(&manga.key))?;
			let (details, found) = parse_details(&html, &manga.key);
			if let Some(found) = found.as_deref() {
				remember_mid(&manga.key, found);
			}
			mid = found;
			if needs_details {
				let chapters = manga.chapters.take();
				manga = details;
				manga.chapters = chapters;
				send_partial_result(&manga);
			}
		}

		if needs_chapters {
			// One unreadable title must not abort a whole library refresh.
			match mid {
				Some(mid) => match fetch_html(&chapters_url(&mid)) {
					Ok(html) => manga.chapters = Some(parse_chapters(&html, &mid)),
					Err(_) => println!("[18mh] ERROR fetching chapters for {}", manga.key),
				},
				None => println!("[18mh] ERROR no work id on the page of {}", manga.key),
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let Some((mid, cs)) = chapter.key.split_once('/') else {
			bail!("[18mh] bad chapter key {}", chapter.key);
		};
		let pages = parse_pages(&fetch_html(&content_url(mid, cs))?);
		if pages.is_empty() {
			bail!("[18mh] no images for {}", chapter.key);
		}
		Ok(pages)
	}
}

impl ListingProvider for Mh18Source {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		match listing.id.as_str() {
			"dayup" | "hots" | "newss" => {
				Ok(parse_manga_list(fetch_html(&list_url(&listing.id, page))?))
			}
			_ => bail!("Unknown listing: {}", listing.id),
		}
	}
}

impl Home for Mh18Source {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately.
		let mut components: Vec<HomeComponent> = Vec::new();
		for title in core::iter::once(RECENT_TITLE).chain(HOME_ROWS.iter().map(|(t, _)| *t)) {
			components.push(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			});
		}
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		// One request carries every row.
		let root: Element = fetch_html(BASE_URL)?.into();

		let send = |title: &str, entries: Vec<Manga>, listing: Option<&str>| {
			if entries.is_empty() {
				return;
			}
			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: entries.into_iter().map(Link::from).collect(),
					listing: listing.map(|id: &str| Listing {
						id: String::from(id),
						name: String::from(title),
						..Default::default()
					}),
				},
			}));
		};

		send(RECENT_TITLE, parse_cards(&root, ".pb-unit-md a.slicarda"), None);

		if let Some(lists) = root.select(".cardlist") {
			for ((title, id), list) in HOME_ROWS.iter().zip(lists) {
				send(title, parse_cards(&list, ".pb-2 a"), Some(id));
			}
		}

		Ok(HomeLayout::default())
	}
}

impl DeepLinkHandler for Mh18Source {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let path = url
			.split_once("://")
			.map_or(url.as_str(), |(_, rest): (&str, &str)| rest);
		let Some((_, rest)) = path.split_once("/manga/") else {
			return Ok(None);
		};
		let Some(slug) = slug_from_href(&format!("/manga/{rest}")) else {
			return Ok(None);
		};

		// `/manga/<slug>/<mid>-<n>-<index>` does not carry the chapter id `getcontent`
		// needs; only the reader page does.
		let is_chapter = rest
			.split(['?', '#'])
			.next()
			.unwrap_or(rest)
			.trim_end_matches('/')
			.contains('/');
		if is_chapter {
			let reader = format!("{BASE_URL}/manga/{}", rest.split(['?', '#']).next().unwrap_or(rest));
			if let Some(key) = reader_chapter_key(&fetch_html(&reader)?) {
				return Ok(Some(DeepLinkResult::Chapter {
					manga_key: slug,
					key,
				}));
			}
		}
		Ok(Some(DeepLinkResult::Manga { key: slug }))
	}
}

impl WebLoginHandler for Mh18Source {
	/// The settings page closes the site and marks the button done once this is true, so
	/// it waits for the clearance cookie the check leaves behind.
	fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		if key != CLOUDFLARE_KEY {
			bail!("Invalid login key: {key}");
		}
		Ok(cookies
			.get(CLEARANCE_COOKIE)
			.is_some_and(|value: &String| !value.is_empty()))
	}
}

register_source!(Mh18Source, ListingProvider, Home, DeepLinkHandler, WebLoginHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn home_rows_follow_the_page() {
		let root: Element = Html::parse_with_url(include_str!("fixtures/home.html"), BASE_URL)
			.expect("parse")
			.into();
		assert_eq!(parse_cards(&root, ".pb-unit-md a.slicarda").len(), 30);
		let lists: Vec<Element> = root.select(".cardlist").expect("cardlists").collect();
		assert_eq!(lists.len(), HOME_ROWS.len());
		let titles: Vec<String> = root
			.select(".hometitle h2")
			.expect("titles")
			.map(|el: Element| el.text().unwrap_or_default())
			.collect();
		for ((title, _), found) in HOME_ROWS.iter().zip(titles.iter()) {
			assert_eq!(title, found);
		}
		for list in lists {
			assert_eq!(parse_cards(&list, ".pb-2 a").len(), 12);
		}
	}

	#[aidoku_test]
	fn listing_names_match_source_json() {
		let json = include_str!("../res/source.json");
		for (title, id) in HOME_ROWS {
			assert!(json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{title}\"")), "{id}");
		}
	}

	#[aidoku_test]
	fn the_check_is_done_once_the_clearance_cookie_appears() {
		let source = Mh18Source::new();
		let mut cookies: HashMap<String, String> = HashMap::new();
		assert!(!source.handle_web_login(String::from(CLOUDFLARE_KEY), cookies.clone()).unwrap());
		cookies.insert(String::from("__cf_bm"), String::from("x"));
		assert!(!source.handle_web_login(String::from(CLOUDFLARE_KEY), cookies.clone()).unwrap());
		cookies.insert(String::from(CLEARANCE_COOKIE), String::from("abc"));
		assert!(source.handle_web_login(String::from(CLOUDFLARE_KEY), cookies).unwrap());
	}

	#[aidoku_test]
	fn work_links_open_the_work() {
		assert_eq!(
			Mh18Source::new()
				.handle_deep_link(String::from("https://18mh.org/manga/pengyoudejiejie"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("pengyoudejiejie")
			})
		);
		assert_eq!(
			Mh18Source::new()
				.handle_deep_link(String::from("https://18mh.org/hots/page/2"))
				.unwrap(),
			None
		);
	}
}
