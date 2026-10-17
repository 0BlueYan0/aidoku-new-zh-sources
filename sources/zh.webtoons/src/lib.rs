#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{
		html::Document,
		net::Request,
		std::{current_date, send_partial_result},
	},
	prelude::*,
	Chapter, ContentRating, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue,
	GroupSetting, HashMap, NotificationHandler, Setting, WebLoginHandler,
	Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult,
	ImageRequestProvider, Link, Listing, ListingProvider, Manga, MangaPageResult,
	MangaStatus, Page, PageContent, PageContext, Result, Source, Viewer,
};

mod helper;
mod models;
mod api;
mod auth;
#[cfg(all(test, feature = "live-tests"))]
mod live_tests;
use helper::*;

const BASE_URL: &str = "https://www.webtoons.com";
const LANG_PATH: &str = "/zh-hant";
const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

/// Mobile web API: preserves viewer URLs already used as library chapter keys.
const MOBILE_API: &str = "https://m.webtoons.com/api/v1/webtoon";

/// Requests per `send_all`: the app runs at most 5 at once, and more in one call can hang (docs/aidoku-rs-api.md).
const HOME_BATCH: usize = 5;

struct WebtoonSource;

fn with_cache_bust(url: &str) -> String {
	let ts = current_date() as i64;
	if url.contains('?') {
		format!("{url}&_={ts}")
	} else {
		format!("{url}?_={ts}")
	}
}

/// Build-only half of `fetch_manga_list`, so `get_home` can batch rows with `send_all`.
fn manga_list_request(url: &str) -> Result<Request> {
	let request_url = with_cache_bust(url);
	Ok(auth::with_web_session(Request::get(&request_url)?
		.header("Referer", BASE_URL)
		.header("User-Agent", USER_AGENT)
		.header("Cache-Control", "no-cache, no-store, must-revalidate")
		.header("Pragma", "no-cache")))
}

fn parse_manga_list(html: &Document) -> (Vec<Manga>, bool) {
	let mut entries: Vec<Manga> = Vec::new();

	if let Some(list) = html.select("ul.webtoon_list li a.link") {
		for item in list {
			if let Some(manga) = parse_manga_item(&item) {
				entries.push(manga);
			}
		}
	}

	let has_next_page = html.select_first(".pg_next").is_some();
	(entries, has_next_page)
}

/// Helper: fetch a page and parse manga items.
fn fetch_manga_list(url: &str) -> Result<(Vec<Manga>, bool)> {
	let html = manga_list_request(url)?.html()?;
	Ok(parse_manga_list(&html))
}

/// Shared by `get_manga_list` and `get_home` so the home rows hit the same pages as the listings.
fn listing_url(id: &str, page: i32) -> Option<String> {
	match id {
		"popular" => Some(format!(
			"{BASE_URL}{LANG_PATH}/ranking?sortOrder=MANA&page={page}"
		)),
		"monday" | "tuesday" | "wednesday" | "thursday"
		| "friday" | "saturday" | "sunday" | "complete" => Some(format!(
			"{BASE_URL}{LANG_PATH}/originals/{id}?sortOrder=MANA"
		)),
		_ => None,
	}
}

/// The mobile web list is a compatibility map, never the authoritative App list.
fn mobile_chapters(title_no: &str) -> Result<Vec<Chapter>> {
	let title_no = title_no.parse::<i32>().map_err(|_| error!("無效的作品編號"))?;
	let url = with_cache_bust(&format!("{MOBILE_API}/{title_no}/episodes?pageSize=99999"));
	let request = Request::get(url)?.timeout(20.0)
		.header("Referer", BASE_URL).header("User-Agent", USER_AGENT)
		.header("Cache-Control", "no-cache, no-store, must-revalidate");
	let body = auth::with_web_session(request).string()?;
	parse_episodes_json(&body)
}

/// HTML details remain useful when optional App metadata is unavailable.
/// Full chapter updates still require authoritative App metadata.
fn update_viewer(
	manga: &mut Manga,
	needs_chapters: bool,
	info: Result<models::TitleInfo>,
	episode_info: impl FnOnce(&str, i32) -> Result<models::EpisodeInfo>,
) -> Result<Option<models::TitleInfo>> {
	let info = match info {
		Ok(info) => info,
		Err(err) if needs_chapters => return Err(err),
		Err(_) => {
			println!("[webtoons] optional App details unavailable; retaining web details");
			return Ok(None);
		}
	};
	manga.viewer = if info.is_manga() {
		episode_info(&manga.key, info.first_episode_no).map(|episode| episode.viewer()).unwrap_or(manga.viewer)
	} else { Viewer::Webtoon };
	Ok(Some(info))
}

