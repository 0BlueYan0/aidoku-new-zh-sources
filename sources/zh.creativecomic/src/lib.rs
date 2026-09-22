#![no_std]

use aidoku::{
	alloc::{vec, String, Vec},
	imports::{canvas::ImageRef, net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue, GroupSetting,
	HashMap, Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult,
	ImageResponse, Link, LinkValue, Listing, ListingProvider, Manga, MangaPageResult,
	NotificationHandler, Page, PageContent, PageContext, PageImageProcessor, Result,
	Setting, Source, WebLoginHandler,
};

mod auth;
mod crypto;
mod helper;

use helper::*;

/// `source.json`'s listing ids paired with their display names. The home screen matches
/// streamed components to the skeleton by title, so these strings must stay in sync.
const BANNER_TITLE: &str = "精選";
const RANK_TITLE: &str = "閱覽排行";

/// The app runs a limited number of requests at once (5 unless `source.json` says
/// otherwise), so batched fan-out never hands it more than it will run.
const REQUEST_BATCH: usize = 5;

struct CreativeComicSource;

/// Map a listing id onto the `/book` query it stands for: a sort order, and optionally
/// an update window.
fn listing_query(id: &str) -> Option<(&'static str, Option<&'static str>)> {
	match id {
		"update" => Some(("updated_at", None)),
		"week" => Some(("updated_at", Some("week"))),
		"month" => Some(("updated_at", Some("month"))),
		"read" => Some(("read_count", None)),
		"like" => Some(("like_count", None)),
		"collect" => Some(("collect_count", None)),
		_ => None,
	}
}

fn fetch_list(path: &str, page: i32) -> Result<MangaPageResult> {
	let paged: Paged = api_get(path)?;
	let entries: Vec<Manga> = paged.data.into_iter().map(Book::into_manga).collect();
	Ok(MangaPageResult {
		has_next_page: page * PAGE_SIZE < paged.total,
		entries,
	})
}

/// The API lists chapters oldest first; Aidoku expects newest first.
fn build_chapters(list: ChapterList) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = list
		.chapters
		.iter()
		.map(|entry| {
			let key = format!("{}", entry.id);
			Chapter {
				url: Some(chapter_url(&key)),
				key,
				title: chapter_title(entry),
				chapter_number: chapter_number(entry.vol_name.as_deref()),
				date_uploaded: parse_timestamp(entry.online_at.as_deref()),
				..Default::default()
			}
		})
		.collect();
	chapters.reverse();
	chapters
}

impl Source for CreativeComicSource {
	fn new() -> Self {
		auth::purge_experimental_state();
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		auth::ensure_guest_uuid();

		let mut keyword = query;
		let mut sort_by = "updated_at";
		let mut genre: Option<String> = None;

		for filter in filters {
			match filter {
				// Author search goes through the site's full-text search: its dedicated
				// `author` parameter takes a numeric id, not a name, while `keyword`
				// does match author names.
				FilterValue::Text { id, value } => {
					if id == "author" && !value.is_empty() {
						keyword = Some(value);
					}
				}
				FilterValue::Select { id, value } => match id.as_str() {
					// `filters.json` supplies `ids`, so `value` is already the query
					// parameter value. It is still matched against a known set rather
					// than pasted into the URL.
					"sort" => {
						sort_by = match value.as_str() {
							"read_count" => "read_count",
							"like_count" => "like_count",
							"collect_count" => "collect_count",
							_ => "updated_at",
						}
					}
					"type" => {
						if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
							genre = Some(value);
						}
					}
					// A tapped tag. The site exposes no tag-name lookup, so this falls
					// back to full-text search, which does match tags.
					"genre" if keyword.is_none() && !value.is_empty() => {
						keyword = Some(value);
					}
					_ => {}
				},
				_ => {}
			}
		}

		let path = book_list_path(page, keyword.as_deref(), sort_by, genre.as_deref(), None);
		fetch_list(&path, page)
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		auth::ensure_guest_uuid();

		if needs_details {
			// Swallow failures: one unreachable book must not abort a whole library
			// refresh.
			match api_get::<Book>(&format!("/book/{}/info", manga.key)) {
				Ok(book) => manga.copy_from(book.into_manga()),
				Err(_) => println!("[ccc] ERROR fetching details for book {}", manga.key),
			}
			send_partial_result(&manga);
		}

