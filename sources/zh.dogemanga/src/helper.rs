use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		html::{Document, Element},
		net::Request,
	},
	prelude::*,
	Chapter, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Viewer,
};

pub const BASE_URL: &str = "https://dogemanga.com";

/// Pages and covers answer 403 with a Cloudflare challenge unless they carry this.
pub const REFERER: &str = "https://dogemanga.com/";

/// Search pages are offset by this many results (`&o=24`, `&o=48`, ...).
const SEARCH_PAGE_SIZE: i32 = 24;

pub fn fetch_html(url: &str) -> Result<Document> {
	Ok(Request::get(url)?.html()?)
}

pub fn manga_url(id: &str) -> String {
	format!("{BASE_URL}/m/{id}")
}

pub fn reader_url(id: &str) -> String {
	format!("{BASE_URL}/p/{id}")
}

/// The first page of a listing. The rest are reached through the cursor its next-page
/// button carries, see `next_page_url`.
pub fn listing_url(listing: &str) -> Option<String> {
	match listing {
		"hot" => Some(format!("{BASE_URL}/")),
		"latest" => Some(format!("{BASE_URL}/?s=1")),
		_ => None,
	}
}

pub fn search_url(query: &str, page: i32) -> String {
	let query = encode_uri_component(query.trim());
	if page > 1 {
		format!("{BASE_URL}/?q={query}&o={}", (page - 1) * SEARCH_PAGE_SIZE)
	} else {
		format!("{BASE_URL}/?q={query}")
	}
}

/// The listings page by a cursor (`?o=24&p=<last id of the previous page>`), not by a
/// number, so page N's address is only known from page N-1. It is kept when page N-1 is
/// read; the app asks for pages in order.
pub fn remember_next_page(listing: &str, page: i32, url: &str) {
	defaults_set(
		&format!("next_{listing}_{page}"),
		DefaultValue::String(String::from(url)),
	);
}

pub fn next_page_url(listing: &str, page: i32) -> Option<String> {
	defaults_get::<String>(&format!("next_{listing}_{page}")).filter(|u: &String| !u.is_empty())
}

/// `.../<marker><id>` → `<id>`, for `/m/` (works) and `/p/` (reader pages).
pub fn id_after(href: &str, marker: &str) -> Option<String> {
	let (_, rest) = href.split_once(marker)?;
	let id = rest.split(['/', '?', '#']).next().unwrap_or(rest);
	(!id.is_empty()).then(|| String::from(id))
}

fn text_of(el: &Element) -> String {
	el.text().map(|t: String| String::from(t.trim())).unwrap_or_default()
}

/// `連載狀態：連載中` / `連載狀態：連載完結`, inside the card's small print.
fn parse_status(text: &str) -> MangaStatus {
	let Some((_, rest)) = text.split_once("連載狀態：") else {
		return MangaStatus::Unknown;
	};
	let rest = rest.trim_start();
	if rest.starts_with("連載中") {
		MangaStatus::Ongoing
	} else if rest.starts_with("連載完結") || rest.starts_with("已完結") || rest.starts_with("完結") {
		MangaStatus::Completed
	} else {
		MangaStatus::Unknown
	}
}

/// A `.site-card[data-manga-id]` block. List cards and the details page header share the
/// class names. The author links are searches (`/?q=<name>`); the small print also holds
/// an empty last-read link, which has no `href`.
pub fn parse_card(card: &Element) -> Option<Manga> {
	let key = card
		.attr("data-manga-id")
		.map(|id: String| String::from(id.trim()))
		.filter(|id: &String| !id.is_empty())?;
	let title = card
		.select_first(".site-card__manga-title")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = card
		.select_first("img")
		.and_then(|el: Element| el.attr("src"))
		.filter(|src: &String| src.starts_with("http"));

	let mut authors: Vec<String> = Vec::new();
	if let Some(links) = card.select("a[href*='?q=']") {
		for link in links {
			let name = text_of(&link);
			if !name.is_empty() && !authors.contains(&name) {
				authors.push(name);
			}
		}
	}

	let description = card
		.select_first(".site-card__brief")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	let status = card
		.select_first("small")
		.map(|el: Element| parse_status(&text_of(&el)))
		.unwrap_or(MangaStatus::Unknown);

	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		status,
		// The reading direction is not given anywhere; most works are Japanese manga.
		viewer: Viewer::RightToLeft,
		..Default::default()
	})
}

/// `...href="<url>" role="button">下一頁</a>` → `<url>`, with `&amp;` decoded.
fn next_in_markup(markup: &str) -> Option<String> {
	let (before, _) = markup.split_once("下一頁")?;
	let (_, rest) = before.rsplit_once("href=\"")?;
	let (href, _) = rest.split_once('"')?;
	Some(href.replace("&amp;", "&"))
}

