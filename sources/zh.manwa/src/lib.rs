#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{canvas::ImageRef, std::send_partial_result},
	prelude::*,
	BasicLoginHandler, Chapter, CoverImageProcessor, DeepLinkHandler, DeepLinkResult,
	DynamicSettings, FilterValue, GroupSetting, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, ImageResponse, Link, Listing, ListingProvider, Manga,
	MangaPageResult, NotificationHandler, Page, PageContext, PageImageProcessor, Result, Setting,
	Source,
};

mod auth;
mod crypto;
mod helper;

use helper::*;

/// The home rows in the site's order. The titles are the site's own section names.
const HOT: &str = "熱門漫畫";
const COMPLETE: &str = "最新完整";
const VIP: &str = "VIP 專區";
const LATEST: &str = "最新更新";
const FAVORITE: &str = "熱門收藏";

/// The listing each row's 「更多」 opens: (id, name), as in `res/source.json`. The site's
/// own 「更多」 links for the non-VIP rows are categories that come back empty, so each row
/// uses the listing that holds its works (2026-11-05: the hot row's 6 were all in the top 17 of 人氣榜, the
/// latest row's in the top 6 of 最近更新 bar one, the favorite row's in the top 14 of 熱門,
/// the VIP row's the top 6 of VIP 專區). The complete row is the top of tag 完整版, which
/// is not a listing (the user chose to leave that row without one).
const HOT_LISTING: (&str, &str) = ("rank_hot", "人氣榜");
const VIP_LISTING: (&str, &str) = ("vip", "VIP 專區");
const LATEST_LISTING: (&str, &str) = ("update", "最近更新");
const FAVORITE_LISTING: (&str, &str) = ("hot", "熱門");

fn listing((id, name): (&str, &str)) -> Option<Listing> {
	Some(Listing {
		id: String::from(id),
		name: String::from(name),
		..Default::default()
	})
}

fn component(title: &str, value: HomeComponentValue) -> HomeComponent {
	HomeComponent {
		title: Some(String::from(title)),
		subtitle: None,
		value,
	}
}

fn links(entries: Vec<Manga>) -> Vec<Link> {
	entries.into_iter().map(Link::from).collect()
}

struct ManwaSource;

impl Source for ManwaSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		auth::ensure_session();

		let mut author: Option<String> = None;
		let mut browse = Browse::default();
		for filter in filters {
			match filter {
				// A tapped author.
				FilterValue::Text { id, value } if id == "author" => author = Some(value),
				// `filters.json` supplies `ids`, so `value` is the site's own value. The
				// app only sends a select that is not at its first option. A tapped tag
				// arrives as `genre` with the tag's name, which the site reads as it is.
				FilterValue::Select { id, value } => match id.as_str() {
					"sort" => browse.sort = Some(value),
					"status" => browse.status = Some(value),
					"type" => browse.kind = Some(value),
					"area" => browse.area = Some(value),
					"level" => browse.level = Some(value),
					"day" => browse.day = Some(value),
					// A tapped tag: a single tag.
					"genre" => browse.tags = Vec::from([value]),
					_ => {}
				},
				FilterValue::MultiSelect { id, included, .. } if id == "tags" => {
					browse.tags = included;
				}
				_ => {}
			}
		}

		// The search API ignores the browse conditions, so they are hidden while a keyword
		// is typed (`hidesFiltersWhileSearching`).
		let keyword = query.filter(|q: &String| !q.trim().is_empty());
		let author = author.filter(|a: &String| !a.trim().is_empty());
		if keyword.is_some() || author.is_some() {
			return fetch_list(
				search_request(keyword.as_deref(), author.as_deref(), page)?,
				"/api/search",
				page,
				SEARCH_PAGE_SIZE,
				// A keyword is searched as typed; only a tapped author is narrowed.
				if keyword.is_none() {
					author.as_deref()
				} else {
					None
				},
			);
		}
		fetch_list(
			cate_request(&browse, page)?,
			"/api/cate",
			page,
			CATE_PAGE_SIZE,
			None,
		)
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		auth::ensure_session();

		// One page holds both the details and every chapter.
		let html = fetch_details_page(&manga.key)?;
		if needs_details {
			let Some(details) = parse_details(&html, &manga.key) else {
				bail!("[manwa] no title on the details page of {}", manga.key);
			};
			let chapters = manga.chapters.take();
			manga = details;
			manga.chapters = chapters;
			send_partial_result(&manga);
		}
		if needs_chapters {
			// A page with no chapters leaves the library's chapters as they are.
			if let Some(chapters) = parse_chapters(&html, &manga.key, auth::is_vip())? {
				manga.chapters = Some(chapters);
			}
		}
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		auth::ensure_session();
		Ok(pages_from_urls(fetch_page_urls(&chapter.key)?))
	}
}

