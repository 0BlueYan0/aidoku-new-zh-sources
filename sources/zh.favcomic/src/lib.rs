#![no_std]

use aidoku::{
	alloc::{format, vec, String, Vec},
	helpers::uri::encode_uri_component,
	imports::canvas::ImageRef,
	imports::net::Request,
	imports::std::send_partial_result,
	prelude::*,
	BasicLoginHandler, Chapter, CoverImageProcessor, DeepLinkHandler, DeepLinkResult,
	DynamicSettings, FilterValue, GroupSetting, Home, HomeComponent, HomeComponentValue,
	HomeLayout, HomePartialResult, ImageRequestProvider, ImageResponse, Link, Listing,
	ListingProvider, Manga, MangaPageResult, NotificationHandler, Page, PageContext,
	PageImageProcessor, Result, Setting, Source,
};

mod auth;
mod crypto;
mod helper;

use helper::*;

/// Listing id, display name and site path. The names must match `res/source.json` character for
/// character, and later the home screen section titles.
const SECTIONS: [(&str, &str, &str); 8] = [
	("new", "最新更新", "/new"),
	("rank", "排行榜", "/rank"),
	("free", "免費專區", "/free"),
	("boy", "少男漫畫", "/boy"),
	("girl", "少女漫畫", "/girl"),
	("r18", "成人漫畫", "/r18"),
	("webtoon", "條漫", "/webtoon"),
	("picture", "性感圖庫", "/picture"),
];

/// Sections that accept the tag/origin/status/price/sort query parameters.
const FILTERABLE_SECTIONS: [&str; 5] = ["/boy", "/girl", "/r18", "/webtoon", "/new"];

fn section_path(id: &str) -> &'static str {
	SECTIONS
		.iter()
		.find(|(listing_id, _, _)| *listing_id == id)
		.map(|(_, _, path)| *path)
		.unwrap_or("/new")
}

struct FavComicSource;

impl FavComicSource {
	/// The ranking page takes its own parameters and never paginates.
	fn ranking(&self, page: i32) -> Result<MangaPageResult> {
		if page > 1 {
			return Ok(MangaPageResult::default());
		}
		let url = format!("{}/rank?range=1&comicType=0&vip=0", base_url());
		Ok(parse_rank(&fetch_html(&url)?))
	}

	fn section(&self, path: &str, page: i32, query: &str) -> Result<MangaPageResult> {
		let url = format!("{}{path}?page={page}{query}", base_url());
		Ok(parse_listing(&fetch_html(&url)?))
	}
}

impl Source for FavComicSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let query = query.filter(|q| !q.trim().is_empty());

		// Search results carry no filter controls on the site, so a keyword wins outright.
		if let Some(keyword) = query {
			let url = format!(
				"{}/search?keyword={}&page={page}",
				base_url(),
				encode_uri_component(keyword)
			);
			return Ok(parse_listing(&fetch_html(&url)?));
		}

		let mut path = "/new";
		let mut params = String::new();
		for filter in filters {
			match filter {
				FilterValue::Select { id, value } => match id.as_str() {
					"section" => path = section_path(&value),
					// "0" is the site's own "all" option, which it expects to be omitted.
					"tag" | "origin" | "finished" | "free" | "sort" if value != "0" => {
						params.push_str(&format!("&{id}={value}"));
					}
					_ => {}
				},
				FilterValue::Text { id, value } if id == "author" => {
					let url = format!(
						"{}/search?author={}&page={page}",
						base_url(),
						encode_uri_component(value)
					);
					return Ok(parse_listing(&fetch_html(&url)?));
				}
				_ => {}
			}
		}

		// Sections that ignore the filter parameters would silently return unfiltered results.
		if !params.is_empty() && !FILTERABLE_SECTIONS.contains(&path) {
			path = "/new";
		}

		self.section(path, page, &params)
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let base = base_url();
		// Details and chapters both live on the detail page, so one request covers both.
		let document = fetch_html(&format!("{base}/comic/detail/{}", manga.key))?;

		if needs_details {
			manga.url = Some(format!("{base}/comic/detail/{}", manga.key));
			parse_detail(&document, &mut manga);
		}

		if needs_chapters {
			manga.chapters = Some(parse_chapters(&document, &base));
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let url = format!("{}/comic/chapter/{}", base_url(), chapter.key);
		let mut document = fetch_html(&url)?;

		// Code "1" means the site wants a login. If we hold credentials, the token cookie has
		// most likely lapsed, so renew it and ask once more before giving up.
		if chapter_lock_code(&document) == "1" && auth::retry_after_lock() {
			document = fetch_html(&url)?;
		}

		// Still being asked to log in while credentials are stored means the session never
		// reached the site. The reader only sees Aidoku's generic failure here; the settings
		// footer is where they find out the session is at fault, since `account_footer` says so.
		if chapter_lock_code(&document) == "1" && auth::credentials().is_some() {
			bail!("登入未生效，請到設定登出後重新登入");
		}

		parse_pages(&document)
	}
}

impl ListingProvider for FavComicSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		if listing.id == "rank" {
			return self.ranking(page);
		}
		self.section(section_path(&listing.id), page, "")
	}
}