/// The public viewer is an independent reader, including after an expired
/// session. App APIs still check all permissions for App-only chapters.
fn read_chapter(
	manga: &Manga,
	chapter: &Chapter,
	logged_in: bool,
	title_info: impl FnOnce() -> Result<models::TitleInfo>,
	app_pages: impl FnOnce(i32, bool) -> Result<Vec<Page>>,
	web_pages: impl FnOnce(&str) -> Result<Vec<Page>>,
) -> Result<Vec<Page>> {
	let public_url = chapter.url.as_deref().filter(|url| url.starts_with("https://www.webtoons.com/"))
		.or_else(|| chapter.key.starts_with("https://www.webtoons.com/").then_some(chapter.key.as_str()));
	if !logged_in && manga.viewer == Viewer::Webtoon {
		if let Some(url) = public_url { return web_pages(url); }
	}
	let info = match title_info() {
		Ok(info) => info,
		Err(err) => return match public_url {
			Some(url) => web_pages(url),
			None => Err(err),
		},
	};
	if !info.is_manga() && !logged_in {
		if let Some(url) = public_url { return web_pages(url); }
	}
	let app_result = (|| {
		let episode = episode_no(&chapter.key).ok_or_else(|| error!("無效的章節編號"))?;
		app_pages(episode, info.is_manga())
	})();
	match app_result {
		Ok(pages) => Ok(pages),
		Err(err) => match public_url {
			Some(url) => {
				println!("[webtoons] App reader unavailable; trying public viewer");
				web_pages(url)
			}
			None => Err(err),
		}
	}
}

fn web_pages(viewer_url: &str) -> Result<Vec<Page>> {
	let viewer_url = with_cache_bust(viewer_url);
	let request = Request::get(&viewer_url)?.timeout(20.0)
		.header("Referer", BASE_URL)
		.header("User-Agent", USER_AGENT)
		.header("Cache-Control", "no-cache, no-store, must-revalidate")
		.header("Pragma", "no-cache");
	let html = auth::with_web_session(request).html()?;

	let mut pages: Vec<Page> = Vec::new();

	let image_selector = if html.select_first("#_imageList").is_some() {
		"#_imageList img"
	} else {
		".viewer_img img"
	};

	if let Some(images) = html.select(image_selector) {
		for img in images {
			let img_url: Option<String> = img
				.attr("data-url")
				.or_else(|| img.attr("src"));

			if let Some(url) = img_url {
				if url.contains("bg_transparency")
					|| url.contains("warning")
					|| url.contains("loading")
				{
					continue;
				}

				let mut context = PageContext::new();
				context.insert(
					String::from("Referer"),
					String::from("https://www.webtoons.com"),
				);

				pages.push(Page {
					content: PageContent::url_context(&url, context),
					..Default::default()
				});
			}
		}
	}

	if pages.is_empty() {
		bail!("WEBTOON 網頁未回傳閱讀圖片");
	}
	Ok(pages)
}