/// The site's `下一頁` button. Its `href` is the whole next-page address. The button only
/// exists inside `<noscript>` (the site scrolls with a script), which the parser keeps as
/// text, so the markup is read as a string.
pub fn next_href(root: &Element) -> Option<String> {
	let in_element = root
		.select("a.btn-primary")
		.and_then(|mut links| links.find(|a: &Element| text_of(a).contains("下一頁")))
		.and_then(|a: Element| a.attr("href"));
	in_element
		.or_else(|| {
			root.select("noscript")?.find_map(|el: Element| {
				el.text()
					.as_deref()
					.and_then(next_in_markup)
					.or_else(|| el.html().as_deref().and_then(next_in_markup))
			})
		})
		.filter(|href: &String| href.starts_with("http"))
}

/// A listing or search page and its next-page address.
pub fn parse_manga_list(html: Document) -> (MangaPageResult, Option<String>) {
	let root: Element = html.into();
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(cards) = root.select("div.site-card[data-manga-id]") {
		for card in cards {
			if let Some(manga) = parse_card(&card) {
				if !entries.iter().any(|m: &Manga| m.key == manga.key) {
					entries.push(manga);
				}
			}
		}
	}
	let next = if entries.is_empty() {
		None
	} else {
		next_href(&root)
	};
	(
		MangaPageResult {
			entries,
			has_next_page: next.is_some(),
		},
		next,
	)
}

pub fn parse_details(html: &Document) -> Option<Manga> {
	parse_card(&html.select_first(".site-card[data-manga-id]")?)
}

/// `第281话试看` → (Some(281), None), `第06卷` → (None, Some(6)). Side stories carry no
/// number.
pub fn chapter_numbers(title: &str) -> (Option<f32>, Option<f32>) {
	let Some(rest) = title.trim().strip_prefix('第') else {
		return (None, None);
	};
	let end = rest
		.find(|c: char| !(c.is_ascii_digit() || c == '.'))
		.unwrap_or(rest.len());
	let Ok(number) = rest[..end].parse::<f32>() else {
		return (None, None);
	};
	match rest[end..].chars().next() {
		Some('话' | '話' | '回') => (Some(number), None),
		Some('卷') => (None, Some(number)),
		_ => (None, None),
	}
}

/// The page ids listed under one of the details page's tabs.
fn tab_keys(html: &Document, tab: &str) -> Vec<String> {
	let mut keys: Vec<String> = Vec::new();
	if let Some(links) = html.select(format!("#site-manga__tab-pane-{tab} a.site-manga-thumbnail__link")) {
		for link in links {
			if let Some(key) = link.attr("href").and_then(|href: String| id_after(&href, "/p/")) {
				keys.push(key);
			}
		}
	}
	keys
}

/// The `全部` tab: single chapters, side stories and volumes together, in the site's
/// order (volumes are not in volume order there either). A chapter's key is the id of
/// its first page.
///
/// Numbers come from the title but only within the tab the entry sits in: extras such
/// as `第05卷特典` are under `番外篇`, and numbering them as volume 5 would show two
/// volume 5s next to `單行本`'s `第05卷`.
pub fn parse_chapters(html: &Document) -> Vec<Chapter> {
	let volumes = tab_keys(html, "tankobon");
	let extras = tab_keys(html, "bangaihen");
	let mut chapters: Vec<Chapter> = Vec::new();
	let Some(links) = html.select("#site-manga__tab-pane-all a.site-manga-thumbnail__link") else {
		return chapters;
	};
	for link in links {
		let Some(key) = link.attr("href").and_then(|href: String| id_after(&href, "/p/")) else {
			continue;
		};
		if chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		let title = link
			.select_first("img")
			.and_then(|el: Element| el.attr("alt"))
			.map(|t: String| String::from(t.trim()))
			.filter(|t: &String| !t.is_empty())
			.or_else(|| {
				link.select_first("span.text-center")
					.map(|el: Element| text_of(&el))
					.filter(|t: &String| !t.is_empty())
			});
		let (chapter, volume) = title.as_deref().map(chapter_numbers).unwrap_or((None, None));
		let (chapter_number, volume_number) = if volumes.contains(&key) {
			(None, volume)
		} else if extras.contains(&key) {
			(None, None)
		} else {
			(chapter, None)
		};
		chapters.push(Chapter {
			url: Some(reader_url(&key)),
			key,
			title,
			chapter_number,
			volume_number,
			..Default::default()
		});
	}
	chapters
}

/// Every page of the chapter is listed on any of its reader pages, in
/// `data-page-index` order.
fn reader_images(html: &Document) -> Vec<(i32, Element)> {
	let mut images: Vec<(i32, Element)> = Vec::new();
	if let Some(els) = html.select("img.site-reader__image[data-page-index]") {
		for el in els {
			if let Some(index) = el
				.attr("data-page-index")
				.and_then(|i: String| i.trim().parse::<i32>().ok())
			{
				images.push((index, el));
			}
		}
	}
	images.sort_by_key(|(index, _)| *index);
	images
}

