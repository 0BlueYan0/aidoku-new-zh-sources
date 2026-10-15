#![no_std]

use aidoku::{
	alloc::{vec, String, Vec},
	imports::{net::Request, std::send_partial_result},
	prelude::*,
	BasicLoginHandler, Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue,
	GroupSetting, Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult, Link,
	Listing, ListingProvider, Manga, MangaPageResult, NotificationHandler, Page, Result, Setting,
	Source,
};

mod auth;
mod helper;

use helper::*;

/// The home page's rows: heading and the listing behind it. The names must match
/// `res/source.json`; the streamed components are matched to the skeleton by title.
const HOME_ROWS: [(&str, &str); 2] = [("熱門推薦", "hot"), ("最近更新", "latest")];

/// Category `0` is the whole site, newest update first.
const ALL: &str = "0";

/// Requests per `send_all`: the app runs at most 5 at once, and more in one call can hang (docs/aidoku-rs-api.md).
const HOME_BATCH: usize = 5;

fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

/// A page of results. Neither list says reliably when it runs out (the search keeps
/// matching loosely, and `total` is a constant on some lists), so a page with works on
/// it may have a next one.
fn page_of(entries: Vec<Manga>) -> MangaPageResult {
	let has_next_page = !entries.is_empty();
	MangaPageResult {
		entries,
		has_next_page,
	}
}

fn list_page(url: &str) -> Result<MangaPageResult> {
	Ok(page_of(parse_manga_list(&fetch_checked(url, fetch_flight)?)))
}

fn category_page(id: &str, page: i32) -> Result<MangaPageResult> {
	list_page(&category_url(id, page))
}

/// The home page's recommendations, one page.
fn hot_page() -> Result<MangaPageResult> {
	Ok(MangaPageResult {
		entries: parse_manga_list(&fetch_checked(BASE_URL, fetch_flight)?),
		has_next_page: false,
	})
}

/// The page behind a home row, the same one its listing's first page reads.
fn home_row_url(id: &str) -> String {
	match id {
		"hot" => String::from(BASE_URL),
		_ => category_url(ALL, 1),
	}
}

/// The flight payloads of `urls` in order, fetched together. `None` for one that could
/// not be sent or read, which `fetch_checked` would also have given up on without asking
/// again.
fn send_flights(urls: &[String]) -> Vec<Option<String>> {
	let mut built = Vec::new();
	let mut requests: Vec<Request> = Vec::new();
	for url in urls {
		match flight_request(url) {
			Ok(request) => {
				built.push(true);
				requests.push(request);
			}
			Err(error) => {
				println!("[vomicmh] ERROR building the request for {url}: {error:?}");
				built.push(false);
			}
		}
	}
	let mut responses = Vec::new();
	let mut requests = requests.into_iter().peekable();
	while requests.peek().is_some() {
		let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
		responses.extend(Request::send_all(batch));
	}

	let mut responses = responses.into_iter();
	urls.iter()
		.zip(built)
		.map(|(url, built)| {
			if !built {
				return None;
			}
			match responses.next() {
				Some(Ok(response)) => match response.get_string() {
					Ok(payload) => Some(payload),
					Err(error) => {
						println!("[vomicmh] ERROR reading {url}: {error:?}");
						None
					}
				},
				Some(Err(error)) => {
					println!("[vomicmh] ERROR fetching {url}: {error:?}");
					None
				}
				None => None,
			}
		})
		.collect()
}

struct VomicSource;

impl Source for VomicSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let mut category = String::from(ALL);
		let mut author = None;
		for filter in filters {
			match filter {
				// The site's search also matches author names.
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				// `filters.json` supplies `ids`, so `value` is the category id.
				FilterValue::Select { id, value } if id == "category" => category = value,
				// A tapped tag: the category's name as the app shows it.
				FilterValue::Select { id, value } if id == "genre" => match category_id(value.trim()) {
					Some(id) => category = String::from(id),
					None => return Ok(empty_page()),
				},
				_ => {}
			}
		}
		let query = query
			.filter(|q: &String| !q.trim().is_empty())
			.or(author)
			.filter(|q: &String| !q.trim().is_empty());
		match query {
			Some(query) => list_page(&search_url(&query, page)),
			None => category_page(&category, page),
		}
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let url = manga_url(&manga.key);

		// A page the site failed to build leaves what the app has untouched, so a library
		// refresh during a failure neither stops nor wipes a work's chapters.
		if needs_details {
			match fetch_checked(&url, fetch_html).map(|html: String| parse_details(&html, &manga.key)) {
				Ok(Some(details)) => {
					let chapters = manga.chapters.take();
					manga = details;
					manga.chapters = chapters;
					send_partial_result(&manga);
				}
				Ok(None) => println!("[vomicmh] ERROR no details on the page of {}", manga.key),
				Err(error) => println!("[vomicmh] ERROR details of {}: {error:?}", manga.key),
			}
		}

		if needs_chapters {
			match fetch_checked(&url, fetch_flight) {
				// An empty list is the site's answer for works it keeps to its own app.
				Ok(payload) => manga.chapters = Some(parse_chapters(&payload, &manga.key)),
				Err(error) => println!("[vomicmh] ERROR chapters of {}: {error:?}", manga.key),
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let Some(token) = auth::token() else {
			bail!("[vomicmh] reading needs a login");
		};
		let original = wants_original_images();
		let mut pages = parse_pages(&fetch_reader(&manga.key, &chapter.key, &token)?, original);
		// Asked once more, as with category pages; an empty answer is also what a token
		// the site no longer accepts gets, which the settings page reports.
		if pages.is_empty() {
			println!("[vomicmh] no images for chapter {}, asking again", chapter.key);
			pages = parse_pages(&fetch_reader(&manga.key, &chapter.key, &token)?, original);
		}
		if pages.is_empty() {
			bail!("[vomicmh] no images for chapter {}", chapter.key);
		}
		Ok(pages)
	}
}

