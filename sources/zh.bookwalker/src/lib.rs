#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{canvas::ImageRef, net::Request},
	prelude::*,
	Chapter, DynamicSettings, FilterValue, GroupSetting, HashMap, ImageRequestProvider,
	ImageResponse, Listing, ListingProvider, Manga, MangaPageResult, NotificationHandler, Page,
	PageContext, PageImageProcessor, Result, Setting, Source, WebLoginHandler,
};

mod auth;
mod crypto;
mod descramble;
mod helper;
mod reader;

use helper::*;

struct BookwalkerSource;

/// Shown while signed out. The group cannot simply be left out: a source that hands
/// the app an empty list of dynamic settings gets a settings screen whose buttons stop
/// responding.
const SIGNED_OUT_HINT: &str = "尚未登入。請按上方「登入」，登入完成後下方會出現書櫃數量。";
/// Shown when a cookie is stored but the book count is missing. The app's login row
/// still reads "登出" in that state, so signing out is the first step.
const SESSION_LOST_HINT: &str = "登入已失效，請按「登出」後重新登入。";

/// Fetch one bookcase page. The source's login state is its own stored cookie, not
/// the app's request jar: the app never carries the web login into that jar (verified
/// on device 2026-09-23), so the cookie is attached here. Gating on it makes logout
/// hide the books, and costs no request when signed out. `None` means signed out or
/// an expired cookie.
fn fetch_bookcase(url: &str) -> Result<Option<Vec<BookCard>>> {
	let Some(cookie) = auth::cookie_header() else {
		return Ok(None);
	};
	let html = auth::bookcase_request(url, &cookie)?.html()?;
	if !is_bookcase(&html) {
		return Ok(None);
	}
	Ok(Some(parse_bookcase(&html)))
}

/// The series view (`sd=1`) of purchased books, archived ones included.
fn bookcase_page(category: &str, query: Option<&str>) -> Result<MangaPageResult> {
	let url = format!("{BASE_URL}/bookcase/available_book_list/buy?sd=1&d=0&sort=0&c={category}");
	let cards = fetch_bookcase(&url)?.unwrap_or_default();
	// The site's bookcase has no keyword parameter that works, so titles are matched here.
	let needle = query.map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty());
	let entries = cards
		.iter()
		.filter(|card| match &needle {
			Some(needle) => card.name.to_lowercase().contains(needle.as_str()),
			None => true,
		})
		.filter_map(|card| card.to_manga())
		.collect();
	Ok(MangaPageResult {
		entries,
		// Every purchased book arrives on one page; whether a large bookcase pages
		// could not be checked with the test account.
		has_next_page: false,
	})
}

impl Source for BookwalkerSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		if page > 1 {
			return Ok(MangaPageResult::default());
		}
		let mut category = String::from("0");
		for filter in filters {
			// `filters.json` supplies `ids`, so `value` is already the site's `c` value.
			if let FilterValue::Select { id, value } = filter {
				if id == "category" {
					category = value;
				}
			}
		}
		bookcase_page(&category, query.as_deref())
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let cards = if let Some(series_id) = manga.key.strip_prefix(SERIES_PREFIX) {
			// `sort=7` lists volumes oldest first. The plain `?s=` page defaults to the
			// "all" shelf, which leaves out archived volumes, hence `/buy`.
			let url = format!(
				"{BASE_URL}/bookcase/available_book_list/buy?s={series_id}&sd=0&d=0&sort=7"
			);
			fetch_bookcase(&url)?
		} else if let Some(pid) = manga.key.strip_prefix(PRODUCT_PREFIX) {
			let url = format!("{BASE_URL}/bookcase/available_book_list/buy?sd=0&d=0&sort=0");
			fetch_bookcase(&url)?.map(|cards| {
				cards
					.into_iter()
					.filter(|card| card.product_ids.iter().any(|id| id == pid))
					.collect()
			})
		} else {
			bail!("unknown manga key {}", manga.key);
		};
		let Some(cards) = cards else {
			bail!("not logged in");
		};

		if needs_details {
			if let Some(newest) = cards.last() {
				let series_id = manga.key.strip_prefix(SERIES_PREFIX).map(String::from);
				let card = BookCard {
					series_id,
					product_ids: newest.product_ids.clone(),
					name: newest.name.clone(),
					authors: newest.authors.clone(),
					category: newest.category.clone(),
				};
				if let Some(details) = card.to_manga() {
					manga.copy_from(details);
				}
			}
		}

		if needs_chapters {
			let chapters: Vec<Chapter> = cards
				.iter()
				.rev()
				.filter_map(|card| card.to_chapter())
				.collect();
			manga.chapters = Some(chapters);
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		// The chapter key is the product id.
		reader::get_pages(&chapter.key)
	}
}

impl ImageRequestProvider for BookwalkerSource {
	fn get_image_request(&self, url: String, context: Option<PageContext>) -> Result<Request> {
		reader::image_request(url, context.as_ref())
	}
}

impl PageImageProcessor for BookwalkerSource {
	fn process_page_image(
		&self,
		response: ImageResponse,
		context: Option<PageContext>,
	) -> Result<ImageRef> {
		let bytes = response.image.data();
		match context.as_ref().and_then(|ctx| reader::descramble(&bytes, ctx)) {
			Some(image) => Ok(image),
			// Fall back to the raw image rather than failing the page.
			None => Ok(ImageRef::new(&bytes)),
		}
	}
}

impl ListingProvider for BookwalkerSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		if page > 1 {
			return Ok(MangaPageResult::default());
		}
		match listing.id.as_str() {
			"buy" => bookcase_page("0", None),
			_ => bail!("unknown listing {}", listing.id),
		}
	}
}

impl WebLoginHandler for BookwalkerSource {
	/// Called on every cookie change while the login page is open. The `cookies` the
	/// app passes hold only cookies whose domain contains the login page's host, and
	/// the site's login cookies live on the parent domain, so they are never in there;
	/// `auth` reads the per-source web store the login page writes to instead. `true`
	/// closes the login page and marks the source logged in, so it must not be returned
	/// before a login has actually been confirmed. `Err` is never returned for a failed
	/// attempt: the app shows its own generic message, and the page has to stay open so
	/// the user can finish signing in.
	fn handle_web_login(&self, key: String, _cookies: HashMap<String, String>) -> Result<bool> {
		if key != "login" {
			bail!("Invalid login key: {key}");
		}
		Ok(auth::capture_from_web_login())
	}
}

impl NotificationHandler for BookwalkerSource {
	fn handle_notification(&self, notification: String) {
		// Fires on both login and logout; `auth` tells them apart.
		if notification == "login" {
			auth::handle_login_notification();
		}
	}
}

impl DynamicSettings for BookwalkerSource {
	/// Reads defaults only and sends no request, so it stays instant even when the
	/// login `refreshes` fire it alongside a whole-library refresh. The purchased book
	/// count, stored at login, is the account variable and the visible proof that the
	/// login took. Never answers with an empty list.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let footer = if auth::is_logged_in() {
			match auth::book_count() {
				Some(count) => format!("已登入，書櫃有 {count} 個系列／單本。"),
				None => String::from(SESSION_LOST_HINT),
			}
		} else {
			String::from(SIGNED_OUT_HINT)
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
	BookwalkerSource,
	ListingProvider,
	WebLoginHandler,
	NotificationHandler,
	DynamicSettings,
	ImageRequestProvider,
	PageImageProcessor
);