impl Source for WebtoonSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		if let Some(keyword) = query {
			let url = format!("{BASE_URL}{LANG_PATH}/search?keyword={keyword}");
			let (entries, _) = fetch_manga_list(&url)?;
			return Ok(MangaPageResult {
				entries,
				has_next_page: false,
			});
		}

		let mut genre_slug = "romance";
		let mut sort_order = "MANA";

		for filter in filters {
			if let FilterValue::Select { id, value } = filter {
				if id == "genre" {
					genre_slug = genre_name_to_slug(&value);
				} else if id == "sort" {
					sort_order = match value.as_str() {
						"愛心排序" => "LIKEIT",
						"最近更新" => "UPDATE",
						_ => "MANA",
					};
				}
			}
		}

		let url = format!(
			"{BASE_URL}{LANG_PATH}/genres/{genre_slug}?sortOrder={sort_order}&page={page}"
		);

		let (entries, has_next_page) = fetch_manga_list(&url)?;

		Ok(MangaPageResult {
			entries,
			has_next_page,
		})
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		if needs_details || needs_chapters { auth::ensure_session(); }
		let title_no = manga.key.clone();
		println!(
			"[webtoons] get_manga_update: title_no={}, needs_details={}, needs_chapters={}",
			title_no, needs_details, needs_chapters
		);

		if needs_details {
			let detail_url = if let Some(ref url) = manga.url {
				url.clone()
			} else {
				format!("{BASE_URL}{LANG_PATH}/originals/a/list?title_no={title_no}")
			};

			println!("[webtoons] fetching details: {}", detail_url);

			let detail_url = with_cache_bust(&detail_url);
			let request = Request::get(&detail_url)?
				.header("Referer", BASE_URL)
				.header("User-Agent", USER_AGENT)
				.header("Cache-Control", "no-cache, no-store, must-revalidate")
				.header("Pragma", "no-cache");
			let html = auth::with_web_session(request).html()?;

			if let Some(title_el) = html.select_first("h1.subj") {
				if let Some(text) = title_el.text() {
					manga.title = text;
				}
			} else if let Some(title_el) = html.select_first(".subj") {
				if let Some(text) = title_el.text() {
					manga.title = text;
				}
			}

			if let Some(author_el) = html.select_first(".author_area") {
				if let Some(text) = author_el.text() {
					let cleaned = text
						.replace("Writer Info", "")
						.replace("作家資訊", "");
					let authors: Vec<String> = cleaned
						.split(',')
						.flat_map(|s: &str| s.split('/'))
						.map(|s: &str| String::from(s.trim()))
						.filter(|s: &String| !s.is_empty())
						.collect();
					if !authors.is_empty() {
						manga.authors = Some(authors);
					}
				}
			}

			if let Some(desc_el) = html.select_first("p.summary") {
				if let Some(text) = desc_el.text() {
					manga.description = Some(text);
				}
			} else if let Some(desc_el) = html.select_first(".summary") {
				if let Some(text) = desc_el.text() {
					manga.description = Some(text);
				}
			}

			if let Some(genre_el) = html.select_first(".genre") {
				if let Some(text) = genre_el.text() {
					manga.tags = Some(aidoku::alloc::vec![text]);
				}
			}

			// Only set cover from og:image if not already set from listing
			if manga.cover.is_none() {
				if let Some(meta_el) = html.select_first("meta[property='og:image']") {
					if let Some(cover_url) = meta_el.attr("content") {
						manga.cover = Some(cover_url);
					}
				}
			}

			let is_completed = html.select_first(".ico_completed").is_some();
			manga.status = if is_completed {
				MangaStatus::Completed
			} else {
				MangaStatus::Ongoing
			};

			manga.content_rating = ContentRating::Safe;
			if manga.viewer == Viewer::Unknown { manga.viewer = Viewer::Webtoon; }
			println!("[webtoons] details fetched OK for title_no={}", title_no);
		}

		if needs_details || needs_chapters {
			let info = update_viewer(&mut manga, needs_chapters, api::title_info(&title_no), api::episode_info)?;
			if let Some(info) = info.filter(|_| needs_chapters) {
				let mut legacy = mobile_chapters(&title_no)?;
				// A library refresh may also provide stored keys from before a slug change.
				if let Some(previous) = manga.chapters.as_ref() {
					legacy.extend(previous.iter().cloned());
				}
				let episodes = api::episodes(&title_no, info.total_episode_count)?;
				let mut chapters = models::chapters_from_episodes(episodes, &legacy, info.is_manga())?;
				let rights = api::product_rights(&title_no)?;
				println!("[webtoons] title {}: {} chapters, {} returned rights, logged_in={}",
					title_no, chapters.len(), rights.len(), auth::is_logged_in());
				models::apply_rights(&mut chapters, &title_no, &rights, current_date() * 1000);
				api::remember_keys(&title_no, &mut chapters);
				manga.chapters = Some(chapters);
			}
		}

		println!("[webtoons] get_manga_update done for title_no={}", title_no);
		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		auth::ensure_session();
		read_chapter(&manga, &chapter, auth::is_logged_in(),
			|| api::title_info(&manga.key),
			|episode, is_manga| api::app_pages(&manga.key, episode, is_manga),
			web_pages)
	}

}

