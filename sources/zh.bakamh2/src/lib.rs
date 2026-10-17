#![no_std]

use aidoku::{
	alloc::{vec, String, Vec},
	imports::{
		net::Request,
		std::{send_partial_result, sleep},
	},
	prelude::*,
	BasicLoginHandler, Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue,
	GroupSetting, Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult, Link,
	ImageRequestProvider, Listing, ListingProvider, Manga, MangaPageResult, NotificationHandler,
	Page, PageContext, Result, Setting,
	HashMap, Source, WebLoginHandler,
};

mod auth;
mod helper;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 5] = [
	("最近更新", "latest"),
	("熱門", "trending"),
	("最多觀看", "views"),
	("最高評分", "rating"),
	("最新上架", "new-manga"),
];

struct BakamhSource;

impl Source for BakamhSource {
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
			..Default::default()
		};
		for filter in filters {
			match filter {
				FilterValue::Text { id, value } if id == "author" => search.author = Some(value),
				// `filters.json` supplies `ids`, so `value` is already the query value.
				FilterValue::Select { id, value } => match id.as_str() {
					"sort" => search.order = Some(value),
					"match" => search.match_all = value == "all",
					// A tapped tag. Categories can be searched with everything else; a tag
					// has only its own page.
					"genre" if !value.is_empty() => match category_slug(&value) {
						Some(slug) => {
							if !search.categories.contains(&slug) {
								search.categories.push(slug);
							}
						}
						None => {
							let html = fetch_html_retrying(&tag_url(&value, page))?;
							return Ok(parse_manga_list(&html));
						}
					},
					_ => {}
				},
				FilterValue::MultiSelect { id, included, .. } => match id.as_str() {
					"category" => {
						for slug in included.iter().filter_map(|value| category_slug(value)) {
							if !search.categories.contains(&slug) {
								search.categories.push(slug);
							}
						}
					}
					"status" => search.statuses = included,
					_ => {}
				},
				_ => {}
			}
		}
		let url = search_url(&search, page);
		// The app runs this blank search by itself whenever content reloads (switching domains,
		// clearing the cache), before the home page. On device (2026-10-16) its four challenged
		// tries held the source for over a minute, so the home page never reloaded and a work
		// opened meanwhile hung. Only a search the reader asked for is worth waiting on.
		let html = if page == 1 && url == search_url(&Search::default(), 1) {
			fetch_html(&url)?
		} else {
			fetch_html_retrying(&url)?
		};
		Ok(parse_manga_list(&html))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		// Details and chapters are on the same page.
		// One blocked work must not abort a whole library refresh; the app keeps what it has.
		let html = match fetch_html(&manga_url(&manga.key)) {
			Ok(html) => html,
			Err(_) => {
				println!("[bakamh] ERROR could not load the page of {}", manga.key);
				return Ok(manga);
			}
		};
		// A block page must not empty the library entry.
		if !is_details_page(&html) {
			println!("[bakamh] ERROR not a details page for {}", manga.key);
			return Ok(manga);
		}

		if needs_details {
			if let Some(mut details) = parse_details(&html, &manga.key) {
				// The listing's cover stays. On device (2026-10-10) every work's cover vanished once
				// the details page swapped in the full-size image, though the URL is right and loads
				// in a browser; the cause was not found.
				if manga.cover.is_some() {
					details.cover = manga.cover.take();
				}
				let chapters = manga.chapters.take();
				manga = details;
				manga.chapters = chapters;
				send_partial_result(&manga);
			}
		}

		if needs_chapters {
			let chapters = parse_chapters(&html, &manga.key);
			if chapters.is_empty() {
				println!("[bakamh] ERROR no chapters on the page of {}", manga.key);
			} else {
				manga.chapters = Some(chapters);
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let url = chapter_url(&manga.key, &chapter.key);
		let mut html = fetch_html_retrying(&url)?;
		let mut pages = parse_pages(&html);
		// Only a sign-in lock is worth signing in again for; early access needs an invite
		// code, which no login provides.
		if pages.is_empty() && chapter_lock(&html) == Some(Lock::SignIn) && auth::retry_after_lock()
		{
			html = fetch_html_retrying(&url)?;
			pages = parse_pages(&html);
		}
		if pages.is_empty() {
			bail!(
				"[bakamh] no images for {}/{} ({:?})",
				manga.key,
				chapter.key,
				chapter_lock(&html)
			);
		}
		Ok(pages)
	}
}

impl ListingProvider for BakamhSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some(url) = listing_url(&listing.id, page) else {
			bail!("Unknown listing: {}", listing.id);
		};
		Ok(parse_manga_list(&fetch_html_retrying(&url)?))
	}
}