		if needs_chapters {
			match api_get::<ChapterList>(&format!("/book/{}/chapter", manga.key)) {
				Ok(list) => manga.chapters = Some(build_chapters(list)),
				Err(_) => println!("[ccc] ERROR fetching chapters for book {}", manga.key),
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		auth::ensure_guest_uuid();

		// Paid chapters answer 403 here, with no page list at all.
		let content: ChapterContent = api_get(&format!("/book/chapter/{}", chapter.key))?;
		let Some(detail) = content.chapter else {
			bail!("章節 {} 沒有內容", chapter.key);
		};
		if detail.proportion.is_empty() {
			bail!("章節 {} 沒有頁面", chapter.key);
		}

		// Every page is encrypted under its own key. The keys are stable, so they are
		// all fetched up front in parallel and carried along in each page's context;
		// that also pins them to the token in use now, which keeps working if the
		// session refreshes mid-chapter.
		let secret = auth::image_secret();
		let mut responses = Vec::new();
		for batch in detail.proportion.chunks(REQUEST_BATCH) {
			let mut requests: Vec<Request> = Vec::new();
			for proportion in batch {
				requests.push(api_request(&format!(
					"/book/chapter/image/{}",
					proportion.id
				))?);
			}
			responses.extend(Request::send_all(requests));
		}

		// Drawn one at a time rather than zipped: were the response list ever to come
		// back short, zipping would drop those pages off the end of the chapter without
		// a word, while this leaves them in place as pages that failed to decrypt.
		let mut responses = responses.into_iter();
		let mut pages: Vec<Page> = Vec::new();
		for proportion in &detail.proportion {
			let unwrapped = responses
				.next()
				.and_then(|response| response.ok())
				.and_then(|response| response.get_json_owned::<Envelope<ImageKey>>().ok())
				.and_then(|envelope| envelope.data)
				.and_then(|payload| crypto::unwrap_page_key(&payload.key, &secret));

			let mut context = PageContext::new();
			match unwrapped {
				Some((key, iv)) => {
					context.insert(String::from("key"), crypto::to_hex(&key));
					context.insert(String::from("iv"), crypto::to_hex(&iv));
				}
				// Leaving the context empty makes the failure visible as one broken
				// page rather than a dead chapter.
				None => println!("[ccc] ERROR unwrapping the key for page {}", proportion.id),
			}

			pages.push(Page {
				content: PageContent::url_context(page_image_url(proportion.id), context),
				..Default::default()
			});
		}

		Ok(pages)
	}
}

impl ListingProvider for CreativeComicSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		auth::ensure_guest_uuid();

		let Some((sort_by, window)) = listing_query(&listing.id) else {
			return Err(error!("Unknown listing: {}", listing.id));
		};
		let path = book_list_path(page, None, sort_by, None, window);
		fetch_list(&path, page)
	}
}

impl Home for CreativeComicSource {
	fn get_home(&self) -> Result<HomeLayout> {
		auth::ensure_guest_uuid();

		// Send an empty skeleton first so the home screen lays out immediately, then
		// stream each row in. One request covers both rows.
		send_partial_result(&HomePartialResult::Layout(HomeLayout {
			components: vec![
				HomeComponent {
					title: Some(String::from(BANNER_TITLE)),
					subtitle: None,
					value: HomeComponentValue::empty_image_scroller(),
				},
				HomeComponent {
					title: Some(String::from(RANK_TITLE)),
					subtitle: None,
					value: HomeComponentValue::empty_manga_list(),
				},
			],
		}));

		match api_get::<HomeV2>("/public/home_v2") {
			Ok(home) => {
				// Banners also point at announcements and external pages; only the
				// ones tied to a book can open anything here.
				let links: Vec<Link> = home
					.banner
					.into_iter()
					.filter(|banner| banner.kind.as_deref() == Some("book"))
					.filter_map(|banner| {
						let key = banner.value.filter(|value| !value.is_empty())?;
						Some(Link {
							title: banner.title.clone().unwrap_or_default(),
							subtitle: None,
							image_url: banner.image2.or(banner.image1),
							value: Some(LinkValue::Manga(Manga {
								key,
								title: banner.title.unwrap_or_default(),
								..Default::default()
							})),
						})
					})
					.collect();

				if !links.is_empty() {
					send_partial_result(&HomePartialResult::Component(HomeComponent {
						title: Some(String::from(BANNER_TITLE)),
						subtitle: None,
						value: HomeComponentValue::ImageScroller {
							links,
							auto_scroll_interval: Some(5.0),
							// Pinned to the 1.85:1 ratio of `image2`. Leaving these
							// unset let the banner run off the right of the screen.
							width: Some(300),
							height: Some(162),
						},
					}));
				}

				let ranked: Vec<Link> = home
					.rank
					.map(|rank| rank.read)
					.unwrap_or_default()
					.into_iter()
					.map(|entry| Link::from(entry.into_manga()))
					.collect();

				if !ranked.is_empty() {
					send_partial_result(&HomePartialResult::Component(HomeComponent {
						title: Some(String::from(RANK_TITLE)),
						subtitle: None,
						value: HomeComponentValue::MangaList {
							ranking: true,
							page_size: None,
							entries: ranked,
							listing: Some(Listing {
								id: String::from("read"),
								name: String::from(RANK_TITLE),
								..Default::default()
							}),
						},
					}));
				}
			}
			// A failing row should not take the whole home screen down.
			Err(_) => println!("[ccc] ERROR fetching the home payload"),
		}

		Ok(HomeLayout::default())
	}
}

