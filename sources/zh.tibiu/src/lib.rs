#![no_std]

use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		net::Request,
		std::send_partial_result,
	},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, FilterValue, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, ImageRequestProvider, Link, Listing, ListingProvider, Manga,
	MangaPageResult, Page, PageContent, PageContext, Result, Source,
};

mod helper;

use helper::*;

/// Every listing endpoint on this site returns ten items per page.
const PER_PAGE: usize = 10;

/// `source.json`'s listing ids paired with their display names. The names must match
/// `res/source.json` character for character, because the home screen matches the
/// streamed components to the skeleton by title.
const HOME_SECTIONS: [(&str, &str); 8] = [
	("update", "最新更新"),
	("day", "日榜"),
	("week", "周榜"),
	("month", "月榜"),
	("top", "总排行"),
	("fav", "收藏榜"),
	("ticket", "月票榜"),
	("ascension", "飙升榜"),
];

struct TibiuSource;

// ---------------------------------------------------------------------------
// Shared fetching
// ---------------------------------------------------------------------------

fn append_param(url: &mut String, name: &str, value: &str) {
	if !value.is_empty() {
		url.push_str(&format!("&{name}={value}"));
	}
}

/// The category browser reports `total_pages`, so paging can be exact.
fn fetch_category_page(url: &str, page: i32) -> Result<MangaPageResult> {
	let body = fetch_json(url)?;
	let entries = parse_comic_list(&body);
	let total_pages = json_num_value(&body, "total_pages").unwrap_or(0);

	Ok(MangaPageResult {
		has_next_page: !entries.is_empty() && i64::from(page) < total_pages,
		entries,
	})
}

/// Search and ranking report no total, so a full page is taken to imply another.
fn fetch_paged_list(url: &str) -> Result<MangaPageResult> {
	let body = fetch_json(url)?;
	let entries = parse_comic_list(&body);

	Ok(MangaPageResult {
		has_next_page: entries.len() >= PER_PAGE,
		entries,
	})
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

impl Source for TibiuSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut keyword = query;
		let mut order = String::new();
		let mut finish = String::new();
		let mut city = String::new();
		let mut theme = String::new();

		for filter in filters {
			match filter {
				// `filters.json` supplies `ids`, so `value` is already the query
				// parameter value and needs no lookup table.
				FilterValue::Select { id, value } => match id.as_str() {
					"order" => order = value,
					"finish" => finish = value,
					"city" => city = value,
					"theme" => theme = value,
					_ => {}
				},
				// The app routes its built-in author search through a text filter.
				// The site's search box covers titles and authors with the same field.
				FilterValue::Text { id, value }
					if id == "author" && keyword.is_none() && !value.is_empty() =>
				{
					keyword = Some(value);
				}
				_ => {}
			}
		}

		if let Some(keyword) = keyword {
			let trimmed = keyword.trim();
			if !trimmed.is_empty() {
				let url = format!(
					"{API_URL}/data/search?key={}&page={page}",
					encode_uri_component(trimmed)
				);
				return fetch_paged_list(&url);
			}
		}

		let mut url = format!("{API_URL}/data/category_api?page={page}");
		append_param(&mut url, "order", &order);
		append_param(&mut url, "finish", &finish);
		append_param(&mut url, "city", &city);
		append_param(&mut url, "theme", &theme);

		fetch_category_page(&url, page)
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		if needs_details {
			let url = format!("{API_URL}/comic/detail?id={}", manga.key);
			match fetch_json(&url) {
				Ok(body) => {
					if let Some(data) = json_data_field(&body, "data") {
						if let Some(mut details) = parse_comic(data) {
							// The detail response is authoritative for everything the
							// reader sees, but the library key and any chapters already
							// attached have to survive.
							details.key = manga.key;
							details.chapters = manga.chapters.take();
							manga = details;
						}
					}
				}
				Err(_) => println!("[tibiu] ERROR fetching details for id={}", manga.key),
			}
			send_partial_result(&manga);
		}

		if needs_chapters {
			let url = format!("{API_URL}/comic/chapter?mid={}", manga.key);
			// Swallow failures: one unreachable title must not abort a whole library
			// refresh.
			match fetch_json(&url) {
				Ok(body) => {
					let mut chapters: Vec<Chapter> = Vec::new();
					if let Some(array) = json_data_field(&body, "data") {
						for obj in json_top_level_objects(array) {
							if let Some(chapter) = parse_chapter(obj) {
								chapters.push(chapter);
							}
						}
					}

					// The API lists oldest first; Aidoku expects newest first.
					chapters.reverse();

					manga.chapters = Some(chapters);
				}
				Err(_) => println!("[tibiu] ERROR fetching chapters for mid={}", manga.key),
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let url = format!("{API_URL}/data/pic?cid={}", chapter.key);
		let body = fetch_json(&url)?;

		let mut pages: Vec<Page> = Vec::new();
		if let Some(array) = json_data_field(&body, "data") {
			for obj in json_top_level_objects(array) {
				if let Some(image) = json_text(obj, "img") {
					pages.push(Page {
						content: PageContent::url(image),
						..Default::default()
					});
				}
			}
		}

		if pages.is_empty() {
			// The server returns an empty list rather than an error for chapters the
			// current session is not entitled to read.
			bail!("此章节需要 VIP 或金币，本来源不支持登录");
		}

		Ok(pages)
	}
}

// ---------------------------------------------------------------------------
// Listings and home
// ---------------------------------------------------------------------------

impl ListingProvider for TibiuSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		match listing.id.as_str() {
			"update" => {
				let url = format!("{API_URL}/data/category_api?order=addtime&page={page}");
				fetch_category_page(&url, page)
			}
			"top" | "ticket" | "fav" | "day" | "week" | "month" | "ascension" => {
				let url = format!("{API_URL}/rankdata/lists?type={}&page={page}", listing.id);
				fetch_paged_list(&url)
			}
			_ => Err(error!("Unknown listing: {}", listing.id)),
		}
	}
}