impl ListingProvider for WebtoonSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let Some(url) = listing_url(&listing.id, page) else {
			bail!("Unknown listing: {}", listing.id);
		};
		// Weekday and completed pages are not paginated.
		if page > 1 && listing.id != "popular" {
			return Ok(MangaPageResult {
				entries: Vec::new(),
				has_next_page: false,
			});
		}

		let (entries, has_next_page) = fetch_manga_list(&url)?;

		Ok(MangaPageResult {
			entries,
			has_next_page,
		})
	}
}

impl Home for WebtoonSource {
	fn get_home(&self) -> Result<HomeLayout> {
		send_partial_result(&HomePartialResult::Layout(HomeLayout {
			components: aidoku::alloc::vec![
				HomeComponent {
					title: Some(String::from("人氣排行")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週一連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週二連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週三連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週四連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週五連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週六連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("週日連載")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
				HomeComponent {
					title: Some(String::from("完結作品")),
					subtitle: None,
					value: HomeComponentValue::empty_scroller(),
				},
			],
		}));

		let sections = [
			("popular", "人氣排行"),
			("monday", "週一連載"),
			("tuesday", "週二連載"),
			("wednesday", "週三連載"),
			("thursday", "週四連載"),
			("friday", "週五連載"),
			("saturday", "週六連載"),
			("sunday", "週日連載"),
			("complete", "完結作品"),
		];

		// Rows go out together so the covers, which queue behind this call, start sooner.
		let mut rows = Vec::new();
		let mut requests: Vec<Request> = Vec::new();
		for (id, name) in sections {
			match listing_url(id, 1).map(|url| manga_list_request(&url)) {
				Some(Ok(request)) => {
					rows.push((id, name));
					requests.push(request);
				}
				_ => println!("[webtoons] home row {} could not build its request", id),
			}
		}
		let mut responses = Vec::new();
		let mut requests = requests.into_iter().peekable();
		while requests.peek().is_some() {
			let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
			responses.extend(Request::send_all(batch));
		}

		let mut responses = responses.into_iter();
		for (id, name) in rows {
			let entries = match responses.next().map(|r| r.and_then(|res| res.get_html())) {
				Some(Ok(html)) => parse_manga_list(&html).0,
				Some(Err(err)) => {
					println!("[webtoons] home row {} failed: {:?}", id, err);
					continue;
				}
				None => {
					println!("[webtoons] home row {} got no response", id);
					continue;
				}
			};
			if !entries.is_empty() {
				let links: Vec<Link> = entries.into_iter().map(Link::from).collect();
				send_partial_result(&HomePartialResult::Component(HomeComponent {
					title: Some(String::from(name)),
					subtitle: None,
					value: HomeComponentValue::Scroller {
						entries: links,
						listing: Some(Listing {
							id: String::from(id),
							name: String::from(name),
							..Default::default()
						}),
					},
				}));
			}
		}

		Ok(HomeLayout::default())
	}
}

impl ImageRequestProvider for WebtoonSource {
	fn get_image_request(
		&self,
		url: String,
		context: Option<PageContext>,
	) -> Result<Request> {
		api::image_request(url, context.as_ref())
	}
}

impl DeepLinkHandler for WebtoonSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(title_no) = extract_title_no(&url) {
			if url.contains("/viewer") {
				let episode = episode_no(&url).ok_or_else(|| error!("無效的章節連結"))?;
				let key = match api::saved_key(&title_no, episode) {
					Some(key) => key,
					None => mobile_chapters(&title_no)?.into_iter()
						.find(|chapter| episode_no(&chapter.key) == Some(episode))
						.map(|chapter| chapter.key)
						.unwrap_or_else(|| format!("{episode}")),
				};
				Ok(Some(DeepLinkResult::Chapter {
					manga_key: title_no,
					key,
				}))
			} else {
				Ok(Some(DeepLinkResult::Manga { key: title_no }))
			}
		} else {
			Ok(None)
		}
	}
}

register_source!(
	WebtoonSource,
	Home,
	ListingProvider,
	ImageRequestProvider,
	DeepLinkHandler,
	WebLoginHandler,
	NotificationHandler,
	DynamicSettings
);

/// Aidoku's web-login commit removes its login setting when the callback map
/// is empty, even if the source verified cookies from the shared WebView store.
fn native_web_login_result(verified: bool, cookies: &HashMap<String, String>) -> bool {
	if verified && cookies.is_empty() {
		println!("[webtoons] verified login: empty callback map, keeping native sheet open");
	}
	verified && !cookies.is_empty()
}

