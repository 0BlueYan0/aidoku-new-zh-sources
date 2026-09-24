#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::std::send_partial_result,
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, LinkValue, Listing, ListingProvider, Manga, MangaPageResult, Page,
	PageContent, Result, Source,
};

mod decoder;
mod genres;
mod helper;

use helper::*;

/// `/v1/search` pages an author search walks at most. The search is a fuzzy title match,
/// so the author's works tend to sit in the first pages and the rest is noise.
const AUTHOR_SEARCH_MAX_PAGES: i32 = 5;
const AUTHOR_SEARCH_PAGE_SIZE: i32 = 100;

/// A listing id and its name.
type ListingRef = (&'static str, &'static str);

/// `/v1/home` rows in the site's order: (response field, heading, listing id and name).
/// Headings are the site's own; the streamed components are matched to the skeleton by
/// title, so they must be unique. Listing names must match `res/source.json`.
const HOME_ROWS: [(&str, &str, Option<ListingRef>); 7] = [
	("featured", "精選", None),
	("recent_updates", "近期更新", Some(("update", "近期更新"))),
	("weekly_hot", "本週熱門", Some(("weekly", "本週熱門"))),
	("popularity_ranking", "人氣排名", Some(("popular", "人氣排名"))),
	("high_rated_korean", "高分韓漫", Some(("korean", "高分韓漫"))),
	("new_releases", "最新上架", Some(("latest", "最新上架"))),
	("completed_recommendations", "完結推薦", Some(("completed", "完結"))),
];

struct HipmhSource;

/// A `/v1/mangas`-shaped page: `data.items` plus `total_pages`.
fn fetch_list_page(url: &str, page: i32) -> Result<MangaPageResult> {
	let data = fetch_data(url)?;
	let entries = json_field(&data, "items").map(parse_manga_list).unwrap_or_default();
	let total_pages = json_i64(&data, "total_pages").unwrap_or(0);
	Ok(MangaPageResult {
		has_next_page: !entries.is_empty() && i64::from(page) < total_pages,
		entries,
	})
}

/// A `/v1/search` page: `data.data` plus `total_pages`.
fn fetch_search_page(query: &str, page: i32) -> Result<MangaPageResult> {
	let data = fetch_data(&search_url(query, page, SEARCH_PAGE_SIZE))?;
	let entries = json_field(&data, "data").map(parse_manga_list).unwrap_or_default();
	let total_pages = json_i64(&data, "total_pages").unwrap_or(0);
	Ok(MangaPageResult {
		has_next_page: !entries.is_empty() && i64::from(page) < total_pages,
		entries,
	})
}

/// The site has no author lookup by name and `/v1/mangas?author=` only takes an id, so
/// search the name and keep the works that credit exactly that author. Works the fuzzy
/// search does not return are missed.
fn author_search(name: &str) -> Result<MangaPageResult> {
	let name = name.trim();
	let mut entries: Vec<Manga> = Vec::new();
	for page in 1..=AUTHOR_SEARCH_MAX_PAGES {
		let data = fetch_data(&search_url(name, page, AUTHOR_SEARCH_PAGE_SIZE))?;
		let rows = json_field(&data, "data").map(json_items).unwrap_or_default();
		for row in rows.iter() {
			if !json_names(row, "authors").iter().any(|author: &String| author == name) {
				continue;
			}
			if let Some(manga) = parse_manga(row, None) {
				if !entries.iter().any(|m: &Manga| m.key == manga.key) {
					entries.push(manga);
				}
			}
		}
		let total_pages = json_i64(&data, "total_pages").unwrap_or(0);
		if rows.is_empty() || i64::from(page) >= total_pages {
			break;
		}
	}
	Ok(MangaPageResult {
		entries,
		has_next_page: false,
	})
}