impl Home for TibiuSource {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately, then
		// stream each row in as it arrives.
		let mut components: Vec<HomeComponent> = Vec::new();
		for (_, name) in HOME_SECTIONS {
			components.push(HomeComponent {
				title: Some(String::from(name)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			});
		}
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		for (id, name) in HOME_SECTIONS {
			let result = self.get_manga_list(
				Listing {
					id: String::from(id),
					name: String::from(name),
					..Default::default()
				},
				1,
			);

			// One failing row should not take the whole home screen down.
			if let Ok(result) = result {
				if !result.entries.is_empty() {
					let entries: Vec<Link> = result.entries.into_iter().map(Link::from).collect();
					send_partial_result(&HomePartialResult::Component(HomeComponent {
						title: Some(String::from(name)),
						subtitle: None,
						value: HomeComponentValue::Scroller {
							entries,
							listing: Some(Listing {
								id: String::from(id),
								name: String::from(name),
								..Default::default()
							}),
						},
					}));
				}
			}
		}

		Ok(HomeLayout::default())
	}
}

// ---------------------------------------------------------------------------
// Images and deep links
// ---------------------------------------------------------------------------

impl ImageRequestProvider for TibiuSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		// The CDN does not currently check Referer, but every other zh source sends one
		// and it costs nothing. No session needed: images live on a separate host that
		// serves them to anyone holding the URL.
		Ok(Request::get(&url)?
			.header("User-Agent", USER_AGENT)
			.header("Referer", BASE_URL))
	}
}

impl DeepLinkHandler for TibiuSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let path = match url.split_once("comic.tibiu.net") {
			Some((_, rest)) => rest,
			None => url.as_str(),
		};

		if let Some(rest) = path.strip_prefix("/chapter/") {
			let mut parts = rest.split('/');
			if let (Some(manga_key), Some(key)) = (parts.next(), parts.next()) {
				let key = trim_url_tail(key);
				if !manga_key.is_empty() && !key.is_empty() {
					return Ok(Some(DeepLinkResult::Chapter {
						manga_key: String::from(manga_key),
						key: String::from(key),
					}));
				}
			}
			return Ok(None);
		}

		if let Some(rest) = path.strip_prefix("/comic/") {
			let key = trim_url_tail(rest.split('/').next().unwrap_or(rest));
			if !key.is_empty() {
				return Ok(Some(DeepLinkResult::Manga {
					key: String::from(key),
				}));
			}
		}

		Ok(None)
	}
}

fn trim_url_tail(segment: &str) -> &str {
	segment
		.split(['?', '#'])
		.next()
		.unwrap_or(segment)
}

register_source!(
	TibiuSource,
	ListingProvider,
	Home,
	ImageRequestProvider,
	DeepLinkHandler
);