impl ListingProvider for ManwaSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		auth::ensure_session();
		let Some(request) = listing_request(&listing.id, page) else {
			bail!("Unknown listing: {}", listing.id);
		};
		let page_size = if listing.id.starts_with("rank_") {
			RANK_PAGE_SIZE
		} else {
			CATE_PAGE_SIZE
		};
		fetch_list(request?, &listing.id, page, page_size, None)
	}
}

impl Home for ManwaSource {
	fn get_home(&self) -> Result<HomeLayout> {
		auth::ensure_session();

		// Send an empty skeleton first so the home screen lays out immediately.
		send_partial_result(&HomePartialResult::Layout(HomeLayout {
			components: Vec::from([
				component(HOT, HomeComponentValue::empty_scroller()),
				component(COMPLETE, HomeComponentValue::empty_scroller()),
				component(VIP, HomeComponentValue::empty_scroller()),
				component(LATEST, HomeComponentValue::empty_scroller()),
				component(FAVORITE, HomeComponentValue::empty_scroller()),
			]),
		}));

		// One request returns every row.
		let data = read_home(home_request()?.send()?)?;
		let rows = [
			(HOT, data.comic_list, listing(HOT_LISTING)),
			(COMPLETE, data.gufeng_list, None),
			(VIP, data.vip_list, listing(VIP_LISTING)),
			(LATEST, data.xuanhuan_list, listing(LATEST_LISTING)),
			(FAVORITE, data.xiaoyuan_list, listing(FAVORITE_LISTING)),
		];
		for (title, entries, listing) in rows {
			let entries = home_entries(entries);
			// A row with nothing in it keeps its skeleton.
			if entries.is_empty() {
				continue;
			}
			send_partial_result(&HomePartialResult::Component(component(
				title,
				HomeComponentValue::Scroller {
					entries: links(entries),
					listing,
				},
			)));
		}

		Ok(HomeLayout::default())
	}
}

/// Decrypts an image body, or hands it back untouched if it was never encrypted.
///
/// Aidoku skips its own decoding step for sources that declare a processor, so `data()`
/// here is the raw response body. A body that was decodable arrives re-encoded instead,
/// which is why the magic bytes are checked before attempting decryption.
fn process(response: ImageResponse) -> Result<ImageRef> {
	let data = response.image.data();
	if data.is_empty() {
		bail!("[manwa] image download failed (HTTP {})", response.code);
	}

	if crypto::is_plain_image(&data) {
		return Ok(ImageRef::new(&data));
	}

	let Some(plain) = crypto::decrypt_image(data) else {
		bail!("[manwa] the image is neither a known format nor decryptable");
	};

	// A rotated key decrypts to noise rather than failing, so refuse to hand back a
	// broken image.
	if !crypto::is_plain_image(&plain) {
		bail!("[manwa] decrypted data is not an image, the site key may have changed");
	}

	Ok(ImageRef::new(&plain))
}

impl PageImageProcessor for ManwaSource {
	fn process_page_image(
		&self,
		response: ImageResponse,
		_context: Option<PageContext>,
	) -> Result<ImageRef> {
		process(response)
	}
}

