use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::defaults_get,
		html::{Document, Element, ElementList},
		net::{Request, Response},
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Viewer,
};

pub const BASE_URL: &str = "https://www.jjmhw9.top";

/// The site serves a different (mobile) template to phones, whose update page covers
/// one day only and whose pagination links point elsewhere. A desktop agent gets the
/// template this source reads.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// Every mirror writes covers and pages as `https://www.jjmhw9.top/static/upload/...`,
/// and every mirror also serves that path itself.
const UPLOAD_PATH: &str = "/static/upload/";

/// The rank page's four boards: listing id and the heading the site gives the board.
pub const RANKS: [(&str, &str); 4] = [
	("rank_new", "新书榜"),
	("rank_hot", "人气榜"),
	("rank_end", "完结榜"),
	("rank_rec", "推荐榜"),
];

/// Resolve the base URL, most specific source first: the `customBaseUrl` text setting,
/// then the domain the app's `allowsBaseUrlSelect` picker wrote to `url`, then the
/// compiled-in default.
pub fn base_url() -> String {
	for key in ["customBaseUrl", "url"] {
		if let Some(url) = defaults_get::<String>(key)
			.as_deref()
			.and_then(normalize_base_url)
		{
			return url;
		}
	}
	String::from(BASE_URL)
}

/// Tidy a base URL typed by the user or written by the app: trim it, drop the trailing
/// slash and supply `https://` when the scheme was left out. Blank input yields `None`.
pub fn normalize_base_url(value: &str) -> Option<String> {
	let trimmed = value.trim().trim_end_matches('/');
	if trimmed.is_empty() {
		None
	} else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
		Some(String::from(trimmed))
	} else {
		Some(format!("https://{trimmed}"))
	}
}

/// Kept apart from `fetch_html` so the home can send its rows together.
pub fn request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?.header("User-Agent", USER_AGENT))
}

/// A non-2xx answer is an error, so an error page is never read as an empty list.
pub fn fetch_html(url: &str) -> Result<Document> {
	read_html(request(url)?.send()?, url)
}

/// `fetch_html`'s status check, shared with the home's batched rows.
pub fn read_html(response: Response, url: &str) -> Result<Document> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[msxhm] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
}

/// An image address moved onto `base`, so a reader who picked a mirror because the
/// default domain is unreachable gets the images from that mirror too.
pub fn rehost(url: &str, base: &str) -> String {
	match url.find(UPLOAD_PATH) {
		Some(index) if url.starts_with("http") => format!("{base}{}", &url[index..]),
		_ => String::from(url),
	}
}

pub fn manga_url(base: &str, key: &str) -> String {
	format!("{base}/book/{key}")
}

pub fn chapter_url(base: &str, key: &str) -> String {
	format!("{base}/chapter/{key}")
}

/// A listing's address. The rank boards are one page; `None` past it and for unknown ids.
pub fn listing_url(base: &str, id: &str, page: i32) -> Option<String> {
	let path = match id {
		"update" => format!("/update?page={page}"),
		"latest" => format!("/booklist?page={page}"),
		"ongoing" => format!("/booklist?end=0&page={page}"),
		"completed" => format!("/booklist?end=1&page={page}"),
		_ if RANKS.iter().any(|(rank, _)| *rank == id) => {
			if page > 1 {
				return None;
			}
			String::from("/rank")
		}
		_ => return None,
	};
	Some(format!("{base}{path}"))
}

/// The catalogue's three conditions. They combine; `-1` and an absent tag mean any.
pub struct Browse {
	pub tag: Option<String>,
	pub area: String,
	pub end: String,
}

impl Default for Browse {
	fn default() -> Self {
		Self {
			tag: None,
			area: String::from("-1"),
			end: String::from("-1"),
		}
	}
}

pub fn browse_url(base: &str, browse: &Browse, page: i32) -> String {
	let mut url = format!("{base}/booklist?page={page}");
	if let Some(tag) = browse
		.tag
		.as_deref()
		.filter(|t: &&str| !t.trim().is_empty())
	{
		url.push_str(&format!("&tag={}", encode_uri_component(tag.trim())));
	}
	if browse.area != "-1" {
		url.push_str(&format!("&area={}", browse.area));
	}
	if browse.end != "-1" {
		url.push_str(&format!("&end={}", browse.end));
	}
	url
}

/// The search answers 20 works by relevance and has no further pages.
pub fn search_url(base: &str, query: &str) -> String {
	format!(
		"{base}/search?keyword={}",
		encode_uri_component(query.trim())
	)
}

/// The number after `/<kind>/` in a link: `/book/1224` → `1224`.
pub fn id_after(href: &str, kind: &str) -> Option<String> {
	let (_, rest) = href.split_once(&format!("/{kind}/"))?;
	let id: String = rest
		.chars()
		.take_while(|c: &char| c.is_ascii_digit())
		.collect();
	let next = rest[id.len()..].chars().next();
	(!id.is_empty() && matches!(next, None | Some('/' | '?' | '#'))).then_some(id)
}

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// `background-image: url(<address>)`.
fn background_image(style: &str) -> Option<&str> {
	let (_, rest) = style.split_once("url(")?;
	let (url, _) = rest.split_once(')')?;
	let url = url.trim().trim_matches(['"', '\'']);
	(!url.is_empty()).then_some(url)
}

