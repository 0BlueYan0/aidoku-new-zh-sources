#![no_std]

use aidoku::{
	alloc::{String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
	},
	prelude::*,
	BasicLoginHandler, Chapter, ContentRating, DeepLinkHandler, DeepLinkResult,
	FilterValue, Home, HomeComponent, HomeComponentValue, HomeLayout, ImageRequestProvider, Link,
	Listing, ListingProvider, Manga, MangaPageResult, MangaStatus, Page,
	PageContent, PageContext, Result, Source, Viewer,
};

mod helper;
use helper::*;

const BASE_URL: &str = "https://komiic.com";
const LOGIN_URL: &str = "https://komiic.com/api/login";

struct KomiicSource;

/// Retrieve the stored auth token from defaults.
fn get_token() -> Option<String> {
	defaults_get::<String>("auth_token")
}

/// Helper to send a GraphQL request with the stored auth token.
fn gql(query: &str, variables: &str) -> Result<String> {
	let token = get_token();
	graphql_request(query, variables, token.as_deref())
}

/// Parse a list of comics from a GraphQL response.
/// `data_key` is the top-level data field name (e.g. "recentUpdate", "hotComics").
fn parse_comic_list(body: &str, data_key: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();

	// Try to find the data field as an array
	if let Some(array_str) = json_data_field(body, data_key) {
		let objects = json_top_level_objects(array_str);
		for obj in objects {
			if let Some(manga) = parse_comic(obj) {
				entries.push(manga);
			}
		}
	}

	entries
}