impl Home for BakamhSource {
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

		if uses_relay() {
			send_home_rows_together();
			return Ok(HomeLayout::default());
		}

		// Without a relay the rows go one after another: the app's Cloudflare handling gets a
		// round per request, so five rows are five chances, and challenges met at the same time
		// cancel each other (`canceled`).
		let mut blocked = Vec::new();
		for (title, id) in HOME_ROWS {
			let Some(url) = listing_url(id, 1) else {
				continue;
			};
			// A row that fails stays empty; the others still load.
			match fetch_page(&url, None) {
				Ok(Some(html)) => send_home_row(title, id, parse_manga_list(&html).entries),
				Ok(None) => blocked.push((title, id, url)),
				Err(_) => println!("[bakamh] ERROR loading the {id} row"),
			}
		}

		// The rows met while the app was still solving come back empty, and on device
		// (2026-10-16) the first three stayed empty until a pull to refresh. By the end of the
		// loop the challenge is usually passed, so each of them is asked once more, after the
		// same pause `fetch_html_retrying` uses so the closing challenge sheet does not cancel it.
		// Being offline is not retried.
		if !blocked.is_empty() {
			sleep(CHALLENGE_PAUSE);
			for (title, id, url) in blocked {
				match fetch_page(&url, None) {
					Ok(Some(html)) => send_home_row(title, id, parse_manga_list(&html).entries),
					Ok(None) => println!("[bakamh] ERROR the {id} row is still blocked after a retry"),
					Err(_) => println!("[bakamh] ERROR loading the {id} row"),
				}
			}
		}

		Ok(HomeLayout::default())
	}
}

/// Requests per `send_all`: the app runs at most 5 at once, and more in one call can hang (docs/aidoku-rs-api.md).
const HOME_BATCH: usize = 5;

/// The home rows through the relay, sent in one batch so the home screen, and the covers queued
/// behind this call, are not held up by five requests in a row. The relay solves each host's
/// challenge once and holds the others back meanwhile, so nothing here is cancelled.
fn send_home_rows_together() {
	// Rows whose request could not be built stay out of the batch and stay empty, as a failed
	// `fetch_html` left them.
	let mut rows = Vec::new();
	let mut requests = Vec::new();
	for (title, id) in HOME_ROWS {
		let Some(url) = listing_url(id, 1) else {
			continue;
		};
		match site_get(&url) {
			Ok(request) => {
				rows.push((title, id, url));
				requests.push(request);
			}
			Err(_) => println!("[bakamh] ERROR loading the {id} row"),
		}
	}

	let mut responses = Vec::new();
	let mut requests = requests.into_iter().peekable();
	while requests.peek().is_some() {
		let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
		responses.extend(Request::send_all(batch));
	}

	// Read in row order after the batch: `read_page` records the site's sign-in state, which
	// must not happen while requests are still being built.
	let mut responses = responses.into_iter();
	for (title, id, url) in rows {
		let page = match responses.next() {
			Some(Ok(response)) => read_page(&url, response),
			Some(Err(_)) | None => Err(error!("[bakamh] no response for {url}")),
		};
		let entries = match page {
			Ok(Some(html)) => parse_manga_list(&html).entries,
			// A challenge page, which `fetch_html` turns into an error.
			Ok(None) | Err(_) => {
				println!("[bakamh] ERROR loading the {id} row");
				continue;
			}
		};
		send_home_row(title, id, entries);
	}
}

/// Streams one filled row into the skeleton; an empty row keeps its placeholder.
fn send_home_row(title: &str, id: &str, entries: Vec<Manga>) {
	if entries.is_empty() {
		return;
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

impl DeepLinkHandler for BakamhSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		Ok(work_keys(&url).map(|(manga_key, chapter)| match chapter {
			Some(key) => DeepLinkResult::Chapter { manga_key, key },
			None => DeepLinkResult::Manga { key: manga_key },
		}))
	}
}

impl BasicLoginHandler for BakamhSource {
	fn handle_basic_login(&self, _key: String, username: String, password: String) -> Result<bool> {
		// The login dialog shows the app's own wording whatever the source returns, so a
		// failure is explained by the static footer in `settings.json` instead.
		Ok(auth::handle_login(&username, &password))
	}
}

impl NotificationHandler for BakamhSource {
	/// Aidoku sends the same notification for logging in and out, so `auth` tells them
	/// apart. Without this, logging out would leave the site's cookie in place.
	fn handle_notification(&self, notification: String) {
		if notification == "login" {
			auth::handle_login_notification();
		}
	}
}