/// Works from links that carry a `title`, paired with the covers drawn as
/// `background-image`. A cover belongs to the work whose id is in its path, since the
/// rank boards draw a cover apart from the work's link.
fn collect(links: Option<ElementList>, covers: Option<ElementList>, base: &str) -> Vec<Manga> {
	let covers: Vec<(String, String)> = covers
		.into_iter()
		.flatten()
		.filter_map(|el: Element| {
			let style = el.attr("style")?;
			let url = background_image(&style)?;
			Some((id_after(url, "book")?, rehost(url, base)))
		})
		.collect();
	let mut entries: Vec<Manga> = Vec::new();
	for link in links.into_iter().flatten() {
		let Some(key) = link
			.attr("href")
			.and_then(|href: String| id_after(&href, "book"))
		else {
			continue;
		};
		let title = link
			.attr("title")
			.map(|t: String| String::from(t.trim()))
			.unwrap_or_default();
		if title.is_empty() || entries.iter().any(|m: &Manga| m.key == key) {
			continue;
		}
		let cover = covers
			.iter()
			.find(|(id, _)| *id == key)
			.map(|(_, url)| url.clone());
		entries.push(Manga {
			url: Some(manga_url(base, &key)),
			key,
			title,
			cover,
			content_rating: ContentRating::NSFW,
			viewer: Viewer::Webtoon,
			..Default::default()
		});
	}
	entries
}

/// The catalogue, update and search pages: a grid of `.mh-item` cards.
pub fn parse_manga_list(html: &Document, base: &str) -> MangaPageResult {
	let entries = collect(
		html.select(".mh-item a[title]"),
		html.select(".mh-item .mh-cover"),
		base,
	);
	let has_next_page = !entries.is_empty() && html.select_first("a#nextPage").is_some();
	MangaPageResult {
		entries,
		has_next_page,
	}
}

/// One board of the rank page, found by its heading. A page without that board is an
/// error rather than an empty board: the site renamed it or answered another page.
pub fn parse_rank(html: &Document, heading: &str, base: &str) -> Result<MangaPageResult> {
	let board = html
		.select("ul.top-cat > li")
		.and_then(|boards: ElementList| {
			boards.into_iter().find(|li: &Element| {
				li.select_first("div.title")
					.map(|t: Element| text_of(&t))
					.as_deref() == Some(heading)
			})
		});
	let Some(board) = board else {
		bail!("[msxhm] no rank board {heading}");
	};
	Ok(MangaPageResult {
		entries: collect(board.select("a[title]"), board.select(".mh-cover"), base),
		has_next_page: false,
	})
}

/// The value of a `<label>：<value>` line among the details page's subtitles and tips.
fn labelled(html: &Document, selector: &str, label: &str) -> Option<String> {
	html.select(selector)?.find_map(|el: Element| {
		let text = text_of(&el);
		let value = text
			.strip_prefix(label)?
			.trim_start_matches(['：', ':'])
			.trim();
		(!value.is_empty()).then(|| String::from(value))
	})
}

pub fn parse_details(html: &Document, key: &str, base: &str) -> Option<Manga> {
	let title = html
		.select_first(".banner_detail_form .info h1")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first(".banner_detail_form .cover img")
		.and_then(|el: Element| el.attr("src"))
		.map(|src: String| rehost(&src, base));
	let authors: Vec<String> = labelled(html, ".banner_detail_form p.subtitle", "作者")
		.map(|names: String| {
			names
				.split('&')
				.map(|n: &str| String::from(n.trim()))
				.filter(|n: &String| !n.is_empty())
				.collect()
		})
		.unwrap_or_default();
	let status = match labelled(html, ".banner_detail_form p.tip span.block", "状态").as_deref() {
		Some(s) if s.contains("完结") => MangaStatus::Completed,
		Some(s) if s.contains("连载") => MangaStatus::Ongoing,
		_ => MangaStatus::Unknown,
	};
	let mut tags: Vec<String> = Vec::new();
	for link in html
		.select(".banner_detail_form p.tip a[href*=\"tag=\"]")
		.into_iter()
		.flatten()
	{
		let name = text_of(&link);
		if !name.is_empty() && !tags.contains(&name) {
			tags.push(name);
		}
	}
	let description = html
		.select_first(".banner_detail_form p.content")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(base, key)),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		tags: (!tags.is_empty()).then_some(tags),
		status,
		content_rating: ContentRating::NSFW,
		// Pages are one long strip cut into consecutive images.
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

/// `第12話-標題` → 12. Titles without a number get none.
pub fn chapter_number(title: &str) -> Option<f32> {
	let rest = title.trim().strip_prefix('第')?;
	let end = rest.find(['話', '话', '回'])?;
	rest[..end].trim().parse::<f32>().ok()
}

