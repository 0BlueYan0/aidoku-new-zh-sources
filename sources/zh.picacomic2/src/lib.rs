#![no_std]

use aidoku::{
	alloc::{String, Vec},
	prelude::*,
	BasicLoginHandler, Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue,
	GroupSetting, Listing, ListingProvider, Manga, MangaPageResult, NotificationHandler, Page,
	Result, Setting, Source,
};

mod auth;
mod crypto;
mod home;
mod json;
mod net;
mod punch;
mod settings;

use net::Url;

struct Picacomic2;

fn explore_page(url: Url) -> Result<MangaPageResult> {
	let response: json::ExploreResponse = net::request_json(url)?;
	Ok(response
		.data
		.into_page(&settings::get_api_line(), &settings::get_blocklist()))
}

fn rank_page(url: Url) -> Result<MangaPageResult> {
	let response: json::RankResponse = net::request_json(url)?;
	Ok(response
		.data
		.into_page(&settings::get_api_line(), &settings::get_blocklist()))
}

impl Source for Picacomic2 {
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
		punch::punch_if_due();
		// Keyword search answers with the same shape as the plain lists.
		explore_page(Url::from_query_or_filters(query.as_deref(), page, &filters))
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		auth::ensure_session();
		punch::punch_if_due();

		if needs_details {
			let response: json::ComicResponse = net::request_json(Url::Manga {
				id: manga.key.clone(),
			})?;
			let comic = response.data.comic.into_manga(&settings::get_api_line());
			manga = Manga {
				chapters: manga.chapters,
				..comic
			};
		}

		if needs_chapters {
			// Any page that fails fails the whole list, so a library refresh never stores a
			// chapter list with a gap.
			let mut chapters: Vec<Chapter> = Vec::new();
			let mut page = 1;
			loop {
				let response: json::ChapterResponse = net::request_json(Url::ChapterList {
					id: manga.key.clone(),
					page,
				})?;
				let total = response.data.eps.pages;
				chapters.extend(
					response
						.data
						.eps
						.docs
						.into_iter()
						.map(json::ChapterItem::into_chapter),
				);
				if page >= total {
					break;
				}
				page += 1;
			}
			manga.chapters = Some(chapters);
		}

		Ok(manga)
	}

	fn get_page_list(&self, manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		auth::ensure_session();
		punch::punch_if_due();

		let line = settings::get_api_line();
		let mut pages: Vec<Page> = Vec::new();
		let mut page = 1;
		loop {
			let response: json::PageResponse = net::request_json(Url::PageList {
				manga_id: manga.key.clone(),
				chapter_id: chapter.key.clone(),
				page,
			})?;
			let total = response.data.pages.pages;
			pages.extend(
				response
					.data
					.pages
					.docs
					.into_iter()
					.map(|item| item.into_page(&line)),
			);
			if page >= total {
				break;
			}
			page += 1;
		}
		if pages.is_empty() {
			bail!("[picacomic2] the chapter has no pages");
		}
		Ok(pages)
	}
}

impl ListingProvider for Picacomic2 {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		auth::ensure_session();
		punch::punch_if_due();

		match listing.id.as_str() {
			"update" => explore_page(Url::latest(page)),
			"dayup" => rank_page(Url::Rank { time: "H24" }),
			"weekup" => rank_page(Url::Rank { time: "D7" }),
			"monthup" => rank_page(Url::Rank { time: "D30" }),
			"random" => rank_page(Url::Random),
			"favourite" => explore_page(Url::Favourite {
				sort: String::from("dd"),
				page,
			}),
			"djkz" => explore_page(Url::explore_category("大家都在看", page)),
			"gfdjkz" => explore_page(Url::explore_category("官方都在看", page)),
			"dswj" => explore_page(Url::explore_category("大濕推薦", page)),
			"nndtn" => explore_page(Url::explore_category("那年今天", page)),
			other => bail!("[picacomic2] unknown listing `{other}`"),
		}
	}
}

/// The work id in a PicaWeb work link (`https://picawang.com/comic/<id>`) or an official web
/// link (`https://manhuabika.com/pcomicview/?cid=<id>`).
fn comic_id(url: &str) -> Option<String> {
	let rest = url.strip_prefix("https://")?;
	let (host, path) = rest.split_once('/')?;
	let host = host.strip_prefix("www.").unwrap_or(host);
	let id = match host {
		"picawang.com" => path.strip_prefix("comic/")?,
		"manhuabika.com" => path.split_once("cid=")?.1,
		_ => return None,
	};
	let id = id.split(['/', '?', '&', '#']).next()?;
	if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
		return None;
	}
	Some(String::from(id))
}

impl DeepLinkHandler for Picacomic2 {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		Ok(comic_id(&url).map(|key| DeepLinkResult::Manga { key }))
	}
}

impl BasicLoginHandler for Picacomic2 {
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
				bail!("[picacomic2] the sign-in request got no usable answer")
			}
		}
	}
}

impl NotificationHandler for Picacomic2 {
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

impl DynamicSettings for Picacomic2 {
	/// Never answers with an empty list: a source that hands the app an empty list of dynamic
	/// settings gets a settings screen whose buttons stop responding. Every path of
	/// `account_footer` is a non-empty text, and only a stored token makes it send a request.
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
	Picacomic2,
	Home,
	ListingProvider,
	DeepLinkHandler,
	BasicLoginHandler,
	NotificationHandler,
	DynamicSettings
);

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn deep_links() {
		let id = "677ea24f4603e87c1d3d8a95";
		assert_eq!(
			comic_id(&format!("https://picawang.com/comic/{id}")).as_deref(),
			Some(id)
		);
		assert_eq!(
			comic_id(&format!("https://www.picawang.com/comic/{id}?from=share")).as_deref(),
			Some(id)
		);
		assert_eq!(
			comic_id(&format!("https://manhuabika.com/pcomicview/?cid={id}")).as_deref(),
			Some(id)
		);
		assert_eq!(
			comic_id(&format!("https://manhuabika.com/pcomicview/?cid={id}&x=1")).as_deref(),
			Some(id)
		);
		assert_eq!(comic_id("https://picawang.com/"), None);
		assert_eq!(comic_id("https://picawang.com/comic/"), None);
		assert_eq!(comic_id("https://manhuabika.com/pcomicview/"), None);
		assert_eq!(comic_id("https://example.com/comic/abc"), None);
		assert_eq!(comic_id("not a url"), None);
	}
}