impl Source for HipmhSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut sort = String::from("updated");
		let mut status = String::new();
		let mut category = String::new();
		let mut genre = String::new();
		let mut author: Option<String> = None;

		for filter in filters {
			match filter {
				// `filters.json` supplies `ids`, so `value` is already the query value.
				FilterValue::Select { id, value } => match id.as_str() {
					"sort" if !value.is_empty() => sort = value,
					"status" => status = value,
					"category" => category = value,
					"genre" => genre = value,
					_ => {}
				},
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				_ => {}
			}
		}

		if let Some(query) = query.filter(|q: &String| !q.trim().is_empty()) {
			return fetch_search_page(query.trim(), page);
		}
		if let Some(author) = author.filter(|a: &String| !a.trim().is_empty()) {
			return author_search(&author);
		}

		let mut url = format!(
			"{API_URL}/v1/mangas?sort={sort}&page={page}&per_page={LIST_PER_PAGE}"
		);
		if !genre.is_empty() {
			// A tapped tag arrives as its name rather than the option id. The genre filter
			// is the only one marked `isGenre`, so only names from its list get here.
			let id = if genre.bytes().all(|b: u8| b.is_ascii_digit()) {
				Some(genre)
			} else {
				genres::id_for_name(&genre).map(|id: u32| format!("{id}"))
			};
			match id {
				Some(id) => url.push_str(&format!("&genre={id}")),
				None => {
					return Ok(MangaPageResult {
						entries: Vec::new(),
						has_next_page: false,
					})
				}
			}
		}
		if !status.is_empty() {
			url.push_str(&format!("&status={status}"));
		}
		if !category.is_empty() {
			url.push_str(&format!("&category={category}"));
		}
		fetch_list_page(&url, page)
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		if needs_details {
			let data = fetch_data(&format!("{API_URL}/v1/manga?mid={}", manga.key))?;
			let mut details = parse_manga(&data, Some(&manga.key))
				.ok_or_else(|| error!("[hipmh] unreadable details for {}", manga.key))?;
			// `/v1/manga` gives a numeric `id`; the library key must survive.
			details.key = manga.key.clone();
			details.url = Some(manga_url(&manga.key));
			details.chapters = manga.chapters.take();
			manga = details;
			send_partial_result(&manga);
		}

		if needs_chapters {
			// Pages of at most 50 are fetched one after another. Only a complete list is
			// kept: a partial one would read as chapters having been removed.
			let mut chapters: Vec<Chapter> = Vec::new();
			let mut page = 1;
			let complete = loop {
				let data = match fetch_data(&chapters_url(&manga.key, page)) {
					Ok(data) => data,
					Err(_) => break false,
				};
				let items = json_field(&data, "items").map(json_items).unwrap_or_default();
				chapters.extend(items.iter().filter_map(|obj: &&str| parse_chapter(obj)));
				let total_pages = json_i64(&data, "total_pages").unwrap_or(0);
				if items.is_empty() || i64::from(page) >= total_pages {
					break true;
				}
				page += 1;
			};
			if complete {
				manga.chapters = Some(chapters);
			} else {
				// One unreachable title must not abort a whole library refresh.
				println!("[hipmh] ERROR fetching chapters for {} at page {page}", manga.key);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let data = fetch_data(&format!("{API_URL}/v2/chapter?hid={}", chapter.key))?;

		// The reader decodes a string and passes an array through as it is.
		let mut images = match json_field(&data, "images") {
			Some(raw) if raw.starts_with('"') => {
				decoder::decode(&json_string(&data, "images").unwrap_or_default())?
			}
			Some(raw) => json_top_strings(raw),
			None => Vec::new(),
		};
		if let (Some(order_id), Some(sid)) = (json_i64(&data, "order_id"), json_i64(&data, "sid")) {
			if order_id >= 0 && sid >= 0 {
				decoder::remove_decoy(&mut images, order_id as u64, sid as u64);
			}
		}
		if images.is_empty() {
			bail!("[hipmh] no images for {}", chapter.key);
		}

		let host = image_host(json_i64(&data, "line"));
		Ok(images
			.into_iter()
			.map(|path: String| {
				let url = if path.starts_with("http://") || path.starts_with("https://") {
					path
				} else if path.starts_with('/') {
					format!("{host}{path}")
				} else {
					format!("{host}/{path}")
				};
				Page {
					content: PageContent::url(url),
					..Default::default()
				}
			})
			.collect())
	}
}

impl ListingProvider for HipmhSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let list = |query: &str| {
			format!("{API_URL}/v1/mangas?{query}&page={page}&per_page={LIST_PER_PAGE}")
		};
		let url = match listing.id.as_str() {
			"update" => list("sort=updated"),
			"popular" => list("sort=popular"),
			"latest" => list("sort=latest"),
			"completed" => list("sort=updated&status=completed"),
			"korean" => list("sort=updated&tag=69"),
			// The weekly ranking ignores `per_page` and always serves 18.
			"weekly" => format!("{API_URL}/v1/mangas/weekly?page={page}"),
			_ => bail!("Unknown listing: {}", listing.id),
		};
		fetch_list_page(&url, page)
	}
}

impl Home for HipmhSource {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately.
		let mut components: Vec<HomeComponent> = Vec::new();
		components.push(HomeComponent {
			title: None,
			subtitle: None,
			value: HomeComponentValue::empty_image_scroller(),
		});
		for (_, title, _) in HOME_ROWS {
			components.push(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			});
		}
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		// One request carries every row.
		let data = fetch_data(&format!("{API_URL}/v1/home"))?;

		let banners: Vec<Link> = json_field(&data, "banners")
			.map(json_items)
			.unwrap_or_default()
			.into_iter()
			.filter_map(parse_home_link)
			.map(|manga: Manga| Link {
				title: manga.title.clone(),
				subtitle: None,
				image_url: manga.cover.clone(),
				value: Some(LinkValue::Manga(manga)),
			})
			.collect();
		if !banners.is_empty() {
			// An image scroller rather than a big scroller, as in zh.creativecomic: a big
			// scroller fills the width and gives no sign that it can be swiped. At 300 wide
			// the next banner shows at the right edge. The banners are square.
			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: None,
				subtitle: None,
				value: HomeComponentValue::ImageScroller {
					links: banners,
					auto_scroll_interval: Some(5.0),
					width: Some(300),
					height: Some(300),
				},
			}));
		}

		for (field, title, listing) in HOME_ROWS {
			let Some(array) = json_field(&data, field) else {
				continue;
			};
			let mangas = if field == "featured" {
				json_items(array).into_iter().filter_map(parse_home_link).collect()
			} else {
				parse_manga_list(array)
			};
			if mangas.is_empty() {
				continue;
			}
			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: mangas.into_iter().map(Link::from).collect(),
					listing: listing.map(|(id, name): (&str, &str)| Listing {
						id: String::from(id),
						name: String::from(name),
						..Default::default()
					}),
				},
			}));
		}

		Ok(HomeLayout::default())
	}
}