impl PageImageProcessor for CreativeComicSource {
	fn process_page_image(
		&self,
		response: ImageResponse,
		context: Option<PageContext>,
	) -> Result<ImageRef> {
		let context = context.ok_or_else(|| error!("頁面缺少解密參數"))?;
		let key = context
			.get("key")
			.and_then(|value| crypto::key_from_hex(value))
			.ok_or_else(|| error!("頁面缺少金鑰"))?;
		let iv = context
			.get("iv")
			.and_then(|value| crypto::iv_from_hex(value))
			.ok_or_else(|| error!("頁面缺少 iv"))?;

		// The body is AES ciphertext, so the app cannot decode it as an image and
		// hands back the untouched wire bytes here.
		let ciphertext = response.image.data();
		let image = crypto::decrypt_page(&ciphertext, &key, &iv)
			.ok_or_else(|| error!("頁面解密失敗"))?;

		Ok(ImageRef::new(&image))
	}
}

impl WebLoginHandler for CreativeComicSource {
	/// Called on every cookie update while the login page is open, so this just reports
	/// whether a token has turned up yet.
	fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		if key != "login" {
			bail!("Invalid login key: {key}");
		}
		Ok(auth::capture_web_login(&cookies))
	}
}

impl NotificationHandler for CreativeComicSource {
	fn handle_notification(&self, notification: String) {
		match notification.as_str() {
			"login" => auth::handle_login_notification(),
			// The site keeps its session in localStorage and sets no cookies, so the
			// web login callback never fires; this button reads the session across.
			"syncLogin" => {
				// The settings footer reports the reason when this fails, so there is
				// nothing to branch on here.
				let synced = auth::sync_from_web_view();
				println!("[ccc] session sync succeeded: {synced}");
			}
			// The app's own login button never flips to "log out" for this site,
			// because it tracks state through a callback CCC can never trigger.
			"clearLogin" => auth::clear(),
			_ => {}
		}
	}
}

impl DynamicSettings for CreativeComicSource {
	/// Never answers with an empty list: handing the app no dynamic settings at all
	/// leaves this source's settings screen unusable - the buttons stop responding and
	/// it reads as a freeze. `account_footer` therefore always has something to say.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let mut settings: Vec<Setting> = Vec::new();
		if let Some(footer) = auth::account_footer() {
			settings.push(
				GroupSetting {
					key: "accountInfo".into(),
					title: "帳號資訊".into(),
					footer: Some(footer.into()),
					items: Vec::new(),
					..Default::default()
				}
				.into(),
			);
		}
		Ok(settings)
	}
}

impl DeepLinkHandler for CreativeComicSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		fn leading_id(rest: &str) -> Option<String> {
			let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
			(!id.is_empty()).then_some(id)
		}

		// /book/{id}/info, /book/{id}/content, /book/{id}/donate
		if let Some(rest) = url.split("/book/").nth(1) {
			if let Some(key) = leading_id(rest) {
				return Ok(Some(DeepLinkResult::Manga { key }));
			}
		}

		// /reader_comic/{chapter id}. The URL carries no book id, so it costs one
		// lookup to find the manga this chapter belongs to.
		if let Some(rest) = url.split("/reader_comic/").nth(1) {
			if let Some(key) = leading_id(rest) {
				// The only branch that makes a request, so the credential is set up here
				// rather than at the top: without one CCC answers `403 uuid錯誤`, and a
				// deep link is exactly what a fresh install reaches first.
				auth::ensure_guest_uuid();
				let content: ChapterContent = api_get(&format!("/book/chapter/{key}"))?;
				if let Some(detail) = content.chapter {
					if detail.book > 0 {
						return Ok(Some(DeepLinkResult::Chapter {
							manga_key: format!("{}", detail.book),
							key,
						}));
					}
				}
			}
		}

		Ok(None)
	}
}

register_source!(
	CreativeComicSource,
	ListingProvider,
	Home,
	PageImageProcessor,
	WebLoginHandler,
	NotificationHandler,
	DynamicSettings,
	DeepLinkHandler
);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// Every listing id in `res/source.json` must resolve to a query, or that row of
	/// the browse screen comes back empty.
	#[aidoku_test]
	fn every_listing_id_maps_to_a_query() {
		for id in ["update", "week", "month", "read", "like", "collect"] {
			assert!(listing_query(id).is_some(), "listing {id} is unhandled");
		}
		assert!(listing_query("nope").is_none());
	}

	#[aidoku_test]
	fn the_ranking_row_reuses_a_real_listing() {
		assert!(listing_query("read").is_some());
	}

	/// Every CCC url carries a language prefix, so the parsing must survive it.
	#[aidoku_test]
	fn deep_links_survive_the_language_prefix() {
		let source = CreativeComicSource;
		for url in [
			"https://www.creative-comic.tw/zh/book/512/info",
			"https://www.creative-comic.tw/book/512/content",
			"https://www.creative-comic.tw/en/book/512/donate",
		] {
			let result = source.handle_deep_link(String::from(url)).unwrap();
			assert_eq!(
				result,
				Some(DeepLinkResult::Manga {
					key: String::from("512")
				}),
				"failed for {url}"
			);
		}
	}

	#[aidoku_test]
	fn unrelated_urls_are_ignored() {
		let source = CreativeComicSource;
		let result = source
			.handle_deep_link(String::from("https://www.creative-comic.tw/zh/about"))
			.unwrap();
		assert_eq!(result, None);
	}
}
