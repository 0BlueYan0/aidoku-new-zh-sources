#![no_std]

use aidoku::{
	alloc::{vec, String, Vec},
	imports::{canvas::ImageRef, net::Request, std::send_partial_result},
	prelude::*,
	Chapter, DeepLinkHandler, DeepLinkResult, DynamicSettings, FilterValue, GroupSetting, HashMap,
	Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult, ImageResponse, Link,
	LinkValue, Listing, ListingProvider, Manga, MangaPageResult, NotificationHandler, Page,
	PageContent, PageContext, PageImageProcessor, Result, Setting, Source, WebLoginHandler,
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
	// Book 276 「集合！RENDEZVOUS」 shares one numbered `vol_name` (`【001 打了三百年】`)
	// across five chapters whose `name` restarts at 第一話. Such a label is a story arc:
	// its number is the volume and the chapter number comes from `name`, the way the
	// official zh.bilimanga takes the volume from the volume heading and the chapter
	// from the chapter title.
	let arcs: Vec<bool> = list
		.chapters
		.iter()
		.map(|entry| is_arc(entry, &list.chapters))
		.collect();
	let labelled: Vec<(Option<f32>, Option<f32>, bool)> = list
		.chapters
		.iter()
		.zip(&arcs)
		.map(|(entry, arc)| {
			if *arc {
				arc_numbers(entry)
			} else {
				labelled_numbers(entry.vol_name.as_deref())
			}
		})
		.collect();
	// A bare number (`16647`, the book's own name on its 上線預告 entry) is only a chapter
	// number when no label in the list says `第 N 話`; otherwise it would sit far past the
	// real chapters and the tracker would jump to it.
	let any_unit = labelled.iter().any(|(_, _, has_unit)| *has_unit);
	// Labels whose bare number was dropped above: they name the book, not the chapter.
	let book_named: Vec<bool> = labelled
		.iter()
		.map(|(chapter, _, has_unit)| chapter.is_some() && !has_unit && any_unit)
		.collect();
	let parsed: Vec<(Option<f32>, Option<f32>)> = labelled
		.into_iter()
		.map(|(chapter, volume, has_unit)| (chapter.filter(|_| has_unit || !any_unit), volume))
		.collect();
	// Only when no label in the whole list has a chapter or volume number are the entries numbered by
	// their place in the oldest-first list (oldest is 1), as the official zh.jmcomic does for
	// a list without labels. In a mixed list the unnumbered entries stay `None`, so an
	// announcement never shares a number with a real chapter.
	let by_place = parsed
		.iter()
		.all(|(chapter, volume)| chapter.is_none() && volume.is_none());
	list.chapters
		.iter()
		.enumerate()
		.rev()
		.map(|(index, entry)| {
			let key = format!("{}", entry.id);
			let (chapter_number, volume_number) = parsed[index];
			let chapter_number = match (chapter_number, volume_number) {
				(None, None) if by_place => Some((index + 1) as f32),
				_ => chapter_number,
			};
			// Every arc restarts at 第一話, so the arc name is the only way to tell them apart.
			// Every arc restarts at 第一話, and an entry with no number (`特別篇`, `後記`) only
			// says what it is in its label, so the label goes in front unless the name already
			// says it (`後記`, `第二季預告`).
			let unnumbered = chapter_number.is_none() && volume_number.is_none();
			let label = entry
				.vol_name
				.as_deref()
				.map(str::trim)
				.filter(|label| !label.is_empty());
			let prefix = match label {
				Some(arc) if arcs[index] => Some(arc),
				Some(label) if unnumbered && !book_named[index] => Some(label),
				_ => None,
			};
			let title = match (prefix, chapter_title(entry)) {
				(Some(prefix), Some(title))
					if !entry.name.as_deref().unwrap_or("").contains(prefix) =>
				{
					Some(format!("{prefix} {title}"))
				}
				(Some(prefix), None) => Some(String::from(prefix)),
				(_, title) => title,
			};
			Chapter {
				url: Some(chapter_url(&key)),
				key,
				title,
				chapter_number,
				volume_number,
				locked: chapter_locked(entry),
				date_uploaded: parse_timestamp(entry.online_at.as_deref()),
				..Default::default()
			}
		})
		.collect()
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
		auth::ensure_session();
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
					if id == "author" {
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
						if value.chars().all(|c| c.is_ascii_digit()) {
							genre = Some(value);
						}
					}
					// A tapped tag. The site exposes no tag-name lookup, so this falls
					// back to full-text search, which does match tags.
					"genre" if keyword.is_none() => {
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
		auth::ensure_session();
		auth::ensure_guest_uuid();

		if needs_details {
			// Swallow failures: one unreachable book must not abort a whole library
			// refresh.
			if let Ok(book) = api_get::<Book>(&format!("/book/{}/info", manga.key)) {
				let mut fresh = book.into_manga();
				// `copy_from` takes the title even when it is empty, and an answer without
				// `name` would otherwise blank the title the library already shows.
				if fresh.title.is_empty() {
					fresh.title = core::mem::take(&mut manga.title);
				}
				manga.copy_from(fresh);
			}
			send_partial_result(&manga);
		}

		if needs_chapters {
			if let Ok(list) = api_get::<ChapterList>(&format!("/book/{}/chapter", manga.key)) {
				manga.chapters = Some(build_chapters(list));
			}
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		auth::ensure_session();
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
			// Leaving the context empty makes the failure visible as one broken
			// page rather than a dead chapter.
			if let Some((key, iv)) = unwrapped {
				context.insert(String::from("key"), crypto::to_hex(&key));
				context.insert(String::from("iv"), crypto::to_hex(&iv));
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
		auth::ensure_session();
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
		auth::ensure_session();
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

		// A failing request should not take the whole home screen down.
		if let Ok(home) = api_get::<HomeV2>("/public/home_v2") {
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
		let image =
			crypto::decrypt_page(&ciphertext, &key, &iv).ok_or_else(|| error!("頁面解密失敗"))?;

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
				auth::sync_from_web_view();
			}
			// The app's own login button never flips to "log out" for this site,
			// because it tracks state through a callback CCC can never trigger.
			"clearLogin" => auth::clear_web_session(),
			_ => {}
		}
	}
}

impl DynamicSettings for CreativeComicSource {
	/// Never answers with an empty list: handing the app no dynamic settings at all
	/// leaves this source's settings screen unusable - the buttons stop responding and
	/// it reads as a freeze. `account_footer` therefore always has something to say.
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![GroupSetting {
			key: "accountInfo".into(),
			title: "帳號資訊".into(),
			footer: Some(auth::account_footer().into()),
			items: Vec::new(),
			..Default::default()
		}
		.into()])
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
				auth::ensure_session();
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

	fn list_of(labels: &[&str]) -> ChapterList {
		ChapterList {
			chapters: labels
				.iter()
				.enumerate()
				.map(|(index, label)| ChapterEntry {
					id: index as i64 + 1,
					vol_name: Some(String::from(*label)),
					..Default::default()
				})
				.collect(),
		}
	}

	/// A list where no label has a number is numbered by place, oldest first.
	#[aidoku_test]
	fn a_list_without_any_number_is_numbered_by_place() {
		let chapters = build_chapters(list_of(&["序", "番外", "後記"]));
		let numbers: Vec<Option<f32>> = chapters.iter().map(|c| c.chapter_number).collect();
		assert_eq!(numbers, [Some(3.0), Some(2.0), Some(1.0)]);
	}

	/// In a mixed list the unnumbered entries stay `None`.
	#[aidoku_test]
	fn unnumbered_entries_stay_none_in_a_mixed_list() {
		let chapters = build_chapters(list_of(&["序", "第 1 話", "公告", "第 2 話"]));
		let numbers: Vec<Option<f32>> = chapters.iter().map(|c| c.chapter_number).collect();
		assert_eq!(numbers, [Some(2.0), None, Some(1.0), None]);
	}

	/// Book 341 「16647」 labels its 上線預告 with the book's name. Next to `第 N 話` labels
	/// that bare number is not a chapter; a list of bare numbers alone still is.
	#[aidoku_test]
	fn a_bare_number_is_not_a_chapter_next_to_labelled_ones() {
		let chapters = build_chapters(list_of(&["16647", "第 1 話", "第 2 話", "後記"]));
		let numbers: Vec<Option<f32>> = chapters.iter().map(|c| c.chapter_number).collect();
		assert_eq!(numbers, [None, Some(2.0), Some(1.0), None]);

		let chapters = build_chapters(list_of(&["1", "2", "3"]));
		let numbers: Vec<Option<f32>> = chapters.iter().map(|c| c.chapter_number).collect();
		assert_eq!(numbers, [Some(3.0), Some(2.0), Some(1.0)]);

		let chapters = build_chapters(list_of(&["16647", "Ep 1", "Ep 2"]));
		let numbers: Vec<Option<f32>> = chapters.iter().map(|c| c.chapter_number).collect();
		assert_eq!(numbers, [Some(2.0), Some(1.0), None]);
	}

	/// A volume's appendix has neither number; the volume itself keeps its volume number.
	#[aidoku_test]
	fn volume_appendix_has_no_numbers_but_the_volume_does() {
		assert_eq!(numbers(Some("第3卷")), (None, Some(3.0)));
		assert_eq!(numbers(Some("第3卷 後記")), (None, None));
		assert_eq!(numbers(Some("01卷後記")), (None, None));
		assert_eq!(numbers(Some("3卷卷首")), (None, None));
		assert_eq!(numbers(Some("06卷番外")), (None, None));
		assert_eq!(numbers(Some("第3卷 新的旅程")), (None, None));
	}

	/// Volumes count as numbers: an extra next to them stays `None` instead of taking a place.
	#[aidoku_test]
	fn a_list_of_volumes_and_an_extra_is_not_numbered_by_place() {
		let chapters = build_chapters(list_of(&["第1卷", "第2卷", "番外"]));
		let pairs: Vec<(Option<f32>, Option<f32>)> = chapters
			.iter()
			.map(|c| (c.chapter_number, c.volume_number))
			.collect();
		assert_eq!(pairs, [(None, None), (None, Some(2.0)), (None, Some(1.0))]);
	}

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