impl Home for FavComicSource {
	fn get_home(&self) -> Result<HomeLayout> {
		// Send the empty skeleton first so the home screen lays out straight away, then stream
		// each row in as it arrives. Aidoku matches a streamed component to its skeleton slot by
		// title, so both sides read their titles from SECTIONS.
		let components = SECTIONS
			.iter()
			.map(|(_, name, _)| HomeComponent {
				title: Some(String::from(*name)),
				subtitle: None,
				value: HomeComponentValue::empty_scroller(),
			})
			.collect::<Vec<HomeComponent>>();
		send_partial_result(&HomePartialResult::Layout(HomeLayout { components }));

		for (id, name, _) in SECTIONS {
			let listing = Listing {
				id: String::from(id),
				name: String::from(name),
				..Default::default()
			};

			// One dead section must not blank the whole home screen.
			let Ok(result) = self.get_manga_list(listing.clone(), 1) else {
				continue;
			};
			if result.entries.is_empty() {
				continue;
			}

			send_partial_result(&HomePartialResult::Component(HomeComponent {
				title: Some(String::from(name)),
				subtitle: None,
				value: HomeComponentValue::Scroller {
					entries: result.entries.into_iter().map(Link::from).collect(),
					listing: Some(listing),
				},
			}));
		}

		Ok(HomeLayout::default())
	}
}

impl ImageRequestProvider for FavComicSource {
	/// Covers and pages alike need the site's Referer, and this is the single place where the
	/// reader's chosen image route is applied.
	fn get_image_request(&self, url: String, _context: Option<PageContext>) -> Result<Request> {
		let url = rewrite_image_host(&url, &preferred_image_host());
		request(&url)
	}
}

/// Decrypts an image body, or hands it back untouched if it was never encrypted.
///
/// Aidoku skips its own decoding step for sources that declare a processor, so `data()` here is
/// the raw response body. A body that *was* decodable arrives re-encoded instead, which is why
/// the magic bytes are checked before attempting decryption.
fn process(response: ImageResponse) -> Result<ImageRef> {
	let data = response.image.data();
	if data.is_empty() {
		bail!("圖片下載失敗（伺服器回應 {}）", response.code);
	}

	if crypto::is_plain_image(&data) {
		return Ok(ImageRef::new(&data));
	}

	let Some(plain) = crypto::decrypt_image(data) else {
		bail!("圖片既不是已知格式也無法解密");
	};

	// A rotated key decrypts to garbage rather than failing, so refuse to hand back a broken
	// image instead of letting the reader see noise.
	if !crypto::is_plain_image(&plain) {
		bail!("圖片解密結果不是有效圖片，網站金鑰可能已更換");
	}

	Ok(ImageRef::new(&plain))
}

impl PageImageProcessor for FavComicSource {
	fn process_page_image(
		&self,
		response: ImageResponse,
		_context: Option<PageContext>,
	) -> Result<ImageRef> {
		process(response)
	}
}

impl CoverImageProcessor for FavComicSource {
	fn process_cover_image(&self, response: ImageResponse) -> Result<ImageRef> {
		process(response)
	}
}

impl DeepLinkHandler for FavComicSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(key) = manga_key_from_href(&url) {
			return Ok(Some(DeepLinkResult::Manga { key }));
		}

		// A chapter url carries no manga id, but the reader page links back to the detail page
		// from its back button, so one fetch recovers it.
		if let Some(key) = chapter_key_from_href(&url) {
			let document = fetch_html(&format!("{}/comic/chapter/{key}", base_url()))?;
			let manga_key = document
				.select_first(".back_box")
				.and_then(|el| el.attr("onclick"))
				.and_then(|onclick| manga_key_from_href(&onclick));
			return Ok(manga_key.map(|manga_key| DeepLinkResult::Chapter { manga_key, key }));
		}

		Ok(None)
	}
}

impl BasicLoginHandler for FavComicSource {
	fn handle_basic_login(&self, _key: String, username: String, password: String) -> Result<bool> {
		Ok(auth::handle_login(&username, &password))
	}
}

impl NotificationHandler for FavComicSource {
	/// Aidoku sends the same notification for logging in and out, so `auth` tells them apart.
	/// Without this, logging out would leave the token cookie in place and the reader would
	/// still be signed in on the site.
	fn handle_notification(&self, notification: String) {
		if notification == "login" {
			auth::handle_login_notification();
		}
	}
}

impl DynamicSettings for FavComicSource {
	/// Adds an account summary under the login setting once someone is signed in.
	///
	/// The balance and quota numbers only exist on the site, and a reader who cannot see them
	/// has no way to tell a working session from a lapsed one.
	/// Never answers with an empty list: a source that hands the app no dynamic settings
	/// gets a settings screen whose buttons stop responding, which reads as the whole
	/// source having frozen. Signed out, this makes no request either - the hint is a
	/// constant, while `account_footer` fetches the account page.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let footer = if auth::hit_device_limit() {
			// The app shows the same generic failure whatever went wrong, so without
			// this a correct password reads as a wrong one.
			String::from(
				"登入失敗：這個帳號同時登入的裝置已達站方上限（實測 3 台）。\n請到喜漫網站登入，按「清除其他設備並重新登入」（需要輸入寄到 email 的 6 位驗證碼），再回來這裡登入。App 內無法解除。",
			)
		} else if auth::credentials().is_some() {
			auth::account_footer()
		} else {
			String::from("尚未登入。登入後可在這裡看到金幣、優惠券與會員狀態。")
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

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn listing_paths_resolve() {
		for (id, _, path) in SECTIONS {
			assert_eq!(section_path(id), path);
		}
		// Anything unknown falls back rather than panicking.
		assert_eq!(section_path("does-not-exist"), "/new");
	}
}

register_source!(
	FavComicSource,
	ListingProvider,
	Home,
	ImageRequestProvider,
	PageImageProcessor,
	CoverImageProcessor,
	DeepLinkHandler,
	BasicLoginHandler,
	NotificationHandler,
	DynamicSettings
);
