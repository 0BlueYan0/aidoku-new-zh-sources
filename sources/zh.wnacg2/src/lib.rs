#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue, GroupSetting, HashMap,
	Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult, ImageRequestProvider,
	Link, Listing, ListingProvider, Manga, MangaPageResult, NotificationHandler, Page, PageContent,
	PageContext, Result, Setting, Source, WebLoginHandler,
};

mod auth;
mod helper;

use helper::*;

/// Defaults for the two search-only filters. The site accepts no other values: anything
/// else is syntactically fine but comes back with zero results.
const DEFAULT_SORT: &str = "create_time_DESC";
const DEFAULT_SCOPE: &str = "_all";

/// Home screen rows, in display order. Six of them are lifted straight off the landing
/// page; `週榜` needs its own request. The titles must match the ones the streamed
/// components carry, because the app pairs a component to its skeleton row by title.
const HOME_SKELETON: [&str; 7] = [
	"最新更新",
	"週榜",
	"同人誌",
	"單行本",
	"雜誌&短篇",
	"韓漫",
	"Cosplay&寫真集",
];

/// The row only signed-in readers get (2026-10-30: the site hides the category without
/// the login cookie).
const HOME_KOREAN_TITLE: &str = "韓漫";

/// The ranking row's title and the listing it opens. The title repeats the listing's
/// `source.json` name so the row and the screen it opens read the same.
const HOME_RANK_TITLE: &str = "週榜";
const HOME_RANK_LISTING: &str = "rank_week";

/// Requests per `send_all`: the app runs at most 5 at once, and more in one call can hang (docs/aidoku-rs-api.md).
const HOME_BATCH: usize = 5;

struct WnacgSource;

fn listing(id: &str) -> Listing {
	Listing {
		id: String::from(id),
		name: String::from(listing_name(id).unwrap_or_default()),
		..Default::default()
	}
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

impl Source for WnacgSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut cate = String::new();
		let mut tag = String::new();
		let mut author = None;
		let mut sort = String::from(DEFAULT_SORT);
		let mut scope = String::from(DEFAULT_SCOPE);

		for filter in filters {
			match filter {
				// `filters.json` supplies `ids`, so `value` is already the query
				// parameter value and needs no lookup table.
				FilterValue::Select { id, value } => match id.as_str() {
					"cate" => cate = value,
					"sort" => sort = value,
					"scope" => scope = value,
					// Tapping a tag on a manga (`supportsTagSearch`).
					"genre" => tag = value,
					_ => {}
				},
				FilterValue::Text { id, value } if id == "tag" => tag = value,
				// Tapping an author or artist (`supportsAuthorSearch`). Authors only ever
				// come from the bracketed title prefix, which the site's search matches.
				// Artists hold the circle from the same prefix (`[circle (artist)]`).
				FilterValue::Text { id, value } if id == "author" || id == "artist" => {
					author = Some(value)
				}
				_ => {}
			}
		}

		// Keyword search, tag browsing and category browsing are three separate
		// endpoints with no way to combine them, so the most specific input wins.
		// Filtered on both sides of `or`: an empty query must not hide the author.
		if let Some(keyword) = query
			.filter(|value: &String| !value.trim().is_empty())
			.or(author)
			.filter(|value: &String| !value.trim().is_empty())
		{
			let html = fetch_html(&search_url(&keyword, &scope, &sort, page))?;
			return Ok(parse_list_page(&html, page, true));
		}

		if !tag.trim().is_empty() {
			let html = fetch_html(&tag_list_url(&tag, page))?;
			return Ok(parse_list_page(&html, page, false));
		}

		let url = if cate.is_empty() {
			album_list_url(page)
		} else {
			cate_list_url(&cate, page)
		};
		let html = fetch_html(&url)?;
		Ok(parse_list_page(&html, page, false))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		if !needs_details && !needs_chapters {
			return Ok(manga);
		}

		// Swallow failures: one unreachable album must not abort a whole library
		// refresh, and the library keeps the details and chapter it already had.
		let Ok(html) = fetch_html(&detail_url(&manga.key)) else {
			return Ok(manga);
		};
		let Ok(date_uploaded) = apply_detail(&html, &mut manga) else {
			return Ok(manga);
		};

		if needs_details {
			send_partial_result(&manga);
		}

		if needs_chapters && is_series(&html) {
			// A second request in this entry point, sent after the first has returned.
			// On failure the library keeps the chapters it had.
			if let Ok(chapters) = fetch_text(&chapters_url(&manga.key))
				.and_then(|json| parse_series_chapters(&json, &manga.key))
			{
				manga.chapters = Some(chapters);
			}
		} else if needs_chapters {
			// An album is one gallery of images with no chapter structure of its own,
			// so it always reports a single chapter.
			manga.chapters = Some(aidoku::alloc::vec![Chapter {
				key: manga.key.clone(),
				title: Some(String::from("全一話")),
				chapter_number: Some(1.0),
				date_uploaded,
				url: Some(detail_url(&manga.key)),
				..Default::default()
			}]);
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		// The image URLs carry a `verify` signature that expires within hours, so this
		// is fetched fresh on every read rather than cached.
		let script = fetch_text(&gallery_url(&chapter.key))?;
		let images = parse_imglist(&script);
		if images.is_empty() {
			bail!("找不到圖片，可能是網址已變更或伺服器暫時無法連線");
		}

		Ok(images
			.into_iter()
			.map(|image: String| Page {
				content: PageContent::url(image),
				..Default::default()
			})
			.collect())
	}
}