impl DeepLinkHandler for HipmhSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let path = url
			.split_once("://")
			.map_or(url.as_str(), |(_, rest): (&str, &str)| rest);

		// `m.hipmh.com/chapter/go?hid=<reader hid>&m=...` or
		// `reader.hipmh.top/chapter/<reader hid>`.
		let reader_hid = if let Some((_, query)) = path.split_once("hid=") {
			query.split(['&', '#']).next()
		} else if let Some((_, rest)) = path.split_once("/chapter/") {
			rest.split(['/', '?', '#']).next()
		} else {
			None
		};
		if let Some((manga_key, key)) = reader_hid.and_then(api_hid) {
			return Ok(Some(DeepLinkResult::Chapter { manga_key, key }));
		}

		if let Some((_, rest)) = path.split_once("/works/") {
			let mid = rest.split(['/', '?', '#']).next().unwrap_or(rest);
			let key = short_mid(mid);
			if !key.is_empty() {
				return Ok(Some(DeepLinkResult::Manga {
					key: String::from(key),
				}));
			}
		}

		Ok(None)
	}
}

register_source!(HipmhSource, ListingProvider, Home, DeepLinkHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::alloc::vec;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn deep_links_resolve_works_and_chapters() {
		let source = HipmhSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from(
					"https://m.hipmh.com/works/bTo4MDgz-wo-tian-ming-da-fan-pai-8076"
				))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("bTo4MDgz")
			})
		);
		let chapter = Some(DeepLinkResult::Chapter {
			manga_key: String::from("bTo4MDgz"),
			key: String::from("Yzo5MjAz-ODA4MzoxLjAw"),
		});
		assert_eq!(
			source
				.handle_deep_link(String::from(
					"https://m.hipmh.com/chapter/go?hid=bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw&m=8083"
				))
				.unwrap(),
			chapter
		);
		assert_eq!(
			source
				.handle_deep_link(String::from(
					"https://reader.hipmh.top/chapter/bTo4MDgzLWM6OTIwMw-ODA4MzoxLjAw"
				))
				.unwrap(),
			chapter
		);
	}

	#[aidoku_test]
	fn reads_a_manga_with_all_its_chapters() {
		let source = HipmhSource::new();
		let manga = source
			.get_manga_update(
				Manga {
					key: String::from("bTo4MDgz"),
					..Default::default()
				},
				true,
				true,
			)
			.expect("update");
		assert_eq!(manga.title, "我！天命大反派");
		assert_eq!(manga.key, "bTo4MDgz");
		let chapters = manga.chapters.expect("chapters");
		// 361 on 2026-09-24; more than one page of 50 either way.
		assert!(chapters.len() >= 361);
		assert_eq!(chapters.last().map(|c: &Chapter| c.key.as_str()), Some("Yzo5MjAz-ODA4MzoxLjAw"));
	}

	#[aidoku_test]
	fn opens_chapter_one_without_the_decoy() {
		let pages = HipmhSource::new()
			.get_page_list(
				Manga::default(),
				Chapter {
					key: String::from("Yzo5MjAz-ODA4MzoxLjAw"),
					..Default::default()
				},
			)
			.expect("pages");
		assert_eq!(pages.len(), 150);
	}

	#[aidoku_test]
	fn searches_titles_and_authors() {
		let source = HipmhSource::new();
		let result = source
			.get_search_manga_list(Some(String::from("天命大反派")), 1, Vec::new())
			.expect("search");
		assert!(result.entries.iter().any(|m: &Manga| m.key == "bTo4MDgz"));

		let result = source
			.get_search_manga_list(
				None,
				1,
				vec![FilterValue::Text {
					id: String::from("author"),
					value: String::from("六芒"),
				}],
			)
			.expect("author search");
		assert!(result.entries.iter().any(|m: &Manga| m.key == "bTo4MDgz"));
		assert!(result
			.entries
			.iter()
			.all(|m: &Manga| m.authors.as_ref().is_some_and(|a: &Vec<String>| a.iter().any(|n: &String| n == "六芒"))));
	}

	#[aidoku_test]
	fn every_listing_loads() {
		let source = HipmhSource::new();
		for id in ["update", "popular", "weekly", "latest", "completed", "korean"] {
			let result = source
				.get_manga_list(
					Listing {
						id: String::from(id),
						..Default::default()
					},
					1,
				)
				.expect(id);
			assert!(!result.entries.is_empty(), "{id}");
		}
	}
}