pub fn parse_pages(html: &Document) -> Vec<Page> {
	reader_images(html)
		.into_iter()
		.filter_map(|(_, el)| el.attr("data-page-image-url"))
		.filter(|url: &String| url.starts_with("http"))
		.map(|url: String| Page {
			content: PageContent::url(url),
			..Default::default()
		})
		.collect()
}

/// A reader page opened from a link: (work id, chapter key). The chapter key is the
/// first page's id, which is listed even when the link points into the middle.
pub fn reader_keys(html: &Document) -> Option<(String, String)> {
	let manga = html
		.select_first("a[href*='/m/']")
		.and_then(|a: Element| a.attr("href"))
		.and_then(|href: String| id_after(&href, "/m/"))?;
	let (_, first) = reader_images(html).into_iter().next()?;
	let chapter = first
		.attr("data-page-id")
		.map(|id: String| String::from(id.trim()))
		.filter(|id: &String| !id.is_empty())?;
	Some((manga, chapter))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, BASE_URL).expect("parse")
	}

	#[aidoku_test]
	fn reads_ids_from_links() {
		assert_eq!(id_after("https://dogemanga.com/m/zw7V_YJF", "/m/").as_deref(), Some("zw7V_YJF"));
		assert_eq!(id_after("https://dogemanga.com/p/-ljHIJQj?x=1", "/p/").as_deref(), Some("-ljHIJQj"));
		assert_eq!(id_after("https://dogemanga.com/?s=1", "/m/"), None);
		assert_eq!(id_after("https://dogemanga.com/m/", "/m/"), None);
	}

	#[aidoku_test]
	fn builds_search_addresses() {
		assert_eq!(search_url("海賊", 1), "https://dogemanga.com/?q=%E6%B5%B7%E8%B3%8A");
		assert_eq!(search_url("海賊", 3), "https://dogemanga.com/?q=%E6%B5%B7%E8%B3%8A&o=48");
	}

	#[aidoku_test]
	fn parses_the_hot_list_and_its_cursor() {
		let (result, next) = parse_manga_list(doc(include_str!("fixtures/hot.html")));
		assert_eq!(result.entries.len(), 23);
		assert!(result.has_next_page);
		assert_eq!(next.as_deref(), Some("https://dogemanga.com/?p=ymBm3Ity"));
		// The page has a card with no content (no title); it is skipped.
		assert!(!result.entries.iter().any(|m: &Manga| m.key == "TXgjojpX"));
		let first = &result.entries[0];
		assert_eq!(first.key, "gt9rgUMw");
		assert!(first
			.cover
			.as_deref()
			.is_some_and(|c: &str| c == "https://dogemanga.com/images/manga-thumbnails/gt9rgUMw.jpg"));
		let shangri = result
			.entries
			.iter()
			.find(|m: &&Manga| m.key == "zw7V_YJF")
			.expect("card");
		assert_eq!(shangri.title, "香格里拉·弗陇提亚~屎作猎人向神作发起挑战~");
		assert_eq!(
			shangri.authors,
			Some(Vec::from([String::from("不二凉介"), String::from("硬梨菜")]))
		);
		assert_eq!(shangri.status, MangaStatus::Ongoing);
	}

	#[aidoku_test]
	fn the_second_hot_page_carries_the_next_cursor() {
		let (result, next) = parse_manga_list(doc(include_str!("fixtures/hot_2.html")));
		assert_eq!(result.entries.len(), 22);
		assert_eq!(next.as_deref(), Some("https://dogemanga.com/?o=24&p=YVCaU30Y"));
	}

	#[aidoku_test]
	fn parses_the_latest_list() {
		let (result, next) = parse_manga_list(doc(include_str!("fixtures/latest.html")));
		assert_eq!(result.entries.len(), 24);
		assert_eq!(next.as_deref(), Some("https://dogemanga.com/?p=thlLShZp&s=1"));
		assert!(result
			.entries
			.iter()
			.any(|m: &Manga| m.status == MangaStatus::Completed));
	}

	#[aidoku_test]
	fn search_pages_end_without_a_button() {
		let (result, next) = parse_manga_list(doc(include_str!("fixtures/search.html")));
		assert_eq!(result.entries.len(), 24);
		assert_eq!(next.as_deref(), Some("https://dogemanga.com/?o=24&q=%E6%B5%B7%E8%B3%8A"));
		let (result, next) = parse_manga_list(doc(include_str!("fixtures/search_last.html")));
		assert_eq!(result.entries.len(), 19);
		assert_eq!(next, None);
		assert!(!result.has_next_page);
	}

	#[aidoku_test]
	fn reads_the_details_page() {
		let manga = parse_details(&doc(include_str!("fixtures/manga.html"))).expect("details");
		assert_eq!(manga.key, "zw7V_YJF");
		assert_eq!(manga.title, "香格里拉·弗陇提亚~屎作猎人向神作发起挑战~");
		assert_eq!(
			manga.authors,
			Some(Vec::from([String::from("不二凉介"), String::from("硬梨菜")]))
		);
		assert!(manga.description.as_deref().is_some_and(|d: &str| d.starts_with("BUG使人")));
		assert_eq!(manga.status, MangaStatus::Ongoing);
		assert_eq!(
			manga.cover.as_deref(),
			Some("https://dogemanga.com/images/manga-thumbnails/zw7V_YJF.jpg")
		);
	}

	#[aidoku_test]
	fn lists_every_chapter_and_volume_newest_first() {
		let chapters = parse_chapters(&doc(include_str!("fixtures/manga.html")));
		assert_eq!(chapters.len(), 289);
		assert_eq!(chapters[0].key, "722O_Csr");
		assert_eq!(chapters[0].title.as_deref(), Some("第281话试看"));
		assert_eq!(chapters[0].chapter_number, Some(281.0));
		assert_eq!(chapters[0].url.as_deref(), Some("https://dogemanga.com/p/722O_Csr"));
		let last = chapters.last().expect("first chapter");
		assert_eq!(last.chapter_number, Some(1.0));
		let volumes = chapters.iter().filter(|c: &&Chapter| c.volume_number.is_some()).count();
		assert_eq!(volumes, 6);
		let side = chapters
			.iter()
			.find(|c: &&Chapter| c.title.as_deref() == Some("吐槽短篇"))
			.expect("side story");
		assert_eq!((side.chapter_number, side.volume_number), (None, None));
	}

	#[aidoku_test]
	fn extras_titled_as_volumes_get_no_volume_number() {
		let chapters = parse_chapters(&doc(include_str!("fixtures/manga_extras.html")));
		assert_eq!(chapters.len(), 483);
		let volumes = chapters.iter().filter(|c: &&Chapter| c.volume_number.is_some()).count();
		assert_eq!(volumes, 28);
		for title in ["第05卷特典", "第07卷Animate特典"] {
			let extra = chapters
				.iter()
				.find(|c: &&Chapter| c.title.as_deref() == Some(title))
				.expect(title);
			assert_eq!((extra.chapter_number, extra.volume_number), (None, None), "{title}");
		}
		let volume = chapters
			.iter()
			.find(|c: &&Chapter| c.title.as_deref() == Some("第05卷"))
			.expect("volume 5");
		assert_eq!((volume.chapter_number, volume.volume_number), (None, Some(5.0)));
		// The site's own order, oldest last: volumes 7, 8, 6, 1 from the bottom.
		let tail: Vec<Option<f32>> =
			chapters[chapters.len() - 4..].iter().map(|c: &Chapter| c.volume_number).collect();
		assert_eq!(tail, Vec::from([Some(1.0), Some(6.0), Some(8.0), Some(7.0)]));
	}

	#[aidoku_test]
	fn numbers_chapters_and_volumes() {
		assert_eq!(chapter_numbers("第281话试看"), (Some(281.0), None));
		assert_eq!(chapter_numbers("第276话 试看"), (Some(276.0), None));
		assert_eq!(chapter_numbers("第09话"), (Some(9.0), None));
		assert_eq!(chapter_numbers("第10.5話"), (Some(10.5), None));
		assert_eq!(chapter_numbers("第06卷"), (None, Some(6.0)));
		assert_eq!(chapter_numbers("吐槽短篇"), (None, None));
	}

	#[aidoku_test]
	fn reads_every_page_from_a_mid_chapter_reader_page() {
		let html = doc(include_str!("fixtures/reader.html"));
		let pages = parse_pages(&html);
		assert_eq!(pages.len(), 19);
		assert_eq!(
			pages[0].content,
			PageContent::url("https://dogemanga.com/images/pages/5gT25mfd.jpg")
		);
		assert_eq!(
			pages[1].content,
			PageContent::url("https://dogemanga.com/images/pages/zQtfEe-q.jpg")
		);
		assert_eq!(
			reader_keys(&html),
			Some((String::from("zw7V_YJF"), String::from("5gT25mfd")))
		);
	}

	#[aidoku_test]
	fn reads_status_text() {
		assert_eq!(parse_status("連載狀態：連載中最近更新：2 天前"), MangaStatus::Ongoing);
		assert_eq!(parse_status("連載狀態：連載完結最近更新：昨天"), MangaStatus::Completed);
		assert_eq!(parse_status("最近更新：昨天"), MangaStatus::Unknown);
	}
}