// ---------------------------------------------------------------------------
// Listings and home
// ---------------------------------------------------------------------------

impl ListingProvider for WnacgSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let url = match listing.id.as_str() {
			"update" => album_list_url(page),
			"rank_day" => rank_list_url("day", page),
			"rank_week" => rank_list_url("week", page),
			"rank_month" => rank_list_url("month", page),
			"rank_year" => rank_list_url("year", page),
			// Category listings are named after the site's own category ids.
			id => match id.strip_prefix("cate_").filter(|cate| !cate.is_empty()) {
				Some(cate) => cate_list_url(cate, page),
				None => return Err(error!("Unknown listing: {id}")),
			},
		};

		let html = fetch_html(&url)?;
		Ok(parse_list_page(&html, page, false))
	}
}

impl Home for WnacgSource {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send an empty skeleton first so the home screen lays out immediately, then
		// stream each row in as it arrives.
		let mut components: Vec<HomeComponent> = Vec::new();
		// Signed out, the landing page has no 韓漫 block, so its row would stay a bare
		// title. Logging in reloads the home screen with the row back.
		let logged_in = auth::is_logged_in();
		for title in HOME_SKELETON {
			if title == HOME_KOREAN_TITLE && !logged_in {
				continue;
			}
			components.push(HomeComponent {
				title: Some(String::from(title)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			});
		}
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		// One request covers six of the seven rows; the ranking board is not on the landing
		// page, so it costs a second. Both go out together so the home screen, and the covers
		// queued behind this call, wait for one round trip instead of two.
		// A request that cannot be built is left out and its rows stay empty; the other still loads.
		let mut rows = Vec::new();
		let mut requests: Vec<Request> = Vec::new();
		for (is_rank, url) in [(false, home_url()), (true, rank_list_url("week", 1))] {
			let Ok(request) = request(&url) else {
				continue;
			};
			rows.push((is_rank, url));
			requests.push(request);
		}
		let mut responses = Vec::new();
		let mut requests = requests.into_iter().peekable();
		while requests.peek().is_some() {
			let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
			responses.extend(Request::send_all(batch));
		}

		// Drawn one at a time so a short response list leaves the trailing rows empty instead
		// of shifting results onto the wrong rows.
		let mut responses = responses.into_iter();
		for (is_rank, url) in rows {
			let html = responses.next().map(|response| read_html(response?, &url));
			if is_rank {
				// Parsed as the `rank_week` listing's first page is.
				if let Some(Ok(html)) = html {
					send_scroller(
						String::from(HOME_RANK_TITLE),
						Some(String::from(HOME_RANK_LISTING)),
						parse_list_page(&html, 1, false).entries,
					);
				}
			} else {
				// A failing row should not take the whole home screen down.
				if let Some(Ok(html)) = html {
					for section in parse_home_sections(&html) {
						send_scroller(section.title, section.listing_id, section.entries);
					}
				}
			}
		}

		// The skeleton and every row have already been sent.
		Ok(HomeLayout::default())
	}
}

