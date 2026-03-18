#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{
		net::Request,
		std::{current_date, send_partial_result},
	},
	prelude::*,
	Chapter, ContentRating, DeepLinkHandler, DeepLinkResult, FilterValue,
	Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult,
	ImageRequestProvider, Link, Listing, ListingProvider, Manga, MangaPageResult,
	MangaStatus, Page, PageContent, PageContext, Result, Source, Viewer,
};

mod helper;
use helper::*;

const BASE_URL: &str = "https://www.webtoons.com";
const LANG_PATH: &str = "/zh-hant";
const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

/// Webtoons mobile API base URL for fetching all episodes in one request.
const MOBILE_API: &str = "https://m.webtoons.com/api/v1/webtoon";

struct WebtoonSource;

fn with_cache_bust(url: &str) -> String {
	let ts = current_date() as i64;
	if url.contains('?') {
		format!("{url}&_={ts}")
	} else {
		format!("{url}?_={ts}")
	}
}

/// Helper: fetch a page and parse manga items.
fn fetch_manga_list(url: &str) -> Result<(Vec<Manga>, bool)> {
	let request_url = with_cache_bust(url);
	let html = Request::get(&request_url)?
		.header("Referer", BASE_URL)
		.header("User-Agent", USER_AGENT)
		.header("Cache-Control", "no-cache, no-store, must-revalidate")
		.header("Pragma", "no-cache")
		.html()?;

	let mut entries: Vec<Manga> = Vec::new();

	if let Some(list) = html.select("ul.webtoon_list li a.link") {
		for item in list {
			if let Some(manga) = parse_manga_item(&item) {
				entries.push(manga);
			}
		}
	}

	let has_next_page = html.select_first(".pg_next").is_some();
	Ok((entries, has_next_page))
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
			match filter {
				FilterValue::Select { id, value } => {
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
				_ => {}
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
			let html = Request::get(&detail_url)?
				.header("Referer", BASE_URL)
				.header("User-Agent", USER_AGENT)
				.header("Cache-Control", "no-cache, no-store, must-revalidate")
				.header("Pragma", "no-cache")
				.html()?;

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
			manga.viewer = Viewer::Webtoon;
			println!("[webtoons] details fetched OK for title_no={}", title_no);
		}

		if needs_chapters {
			// Use Webtoons mobile API to get ALL chapters in one request.
			// Endpoint: m.webtoons.com/api/v1/webtoon/{titleId}/episodes?pageSize=99999
			let api_url = format!(
				"{MOBILE_API}/{title_no}/episodes?pageSize=99999"
			);
			let api_url = with_cache_bust(&api_url);

			println!("[webtoons] fetching chapters: {}", api_url);

			// Use graceful error handling: if the API call fails, log the error
			// but still return Ok(manga) so library refresh doesn't silently break.
			let fetch_result = (|| -> Result<Vec<Chapter>> {
				let body = Request::get(&api_url)?
					.header("Referer", BASE_URL)
					.header("User-Agent", USER_AGENT)
					.header("Cache-Control", "no-cache, no-store, must-revalidate")
					.header("Pragma", "no-cache")
					.string()?;
				Ok(parse_episodes_json(&body))
			})();

			match fetch_result {
				Ok(chapters) => {
					println!(
						"[webtoons] chapters fetched OK: {} chapters for title_no={}",
						chapters.len(),
						title_no
					);
					manga.chapters = Some(chapters);
				}
				Err(_) => {
					println!(
						"[webtoons] ERROR fetching chapters for title_no={}",
						title_no
					);
					// Don't propagate the error — return the manga as-is
					// so the library refresh can continue with other manga.
				}
			}
		}

		println!("[webtoons] get_manga_update done for title_no={}", title_no);
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let viewer_url = if let Some(ref url) = chapter.url {
			url.clone()
		} else {
			chapter.key.clone()
		};

		let viewer_url = with_cache_bust(&viewer_url);
		let html = Request::get(&viewer_url)?
			.header("Referer", BASE_URL)
			.header("User-Agent", USER_AGENT)
			.header("Cache-Control", "no-cache, no-store, must-revalidate")
			.header("Pragma", "no-cache")
			.html()?;

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

		Ok(pages)
	}
}

impl ListingProvider for WebtoonSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let url = match listing.id.as_str() {
			"popular" => format!(
				"{BASE_URL}{LANG_PATH}/ranking?sortOrder=MANA&page={page}"
			),
			day @ ("monday" | "tuesday" | "wednesday" | "thursday"
				| "friday" | "saturday" | "sunday" | "complete") =>
			{
				if page > 1 {
					return Ok(MangaPageResult {
						entries: Vec::new(),
						has_next_page: false,
					});
				}
				format!(
					"{BASE_URL}{LANG_PATH}/originals/{day}?sortOrder=MANA"
				)
			}
			_ => bail!("Unknown listing: {}", listing.id),
		};

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

		for (id, name) in sections {
			if let Ok(result) = self.get_manga_list(
				Listing {
					id: String::from(id),
					name: String::from(name),
					..Default::default()
				},
				1,
			) {
				if !result.entries.is_empty() {
					let links: Vec<Link> = result.entries.into_iter().map(Link::from).collect();
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
		}

		Ok(HomeLayout::default())
	}
}

impl ImageRequestProvider for WebtoonSource {
	fn get_image_request(
		&self,
		url: String,
		_context: Option<PageContext>,
	) -> Result<Request> {
		let request = Request::get(&url)?
			.header("Referer", "https://www.webtoons.com");
		Ok(request)
	}
}

impl DeepLinkHandler for WebtoonSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		if let Some(title_no) = extract_title_no(&url) {
			if url.contains("/viewer") {
				Ok(Some(DeepLinkResult::Chapter {
					manga_key: title_no,
					key: url,
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
	DeepLinkHandler
);