impl WebLoginHandler for WebtoonSource {
	fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		if key != "login" { bail!("Invalid login key: {key}"); }
		Ok(native_web_login_result(auth::capture_web_login(false), &cookies))
	}
}

impl NotificationHandler for WebtoonSource {
	fn handle_notification(&self, notification: String) {
		match notification.as_str() {
			"login" => auth::handle_login_notification(),
			"syncLogin" => {
				let synced = auth::capture_web_login(true);
				println!("[webtoons] manual login sync succeeded={synced}");
			}
			_ => {}
		}
	}
}

impl DynamicSettings for WebtoonSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(aidoku::alloc::vec![GroupSetting {
			key: "accountInfo".into(),
			title: "帳號資訊".into(),
			footer: Some(auth::account_footer().into()),
			items: Vec::new(),
			..Default::default()
		}.into()])
	}
}

#[cfg(test)]
mod login_tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn verified_login_with_empty_callback_cannot_tell_app_to_clear_its_login_key() {
		let mut cookies = HashMap::new();
		assert!(!native_web_login_result(true, &cookies));
		cookies.insert("locale".into(), "zh-hant".into());
		assert!(native_web_login_result(true, &cookies));
		assert!(!native_web_login_result(false, &cookies));
	}
}

#[cfg(test)]
mod fallback_tests {
	use super::*;
	use aidoku_test::aidoku_test;

	fn public_chapter() -> Chapter {
		Chapter { key: "https://www.webtoons.com/zh-hant/a/b/viewer?title_no=1&episode_no=1".into(), ..Default::default() }
	}

	#[aidoku_test]
	fn details_survive_api_failure_but_full_chapter_updates_fail() {
		let mut manga = Manga { title: "網頁詳情".into(), description: Some("簡介".into()), viewer: Viewer::LeftToRight, ..Default::default() };
		assert!(update_viewer(&mut manga, false, Err(error!("offline")), |_, _| panic!("no episode request")).unwrap().is_none());
		assert_eq!(manga.title, "網頁詳情");
		assert_eq!(manga.description.as_deref(), Some("簡介"));
		assert_eq!(manga.viewer, Viewer::LeftToRight);
		assert!(update_viewer(&mut manga, true, Err(error!("offline")), |_, _| panic!("no episode request")).is_err());
	}

	#[aidoku_test]
	fn anonymous_public_webtoon_reads_without_app_api() {
		let manga = Manga { viewer: Viewer::Webtoon, ..Default::default() };
		let pages = read_chapter(&manga, &public_chapter(), false,
			|| panic!("public webtoon must not need titleInfo"),
			|_, _| panic!("no App reader"),
			|_| Ok(aidoku::alloc::vec![Page::default()])).unwrap();
		assert_eq!(pages.len(), 1);
	}

	#[aidoku_test]
	fn api_outage_and_expired_session_can_read_public_viewer_in_same_call() {
		let manga = Manga { viewer: Viewer::RightToLeft, ..Default::default() };
		let chapter = public_chapter();
		assert_eq!(read_chapter(&manga, &chapter, true,
			|| Err(error!("titleInfo offline")), |_, _| panic!("no App reader"),
			|_| Ok(aidoku::alloc::vec![Page::default()])).unwrap().len(), 1);
		let info = models::TitleInfo { viewer: "MANGA".into(), first_episode_no: 1, total_episode_count: 1 };
		assert_eq!(read_chapter(&manga, &chapter, true,
			|| Ok(info), |_, manga| { assert!(manga); Err(error!("登入已失效")) },
			|_| Ok(aidoku::alloc::vec![Page::default()])).unwrap().len(), 1);
	}

	#[aidoku_test]
	fn app_only_failure_and_empty_public_viewer_are_not_successful_reading() {
		let manga = Manga::default();
		let chapter = Chapter { key: "2".into(), ..Default::default() };
		assert!(read_chapter(&manga, &chapter, false, || Err(error!("offline")),
			|_, _| panic!("no App reader"), |_| panic!("no fabricated web URL")).is_err());
		assert!(read_chapter(&Manga { viewer: Viewer::Webtoon, ..Default::default() }, &public_chapter(), false,
			|| panic!("no titleInfo"), |_, _| panic!("no App reader"), |_| Err(error!("no images"))).is_err());
	}
}