fn send_scroller(title: String, listing_id: Option<String>, entries: Vec<Manga>) {
	if entries.is_empty() {
		return;
	}

	let links: Vec<Link> = entries.into_iter().map(Link::from).collect();
	send_partial_result(&HomePartialResult::Component(HomeComponent {
		title: Some(title),
		subtitle: None,
		value: HomeComponentValue::Scroller {
			entries: links,
			listing: listing_id.map(|id: String| listing(&id)),
		},
	}));
}

// ---------------------------------------------------------------------------
// Images and deep links
// ---------------------------------------------------------------------------

impl ImageRequestProvider for WnacgSource {
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		let referer = base_url();
		Ok(Request::get(&url)?
			.header("User-Agent", USER_AGENT)
			.header("Referer", &referer))
	}
}

impl DeepLinkHandler for WnacgSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		let Some(aid) = aid_from_href(&url) else {
			return Ok(None);
		};

		if url.contains("/photos-index-aid-") {
			return Ok(Some(DeepLinkResult::Manga { key: aid }));
		}

		// A series chapter: `/photos-slide-aid-<chapter>-sid-<series>.html`.
		if let Some(sid) = sid_from_href(&url) {
			return Ok(Some(DeepLinkResult::Chapter {
				manga_key: sid,
				key: aid,
			}));
		}

		// Every other reader entry point maps onto the album's single chapter.
		if url.contains("/photos-slide-aid-")
			|| url.contains("/photos-gallery-aid-")
			|| url.contains("/photos-slist-aid-")
			|| url.contains("/download-index-aid-")
		{
			return Ok(Some(DeepLinkResult::Chapter {
				manga_key: aid.clone(),
				key: aid,
			}));
		}

		Ok(None)
	}
}

impl WebLoginHandler for WnacgSource {
	fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		if key != "login" {
			bail!("Invalid login key: {key}");
		}
		Ok(auth::capture_web_login(&cookies))
	}
}

impl NotificationHandler for WnacgSource {
	fn handle_notification(&self, notification: String) {
		// Fires on both login and logout; `auth` tells them apart.
		if notification == "login" {
			auth::handle_login_notification();
		}
	}
}

impl DynamicSettings for WnacgSource {
	/// Reads defaults only and sends no request. Never answers with an empty list: an
	/// empty list leaves every button on the settings page dead (CLAUDE.md).
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let footer = if auth::is_logged_in() {
			match auth::account() {
				Some(name) => format!("已登入：{name}"),
				None => String::from("已登入"),
			}
		} else {
			String::from("尚未登入，登入後可以看韓漫。")
		};

		Ok(aidoku::alloc::vec![GroupSetting {
			key: "accountInfo".into(),
			title: "帳號資訊".into(),
			items: Vec::new(),
			footer: Some(footer.into()),
			..Default::default()
		}
		.into()])
	}
}

register_source!(
	WnacgSource,
	ListingProvider,
	Home,
	ImageRequestProvider,
	DeepLinkHandler,
	WebLoginHandler,
	NotificationHandler,
	DynamicSettings
);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// The ranking row opens the weekly listing, so it must carry that listing's name.
	#[aidoku_test]
	fn home_rank_row_reuses_the_listing_name() {
		assert_eq!(Some(HOME_RANK_TITLE), listing_name(HOME_RANK_LISTING));
		assert!(HOME_SKELETON.contains(&HOME_RANK_TITLE));
	}

	/// The signed-out skeleton drops this row by title, so the title has to match the
	/// one the `cate_19` listing (and the streamed row) carries.
	#[aidoku_test]
	fn korean_row_title_matches_the_listing() {
		assert_eq!(Some(HOME_KOREAN_TITLE), listing_name("cate_19"));
		assert!(HOME_SKELETON.contains(&HOME_KOREAN_TITLE));
	}
}