/// The details page lists every chapter, oldest first; the app wants newest first.
/// A link that is not a chapter gives an empty list, which the caller treats as unreadable,
/// so the library is not handed a list with chapters missing.
pub fn parse_chapters(html: &Document, base: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();
	for link in html.select("#detail-list-select a").into_iter().flatten() {
		let Some(key) = link
			.attr("href")
			.and_then(|href: String| id_after(&href, "chapter"))
		else {
			return Vec::new();
		};
		if chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		let title = text_of(&link);
		chapters.push(Chapter {
			url: Some(chapter_url(base, &key)),
			chapter_number: chapter_number(&title),
			title: (!title.is_empty()).then_some(title),
			key,
			..Default::default()
		});
	}
	chapters.reverse();
	chapters
}

pub fn parse_pages(html: &Document, base: &str) -> Vec<Page> {
	html.select(".comicpage img[data-original]")
		.into_iter()
		.flatten()
		.filter_map(|img: Element| img.attr("data-original"))
		.filter(|url: &String| url.starts_with("http"))
		.map(|url: String| Page {
			content: PageContent::url(rehost(&url, base)),
			..Default::default()
		})
		.collect()
}

/// The work a reader page belongs to.
pub fn reader_manga_key(html: &Document) -> Option<String> {
	html.select_first("a.comic-name")
		.and_then(|el: Element| el.attr("href"))
		.and_then(|href: String| id_after(&href, "book"))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, BASE_URL).expect("parse")
	}

	const MIRROR: &str = "https://www.mxs13.cc";

	#[aidoku_test]
	fn builds_addresses() {
		assert_eq!(
			listing_url(BASE_URL, "completed", 2).as_deref(),
			Some("https://www.jjmhw9.top/booklist?end=1&page=2")
		);
		assert_eq!(
			listing_url(BASE_URL, "update", 1).as_deref(),
			Some("https://www.jjmhw9.top/update?page=1")
		);
		assert_eq!(
			listing_url(BASE_URL, "rank_hot", 1).as_deref(),
			Some("https://www.jjmhw9.top/rank")
		);
		assert_eq!(listing_url(BASE_URL, "rank_hot", 2), None);
		assert_eq!(listing_url(BASE_URL, "hot", 1), None);
		let browse = Browse {
			tag: Some(String::from("巨乳")),
			end: String::from("1"),
			..Default::default()
		};
		assert_eq!(
			browse_url(BASE_URL, &browse, 3),
			"https://www.jjmhw9.top/booklist?page=3&tag=%E5%B7%A8%E4%B9%B3&end=1"
		);
		assert_eq!(
			browse_url(MIRROR, &Browse::default(), 1),
			"https://www.mxs13.cc/booklist?page=1"
		);
		assert_eq!(
			search_url(BASE_URL, " 秘密 "),
			"https://www.jjmhw9.top/search?keyword=%E7%A7%98%E5%AF%86"
		);
		assert_eq!(
			normalize_base_url(" www.mxs13.cc/ ").as_deref(),
			Some(MIRROR)
		);
		assert_eq!(normalize_base_url("  "), None);
	}

	#[aidoku_test]
	fn reads_ids_and_moves_images() {
		assert_eq!(id_after("/book/1224", "book").as_deref(), Some("1224"));
		assert_eq!(
			id_after("https://www.mxs13.cc/chapter/57217?x", "chapter").as_deref(),
			Some("57217")
		);
		assert_eq!(id_after("/booklist?tag=都市", "book"), None);
		assert_eq!(id_after("/book/12a", "book"), None);
		assert_eq!(
			rehost(
				"https://www.jjmhw9.top/static/upload/book/1224/cover.jpg",
				MIRROR
			),
			"https://www.mxs13.cc/static/upload/book/1224/cover.jpg"
		);
		assert_eq!(
			rehost("/static/images/logo.png", MIRROR),
			"/static/images/logo.png"
		);
		// A library cover saved under one mirror ends up on the mirror in use.
		let cover = "https://www.jjmhw9.top/static/upload/book/1224/cover.jpg";
		assert_eq!(
			rehost(&rehost(cover, "https://www.jjmhw8.top"), MIRROR),
			rehost(cover, MIRROR)
		);
	}

	#[aidoku_test]
	fn a_link_that_is_not_a_chapter_rejects_the_list() {
		let html = r#"<ul id="detail-list-select"><li><a href="/chapter/12">第1話</a></li><li><a href="/book/3">更多</a></li></ul>"#;
		assert!(parse_chapters(&doc(html), BASE_URL).is_empty());
		let html = r#"<ul id="detail-list-select"><li><a href="/chapter/12">第1話</a></li><li><a href="/chapter/13">第2話</a></li></ul>"#;
		let keys: Vec<String> = parse_chapters(&doc(html), BASE_URL)
			.into_iter()
			.map(|c| c.key)
			.collect();
		assert_eq!(keys, ["13", "12"]);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