impl Source for KomiicSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		// Keyword search
		if let Some(keyword) = query {
			if !keyword.is_empty() {
				let vars = format!(r#"{{"keyword":"{}"}}"#, keyword);
				let body = gql(SEARCH_QUERY, &vars)?;

				// Parse comics from searchComicsAndAuthors.comics
				let mut entries: Vec<Manga> = Vec::new();
				if let Some(result_obj) = json_data_field(&body, "searchComicsAndAuthors") {
					if let Some(comics_arr) = json_data_field(result_obj, "comics") {
						let objects = json_top_level_objects(comics_arr);
						for obj in objects {
							if let Some(manga) = parse_comic(obj) {
								entries.push(manga);
							}
						}
					}
				}

				return Ok(MangaPageResult {
					entries,
					has_next_page: false,
				});
			}
		}

		// Filter-based browsing
		let mut category_id = String::new();
		let mut order_by = String::from("DATE_UPDATED");
		let mut status_filter = String::new();

		for filter in filters {
			match filter {
				FilterValue::Select { id, value } => {
					if id == "category" {
						category_id = value;
					} else if id == "sort" {
						order_by = match value.as_str() {
							"最新更新" => String::from("DATE_UPDATED"),
							"本月觀看" => String::from("MONTH_VIEWS"),
							"總觀看數" => String::from("VIEWS"),
							"收藏人數" => String::from("FAVORITE_COUNT"),
							_ => String::from("DATE_UPDATED"),
						};
					} else if id == "status" {
						status_filter = match value.as_str() {
							"連載中" => String::from("ONGOING"),
							"已完結" => String::from("END"),
							_ => String::new(),
						};
					}
				}
				_ => {}
			}
		}

		let limit = 20;
		let offset = (page - 1) * limit;

		if !category_id.is_empty() && category_id != "all" {
			// Browse by category
			let vars = format!(
				r#"{{"categoryId":"{}","pagination":{{"limit":{},"offset":{},"orderBy":"{}","status":"{}","asc":true}}}}"#,
				category_id, limit, offset, order_by, status_filter
			);
			let body = gql(COMICS_BY_CATEGORY_QUERY, &vars)?;
			let entries = parse_comic_list(&body, "comicByCategory");
			let has_next_page = entries.len() == limit as usize;

			Ok(MangaPageResult {
				entries,
				has_next_page,
			})
		} else {
			// Default: recent update listing
			let vars = format!(
				r#"{{"pagination":{{"limit":{},"offset":{},"orderBy":"{}","status":"{}","asc":true}}}}"#,
				limit, offset, order_by, status_filter
			);
			let body = gql(RECENT_UPDATE_QUERY, &vars)?;
			let entries = parse_comic_list(&body, "recentUpdate");
			let has_next_page = entries.len() == limit as usize;

			Ok(MangaPageResult {
				entries,
				has_next_page,
			})
		}
	}

	fn get_manga_update(
		&self,
		mut manga: Manga,
		needs_details: bool,
		needs_chapters: bool,
	) -> Result<Manga> {
		let comic_id = manga.key.clone();

		if needs_details {
			let vars = format!(r#"{{"comicId":"{}"}}"#, comic_id);
			let body = gql(COMIC_BY_ID_QUERY, &vars)?;

			if let Some(comic_obj) = json_data_field(&body, "comicById") {
				if let Some(title) = json_str_value(comic_obj, "title") {
					manga.title = String::from(title);
				}

				if let Some(image_url) = json_str_value(comic_obj, "imageUrl") {
					manga.cover = Some(String::from(image_url));
				}

				let status_str = json_str_value(comic_obj, "status").unwrap_or("UNKNOWN");
				manga.status = match status_str {
					"ONGOING" => MangaStatus::Ongoing,
					"END" | "COMPLETED" => MangaStatus::Completed,
					_ => MangaStatus::Unknown,
				};

				// Authors
				let authors_objs = json_array_objects(comic_obj, "authors");
				let authors: Vec<String> = authors_objs
					.iter()
					.filter_map(|a| json_str_value(a, "name").map(String::from))
					.collect();
				if !authors.is_empty() {
					manga.authors = Some(authors);
				}

				// Categories as tags
				let category_objs = json_array_objects(comic_obj, "categories");
				let tags: Vec<String> = category_objs
					.iter()
					.filter_map(|c| json_str_value(c, "name").map(String::from))
					.collect();
				if !tags.is_empty() {
					manga.tags = Some(tags);
				}

				let is_warning = comic_obj.contains("\"isWarning\":true") 
					|| comic_obj.contains("\"isWarning\": true");
				
				manga.content_rating = if is_warning {
					ContentRating::NSFW
				} else {
					let mut is_suggestive = false;
					if let Some(ref t) = manga.tags {
						for tag in t {
							if ["後宮", "BL", "百合", "耽美", "血腥", "暴力", "獵奇"].contains(&tag.as_str()) {
								is_suggestive = true;
								break;
							}
						}
					}
					if is_suggestive {
						ContentRating::Suggestive
					} else {
						ContentRating::Safe
					}
				};

				manga.url = Some(format!("{BASE_URL}/comic/{comic_id}"));
				manga.viewer = Viewer::Webtoon;
			}
		}

		if needs_chapters {
			let vars = format!(r#"{{"comicId":"{}"}}"#, comic_id);
			let body = gql(CHAPTERS_QUERY, &vars)?;

			let mut chapters: Vec<Chapter> = Vec::new();

			if let Some(chapters_arr) = json_data_field(&body, "chaptersByComicId") {
				let objects = json_top_level_objects(chapters_arr);
				for obj in objects {
					if let Some(chapter) = parse_chapter(obj) {
						chapters.push(chapter);
					}
				}
			}

			// Reverse to show newest first
			chapters.reverse();
			manga.chapters = Some(chapters);
		}

		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
		let chapter_id = chapter.key.clone();

		// Step 1: Get image metadata (kids) for this chapter
		let vars = format!(r#"{{"chapterId":"{}"}}"#, chapter_id);
		let body = gql(IMAGES_QUERY, &vars)?;

		let mut pages: Vec<Page> = Vec::new();

		if let Some(images_arr) = json_data_field(&body, "imagesByChapterId") {
			let objects = json_top_level_objects(images_arr);
			for obj in objects {
				if let Some(kid) = json_str_value(obj, "kid") {
					// Encode kid into a custom scheme to be intercepted lazily in get_image_request
					let url = format!("komiickid://{}", kid);
					pages.push(Page {
						content: PageContent::url(url),
						..Default::default()
					});
				}
			}
		}

		Ok(pages)
	}
}

impl ListingProvider for KomiicSource {
	fn get_manga_list(&self, listing: Listing, page: i32) -> Result<MangaPageResult> {
		let limit = 20;
		let offset = (page - 1) * limit;

		let (query, data_key, order_by) = match listing.id.as_str() {
			"hot" => (HOT_COMICS_QUERY, "hotComics", "MONTH_VIEWS"),
			_ => (RECENT_UPDATE_QUERY, "recentUpdate", "DATE_UPDATED"),
		};

		let vars = format!(
			r#"{{"pagination":{{"limit":{},"offset":{},"orderBy":"{}","status":"","asc":true}}}}"#,
			limit, offset, order_by
		);

		let body = gql(query, &vars)?;
		let entries = parse_comic_list(&body, data_key);
		let has_next_page = entries.len() == limit as usize;

		Ok(MangaPageResult {
			entries,
			has_next_page,
		})
	}
}

impl Home for KomiicSource {
	fn get_home(&self) -> Result<HomeLayout> {
		let mut components: Vec<HomeComponent> = Vec::new();

		// Recent Updates
		let recent_vars =
			r#"{"pagination":{"limit":20,"offset":0,"orderBy":"DATE_UPDATED","status":"","asc":true}}"#;
		if let Ok(body) = gql(RECENT_UPDATE_QUERY, recent_vars) {
			let entries = parse_comic_list(&body, "recentUpdate");
			let links: Vec<Link> = entries.into_iter().map(Link::from).collect();

			if !links.is_empty() {
				components.push(HomeComponent {
					title: Some(String::from("最近更新")),
					subtitle: None,
					value: HomeComponentValue::Scroller {
						entries: links,
						listing: Some(Listing {
							id: String::from("recent"),
							name: String::from("最近更新"),
							..Default::default()
						}),
					},
				});
			}
		}

		// Hot Comics
		let hot_vars =
			r#"{"pagination":{"limit":20,"offset":0,"orderBy":"MONTH_VIEWS","status":"","asc":true}}"#;
		if let Ok(body) = gql(HOT_COMICS_QUERY, hot_vars) {
			let entries = parse_comic_list(&body, "hotComics");
			let links: Vec<Link> = entries.into_iter().map(Link::from).collect();

			if !links.is_empty() {
				components.push(HomeComponent {
					title: Some(String::from("本月最夯")),
					subtitle: None,
					value: HomeComponentValue::Scroller {
						entries: links,
						listing: Some(Listing {
							id: String::from("hot"),
							name: String::from("本月最夯"),
							..Default::default()
						}),
					},
				});
			}
		}

		Ok(HomeLayout { components })
	}
}

impl BasicLoginHandler for KomiicSource {
	fn handle_basic_login(
		&self,
		_key: String,
		username: String,
		password: String,
	) -> Result<bool> {
		// POST /api/login with email and password
		let login_body = format!(
			r#"{{"email":"{}","password":"{}"}}"#,
			username, password
		);

		println!(
			"[komiic] Attempting login for: {}",
			username
		);

		let response = Request::post(LOGIN_URL)?
			.header("Content-Type", "application/json")
			.header("Referer", BASE_URL)
			.body(login_body.as_bytes())
			.send()
			.map_err(|_| aidoku::AidokuError::message(String::from("Login request failed")))?;

		let status = response.status_code();
		println!("[komiic] Login response status: {}", status);

		if status == 200 {
			// Try to extract the token from the set-cookie header
			if let Some(cookie_header) = response.get_header("set-cookie") {
				if let Some(token) = extract_token_from_cookie(&cookie_header) {
					println!("[komiic] Login successful, token stored");
					defaults_set(
						"auth_token",
						DefaultValue::String(String::from(token)),
					);
					return Ok(true);
				}
			}
			// Even without cookie, 200 means success - try response body for token
			if let Ok(body) = response.get_string() {
				if let Some(token) = json_str_value(&body, "token") {
					println!("[komiic] Login successful (token from body)");
					defaults_set(
						"auth_token",
						DefaultValue::String(String::from(token)),
					);
					return Ok(true);
				}
			}
			println!("[komiic] Login 200 but no token found");
			Ok(false)
		} else {
			println!("[komiic] Login failed with status {}", status);
			Ok(false)
		}
	}
}



impl ImageRequestProvider for KomiicSource {
	fn get_image_request(
		&self,
		url: String,
		_context: Option<PageContext>,
	) -> Result<Request> {
		if url.starts_with("komiickid://") {
			let kid = &url[12..];
			
			// Fetch ticket for this specific image kid (lazy loading)
			let kids_json = format!(r#"["{}"]"#, kid);
			let ticket_vars = format!(r#"{{"kids":{}}}"#, kids_json);
			
			if let Ok(ticket_body) = gql(IMAGE_TICKETS_QUERY, &ticket_vars) {
				if let Some(tickets_arr) = json_data_field(&ticket_body, "getImageTickets") {
					let ticket_objects = json_top_level_objects(tickets_arr);
					if let Some(ticket_obj) = ticket_objects.first() {
						if let Some(real_url) = json_str_value(ticket_obj, "url") {
							let ticket = json_str_value(ticket_obj, "ticket").unwrap_or("");
							
							let request = Request::get(real_url)?
								.header("Referer", "https://komiic.com")
								.header("x-image-ticket", ticket);
							return Ok(request);
						}
					}
				}
			}
		}

		let request = Request::get(&url)?
			.header("Referer", "https://komiic.com");

		Ok(request)
	}
}

impl DeepLinkHandler for KomiicSource {
	fn handle_deep_link(&self, url: String) -> Result<Option<DeepLinkResult>> {
		// Handle URLs like https://komiic.com/comic/{id}
		if let Some(pos) = url.find("/comic/") {
			let start = pos + 7; // "/comic/".len()
			let rest = &url[start..];
			// Find the end of the comic ID (next '/' or end of string)
			let end = rest.find('/').unwrap_or(rest.len());
			let comic_id = &rest[..end];

			if !comic_id.is_empty() {
				return Ok(Some(DeepLinkResult::Manga {
					key: String::from(comic_id),
				}));
			}
		}

		Ok(None)
	}
}

register_source!(
	KomiicSource,
	ListingProvider,
	Home,
	BasicLoginHandler,
	ImageRequestProvider,
	DeepLinkHandler
);