impl ListingProvider for VomicSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		match listing.id.as_str() {
			"latest" => category_page(ALL, page),
			"hot" if page > 1 => Ok(empty_page()),
			"hot" => hot_page(),
			_ => bail!("Unknown listing: {}", listing.id),
		}
	}
}

impl Home for VomicSource {
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

		// Rows go out together so the covers, which queue behind this call, start sooner.
		let urls: Vec<String> = HOME_ROWS.iter().map(|(_, id)| home_row_url(id)).collect();
		let mut payloads = send_flights(&urls);

		// Same allowance as `fetch_checked`: a page the site failed to build is asked for
		// once more, and a second failure leaves the row empty.
		let retry: Vec<usize> = payloads
			.iter()
			.enumerate()
			.filter(|(_, payload)| payload.as_deref().is_some_and(site_failed))
			.map(|(index, _)| index)
			.collect();
		if !retry.is_empty() {
			let again: Vec<String> = retry.iter().map(|&index| urls[index].clone()).collect();
			for url in &again {
				println!("[vomicmh] the site failed to build {url}, asking again");
			}
			for (index, payload) in retry.into_iter().zip(send_flights(&again)) {
				payloads[index] = payload;
			}
		}

		for ((title, id), payload) in HOME_ROWS.into_iter().zip(payloads) {
			// A row that fails stays empty; the other still loads.
			let entries = match payload {
				Some(payload) if !site_failed(&payload) => parse_manga_list(&payload),
				_ => Vec::new(),
			};
			if entries.is_empty() {
				println!("[vomicmh] ERROR loading the {id} row");
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

impl DeepLinkHandler for VomicSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let [key] = ids_after(&url, "detail")[..] {
			return Ok(Some(DeepLinkResult::Manga { key: String::from(key) }));
		}
		if let [manga_key, key] = ids_after(&url, "chapter")[..] {
			return Ok(Some(DeepLinkResult::Chapter {
				manga_key: String::from(manga_key),
				key: String::from(key),
			}));
		}
		Ok(None)
	}
}

impl BasicLoginHandler for VomicSource {
	fn handle_basic_login(&self, _key: String, username: String, password: String) -> Result<bool> {
		auth::login(&username, &password)
	}
}

impl NotificationHandler for VomicSource {
	fn handle_notification(&self, notification: String) {
		if notification == "login" {
			// The app sends this after logging in and after logging out alike; a recent
			// login timestamp marks the first.
			if auth::is_just_logged_in() {
				auth::clear_just_logged_in();
			} else {
				auth::clear_auth();
			}
		}
	}
}

/// Shown while signed out. The group cannot be left out: a source that hands the app an
/// empty list of dynamic settings gets a settings screen whose buttons stop responding.
const SIGNED_OUT_HINT: &str = "尚未登入，登入後才能閱讀。";
const INVALID_HINT: &str = "登入已失效，請先登出再重新登入。";

impl DynamicSettings for VomicSource {
	/// Never answers with an empty list. Only a stored token leads to a request.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let footer = match auth::token() {
			None => String::from(SIGNED_OUT_HINT),
			Some(token) => match auth::account(&token) {
				auth::Account::Name(name) => format!("已登入：{name}"),
				auth::Account::Rejected => String::from(INVALID_HINT),
				auth::Account::Unreachable => String::from("已登入（暫時讀不到帳號資訊，請檢查網路）"),
			},
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

register_source!(
	VomicSource,
	ListingProvider,
	Home,
	BasicLoginHandler,
	NotificationHandler,
	DynamicSettings,
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
		}
	}

	/// Reaches the site. curl rejects the details page's chunked body; this checks the
	/// request layer the runner shares with the app's.
	#[aidoku_test]
	#[ignore]
	fn loads_a_work_from_the_site() {
		let source = VomicSource::new();
		let manga = Manga {
			key: String::from("27035"),
			..Default::default()
		};
		let manga = source.get_manga_update(manga, true, true).expect("update");
		assert_eq!(manga.title, "哪咤");
		assert!(manga.chapters.is_some_and(|c: Vec<Chapter>| c.len() >= 4));
		let latest = source
			.get_manga_list(
				Listing {
					id: String::from("latest"),
					..Default::default()
				},
				1,
			)
			.expect("latest");
		assert!(!latest.entries.is_empty());
	}

	#[aidoku_test]
	fn links_open_without_a_request() {
		let source = VomicSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.vomicmh.com/detail/27035"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("27035")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.vomicmh.com/chapter/27035/25743"))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("27035"),
				key: String::from("25743")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://www.vomicmh.com/so/cate/4/1"))
				.unwrap(),
			None
		);
	}
}