impl DynamicSettings for BakamhSource {
	/// Never answers with an empty list: the app then stops responding to the settings
	/// screen's buttons. Signed out this makes no request either.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let footer = if auth::credentials().is_some() {
			auth::account_footer()
		} else if auth::has_foreign_session() {
			String::from("網站上仍是登入狀態。按「登出」可以登出網站，之後再用帳號密碼登入。")
		} else {
			match auth::last_failure() {
				Some(auth::LoginOutcome::Rejected) => {
					String::from("上次登入失敗：帳號或密碼錯誤，請重新登入。")
				}
				Some(_) => String::from("上次登入失敗：無法連線到巴卡漫畫，請稍後再登入一次。"),
				None => String::from(
					"尚未登入。登入後可閱讀標示「需登入」的章節，這裡會顯示帳號名稱。",
				),
			}
		};
		Ok(vec![GroupSetting {
			key: "accountInfo".into(),
			title: "帳號資訊".into(),
			items: Vec::new(),
			footer: Some(footer.into()),
			..Default::default()
		}
		.into()])
	}
}

impl WebLoginHandler for BakamhSource {
	/// The settings page closes the site and marks the button done once this is true, so it
	/// waits for the clearance cookie the check leaves behind (as zh.18mh does).
	fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		if !is_cloudflare_key(&key) {
			bail!("Invalid login key: {key}");
		}
		Ok(accept_clearance(&cookies))
	}
}

impl ImageRequestProvider for BakamhSource {
	/// Covers are on the site's own domain, behind its challenge, and the app fetches them
	/// itself, so with a relay they go through it too. Otherwise this is the plain request the
	/// app would have sent. Sends nothing.
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		// A cover saved under another mirror loads from the domain in use.
		let url = onto_base(&url, &base_url());
		if is_site_image(&url) && relay_url(&url).is_some() {
			return site_get(&url);
		}
		Ok(Request::get(url)?)
	}
}

register_source!(
	BakamhSource,
	ListingProvider,
	Home,
	DeepLinkHandler,
	BasicLoginHandler,
	NotificationHandler,
	DynamicSettings,
	WebLoginHandler,
	ImageRequestProvider
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
		for (_, value) in CATEGORIES {
			assert!(json.contains(&format!("\t\t\t\"{value}\"")), "{value}");
		}
		for order in ORDERS {
			assert!(json.contains(&format!("\"{order}\"")), "{order}");
		}
		for status in STATUSES {
			assert!(json.contains(&format!("\"{status}\"")), "{status}");
		}
		assert!(json.contains("\"all\"") && json.contains("\"any\""));
	}

	#[aidoku_test]
	fn cloudflare_button_waits_for_the_clearance() {
		let source = BakamhSource::new();
		let mut cookies: HashMap<String, String> = HashMap::new();
		for (key, _) in CLOUDFLARE_BUTTONS {
			assert!(!source.handle_web_login(String::from(key), cookies.clone()).unwrap());
		}
		cookies.insert(String::from("cf_clearance"), String::from("token"));
		for (key, _) in CLOUDFLARE_BUTTONS {
			assert!(source.handle_web_login(String::from(key), cookies.clone()).unwrap());
		}
		assert!(source.handle_web_login(String::from("login"), cookies).is_err());

		// One button per domain of `urls`, in the same order, each opening its own domain.
		let settings = include_str!("../res/settings.json");
		assert!(!settings.contains("urlKey"));
		let source_json = include_str!("../res/source.json");
		// Equal counts plus the per-row checks below make the three lists match one to one.
		assert_eq!(
			settings.matches("\"method\": \"web\"").count(),
			CLOUDFLARE_BUTTONS.len()
		);
		let (_, urls) = source_json.split_once("\"urls\": [").expect("urls");
		let (urls, _) = urls.split_once(']').expect("urls end");
		assert_eq!(urls.matches("\"https://").count(), CLOUDFLARE_BUTTONS.len());
		let mut last = 0;
		for (key, domain) in CLOUDFLARE_BUTTONS {
			let item = format!("\"key\": \"{key}\",");
			let at = settings.find(&item).expect(key);
			let end = settings[at..].find('}').expect(key);
			let url = format!("\"url\": \"{domain}\"");
			assert!(settings[at..at + end].contains(&url), "{key}");
			let pos = urls.find(&format!("\"{domain}\"")).expect(domain);
			assert!(pos >= last, "{domain}");
			last = pos + 1;
		}
	}

	#[aidoku_test]
	fn links_open_works_and_chapters() {
		let source = BakamhSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://bakamh.com/manga/not-sober/"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("not-sober")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://bakamh.com/manga/not-sober/c-23/"))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("not-sober"),
				key: String::from("c-23")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://bakamh.com/manga-genre/bl/"))
				.unwrap(),
			None
		);
	}
}
