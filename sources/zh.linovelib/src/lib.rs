#![no_std]

use aidoku::{
	alloc::{format, String, Vec},
	imports::{html::Html, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, Link, Listing, ListingProvider, Manga,
	MangaPageResult, Page, PageContent, Result, Source,
};

mod content;
mod helper;
mod reorder;
mod search;
mod tags;

use content::Segment;
use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 4] = [
	("月點擊榜", "monthvisit"),
	("最近更新", "lastupdate"),
	("最新入庫", "postdate"),
	("完本", "full"),
];

/// A chapter of 40 pages would already be far longer than any seen (6); the cap only
/// stops a loop if the site ever linked a page to itself.
const MAX_PAGES: usize = 40;

struct LinovelibSource;

impl Source for LinovelibSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut browse = Browse::default();
		let mut author = None;
		for filter in filters {
			match filter {
				// The site's search also matches author names.
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				// `filters.json` supplies `ids`, so `value` is already the site's value.
				FilterValue::Select { id, value } => match id.as_str() {
					"order" => browse.order = value,
					"isfull" => browse.isfull = value,
					"anime" => browse.anime = value,
					"rgroupid" => browse.region = value,
					"words" => browse.words = value,
					// A tapped tag: its name, in the language of the site it came from.
					"genre" => browse.tags.extend(tags::tag_id(&value).map(String::from)),
					_ => {}
				},
				FilterValue::MultiSelect { id, included, .. } if id == "tag" => {
					for value in included {
						if let Some(tag) = tags::tag_id(&value) {
							if !browse.tags.iter().any(|t: &String| t == tag) {
								browse.tags.push(String::from(tag));
							}
						}
					}
				}
				_ => {}
			}
		}
		let query = query
			.filter(|q: &String| !q.trim().is_empty())
			.or(author)
			.filter(|q: &String| !q.trim().is_empty());
		match query {
			Some(query) => search::search(&query, page),
			None => {
				let base = base_url();
				Ok(parse_manga_list(&fetch_html(&browse_url(base, &browse, page))?, base))
			}
		}
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let base = base_url();
		if needs_details {
			let html = fetch_html(&manga_url(base, &manga.key))?;
			match parse_details(&html, &manga.key, base) {
				Some(details) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				None => println!("[linovelib] ERROR no details on the page of {}", manga.key),
			}
		}

		if needs_chapters {
			// A failed catalogue must not abort a whole library refresh, nor empty the
			// chapters the app already has.
			match fetch_html(&catalog_url(base, &manga.key)) {
				Ok(html) => {
					let chapters = parse_catalog(&html, &manga.key, base);
					if chapters.is_empty() {
						println!("[linovelib] ERROR no chapters in the catalogue of {}", manga.key);
					} else {
						manga.chapters = Some(chapters);
					}
				}
				Err(err) => println!("[linovelib] ERROR catalogue of {}: {err:?}", manga.key),
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let base = base_url();
		let book = manga.key;
		let chapter_id = match parse_placeholder_key(&chapter.key) {
			// A chapter the catalogue lists without a link: its volume page has it.
			Some((volume, index)) => volume_chapter(&fetch_html(&volume_url(base, &book, volume))?, index)
				.ok_or_else(|| error!("[linovelib] no chapter {index} on volume {volume} of {book}"))?,
			None => chapter.key,
		};
		let seed: u64 = chapter_id
			.parse()
			.map_err(|_| error!("[linovelib] bad chapter id {chapter_id}"))?;

		// A chapter spans pages `<id>.html`, `<id>_2.html`, ...; each page says which
		// comes next, and the last one points at the next chapter instead.
		let mut url = chapter_url(base, &book, &chapter_id);
		let mut segments: Vec<Segment> = Vec::new();
		for _ in 0..MAX_PAGES {
			let raw = fetch_text(&url)?;
			if content::is_withheld(&raw) {
				bail!("[linovelib] {url}: the site withheld paragraphs");
			}
			let html = Html::parse_with_url(&raw, &url)?;
			content::append_page(&html, seed, &mut segments);
			match next_page_url(&raw) {
				Some(next) if is_same_chapter(next, &book, &chapter_id) => url = format!("{base}{next}"),
				_ => break,
			}
		}
		if segments.is_empty() {
			bail!("[linovelib] chapter {chapter_id} of {book} is empty");
		}

		// One text page with the illustrations inline. Images in a text page are loaded
		// without the source's image request, so they come from the site's own copy
		// rather than the image host that wants a Referer (device, 2026-10-04: as
		// separate image pages the text around them was cut off, and `data:` images in
		// the text are not shown).
		let markdown = content::markdown(&segments, |src: &str| attachment_url(base, src));
		Ok(Vec::from([Page {
			content: PageContent::text(markdown),
			..Default::default()
		}]))
	}
}

impl ListingProvider for LinovelibSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let base = base_url();
		let Some(url) = listing_url(base, &listing.id, page) else {
			bail!("[linovelib] unknown listing {}", listing.id);
		};
		Ok(parse_manga_list(&fetch_html(&url)?, base))
	}
}

impl Home for LinovelibSource {
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
		for (title, id) in HOME_ROWS {
			// A row that fails stays empty; the others still load.
			let entries = match listing_url(base, id, 1).map(|url: String| fetch_html(&url)) {
				Some(Ok(html)) => parse_manga_list(&html, base).entries,
				_ => {
					println!("[linovelib] ERROR loading the {id} row");
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

impl DeepLinkHandler for LinovelibSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		Ok(parse_link(&url).map(|(manga_key, chapter)| match chapter {
			Some(key) => DeepLinkResult::Chapter { manga_key, key },
			None => DeepLinkResult::Manga { key: manga_key },
		}))
	}
}

register_source!(LinovelibSource, ListingProvider, Home, DeepLinkHandler);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		let json = include_str!("../res/source.json");
		for (title, id) in HOME_ROWS.into_iter().chain(IMPRINTS.into_iter().map(|(id, title)| (title, id))) {
			assert!(json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{title}\"")), "{id}");
			assert!(listing_url(TW_URL, id, 1).is_some(), "{id}");
		}
		assert_eq!(
			listing_url(TW_URL, "emuefubunkojei", 2).as_deref(),
			Some("https://tw.linovelib.com/wenku/emuefubunkojei/2.html")
		);
	}

	#[aidoku_test]
	fn opens_links() {
		let source = LinovelibSource::new();
		assert_eq!(
			source.handle_deep_link(String::from("https://tw.linovelib.com/novel/2/catalog")).unwrap(),
			Some(DeepLinkResult::Manga { key: String::from("2") })
		);
		assert_eq!(
			source.handle_deep_link(String::from("https://www.bilinovel.com/novel/2/403_3.html")).unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("2"),
				key: String::from("403")
			})
		);
		assert_eq!(source.handle_deep_link(String::from("https://tw.linovelib.com/top.html")).unwrap(), None);
	}
}