impl CoverImageProcessor for ManwaSource {
	fn process_cover_image(&self, response: ImageResponse) -> Result<ImageRef> {
		process(response)
	}
}

impl DeepLinkHandler for ManwaSource {
	/// Any mirror's `/comic/<id>` or `/comic/<id>/<chapter>`.
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		Ok(comic_path(&url).map(|(manga_key, chapter)| match chapter {
			Some(key) => DeepLinkResult::Chapter { manga_key, key },
			None => DeepLinkResult::Manga { key: manga_key },
		}))
	}
}

impl BasicLoginHandler for ManwaSource {
	fn handle_basic_login(&self, _key: String, username: String, password: String) -> Result<bool> {
		match auth::login(&username, &password) {
			auth::LoginOutcome::Token(token) => {
				// The credentials are kept so a token the site has dropped can be renewed
				// without asking again.
				auth::save_login(&token, &username, &password);
				Ok(true)
			}
			auth::LoginOutcome::Rejected => {
				auth::record_login_failure(auth::REJECTED_TEXT);
				Ok(false)
			}
			auth::LoginOutcome::Unreachable => {
				auth::record_login_failure(auth::UNREACHABLE_TEXT);
				bail!("[manwa] the login request got no usable answer")
			}
		}
	}
}

impl NotificationHandler for ManwaSource {
	fn handle_notification(&self, notification: String) {
		if notification == "login" {
			// The app fires this for both logging in and logging out. A recent login
			// timestamp marks the one that follows a successful login; anything else is a
			// logout and must wipe the stored credentials, or the silent renewal would log
			// the user straight back in.
			if auth::is_just_logged_in() {
				auth::clear_just_logged_in();
			} else {
				auth::clear_auth();
			}
		}
	}
}

impl DynamicSettings for ManwaSource {
	/// Never answers with an empty list: a source that hands the app an empty list of dynamic
	/// settings gets a settings screen whose buttons stop responding. Every path of
	/// `account_footer` (needs a new login, signed out, signed in) is a non-empty text, and
	/// none of them sends a request.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(Vec::from([GroupSetting {
			key: "accountInfo".into(),
			title: "帳號資訊".into(),
			items: Vec::new(),
			footer: Some(auth::account_footer().into()),
			..Default::default()
		}
		.into()]))
	}
}

register_source!(
	ManwaSource,
	ListingProvider,
	Home,
	BasicLoginHandler,
	NotificationHandler,
	DynamicSettings,
	PageImageProcessor,
	CoverImageProcessor,
	DeepLinkHandler
);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_names_match_source_json() {
		// A Windows checkout may turn the file's line endings into CRLF.
		let json = include_str!("../res/source.json").replace("\r\n", "\n");
		for (id, name) in [HOT_LISTING, VIP_LISTING, LATEST_LISTING, FAVORITE_LISTING] {
			assert!(
				json.contains(&format!("\"id\": \"{id}\",\n\t\t\t\"name\": \"{name}\"")),
				"{id}"
			);
		}
		for id in [
			"update", "new", "hot", "vip", "rank_hot", "rank_end", "rank_new",
		] {
			assert!(json.contains(&format!("\"id\": \"{id}\"")), "{id}");
			assert!(listing_request(id, 1).is_some(), "{id}");
		}
		assert_eq!(json.matches("\"id\": ").count(), 8);
		assert!(listing_request("nope", 1).is_none());
	}

	#[aidoku_test]
	fn links_open_without_a_request() {
		let source = ManwaSource::new();
		assert_eq!(
			source
				.handle_deep_link(String::from("https://manwacn.cc/comic/94789"))
				.unwrap(),
			Some(DeepLinkResult::Manga {
				key: String::from("94789")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://mwmwbu.cc/comic/94789/1361313"))
				.unwrap(),
			Some(DeepLinkResult::Chapter {
				manga_key: String::from("94789"),
				key: String::from("1361313")
			})
		);
		assert_eq!(
			source
				.handle_deep_link(String::from("https://manwacn.cc/cate/bl"))
				.unwrap(),
			None
		);
	}
}
