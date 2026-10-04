use aidoku::{
	alloc::{format, String, Vec},
	imports::{
		defaults::defaults_get,
		html::{Document, Element, Html},
		net::Request,
	},
	prelude::*,
	Chapter, Manga, MangaPageResult, MangaStatus, Result, Viewer,
};

use crate::content;

/// The Traditional Chinese site, and the default.
pub const TW_URL: &str = "https://tw.linovelib.com";
/// The Simplified Chinese twin. Same template, same book and chapter ids; the only one
/// with a working search (tw's search box goes to Google).
pub const CN_URL: &str = "https://www.bilinovel.com";

/// The site picked in settings.
pub fn base_url() -> &'static str {
	match defaults_get::<String>("site").as_deref() {
		Some("cn") => CN_URL,
		_ => TW_URL,
	}
}

/// The Simplified site answers an iPhone agent without `Safari` (the app's own, and
/// in-app browsers) with a page asking for Chrome or Safari instead of the book.
const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

/// A request the site answers in full. Without both the `night` cookie (any value)
/// and an `Accept-Language`, a chapter page holds back most of its paragraphs. The
/// language must match the site: tw sends a `zh-cn` reader to the Simplified site.
pub fn request(url: &str) -> Result<Request> {
	let language = if url.starts_with(CN_URL) { "zh-CN" } else { "zh-TW" };
	Ok(Request::get(url)?
		.header("User-Agent", USER_AGENT)
		.header("Cookie", "night=0")
		.header("Accept-Language", language))
}

/// A page's HTML, or an error for anything but a page. Cloudflare limits the site to
/// about 14 quick requests and then answers `429` with `error code: 1015` for a few
/// seconds (measured 2026-10-04), which a library refresh of many books reaches. That
/// answer must not be read as a book with no chapters.
pub fn fetch_text(url: &str) -> Result<String> {
	let response = request(url)?.send()?;
	let status = response.status_code();
	if status == 429 {
		println!("[linovelib] rate limited: {url}");
		bail!("[linovelib] rate limited");
	}
	if !(200..300).contains(&status) {
		println!("[linovelib] HTTP {status}: {url}");
		bail!("[linovelib] HTTP {status}");
	}
	response.get_string()
}

pub fn fetch_html(url: &str) -> Result<Document> {
	Ok(Html::parse_with_url(fetch_text(url)?, url)?)
}

/// The site's own copy of an illustration. The page points at img3.readpai.com, which
/// answers 403 without a Referer from the site, and the app loads images in a text page
/// without one; the site serves the same file under `/files/article/attachment/`
/// without that check (same bytes, checked 2026-10-04).
pub fn attachment_url(base: &str, src: &str) -> String {
	match src.split_once(".readpai.com/") {
		Some((host, path)) if host.starts_with("https://") || host.starts_with("http://") => {
			format!("{base}/files/article/attachment/{path}")
		}
		_ => String::from(src),
	}
}

pub fn manga_url(base: &str, key: &str) -> String {
	format!("{base}/novel/{key}.html")
}

pub fn catalog_url(base: &str, key: &str) -> String {
	format!("{base}/novel/{key}/catalog")
}

pub fn volume_url(base: &str, key: &str, volume: &str) -> String {
	format!("{base}/novel/{key}/vol_{volume}.html")
}

pub fn chapter_url(base: &str, key: &str, chapter: &str) -> String {
	format!("{base}/novel/{key}/{chapter}.html")
}

/// The imprint lists `/wenku/<slug>/<page>.html`: listing id (the site's slug) and the
/// name from the page title. The site's `lightnovel` slug is every book on the site,
/// not an imprint, and is left out. Must match `res/source.json`.
pub const IMPRINTS: [(&str, &str); 13] = [
	("dengekibunko", "電擊文庫"),
	("fujimibunko", "富士見文庫"),
	("kadokawabunko", "角川文庫"),
	("emuefubunkojei", "MF文庫J"),
	("famitsubunko", "Fami通文庫"),
	("gagraphicbunko", "GA文庫"),
	("hobbyjapanbunko", "HJ文庫"),
	("ichijinsha", "一迅社"),
	("shueisha", "集英社"),
	("shogakukan", "小學館"),
	("kodansha", "講談社"),
	("teenagebunko", "少女文庫"),
	("other", "其他文庫"),
];

/// A listing's address: the rank and completed boards, an imprint, or the catalogue sorted.
pub fn listing_url(base: &str, id: &str, page: i32) -> Option<String> {
	if IMPRINTS.iter().any(|(slug, _)| *slug == id) {
		return Some(format!("{base}/wenku/{id}/{page}.html"));
	}
	match id {
		"monthvisit" => Some(format!("{base}/top/monthvisit/{page}.html")),
		"full" => Some(format!("{base}/topfull/postdate/{page}.html")),
		"postdate" | "lastupdate" => Some(browse_url(
			base,
			&Browse {
				order: String::from(id),
				..Default::default()
			},
			page,
		)),
		_ => None,
	}
}

/// The catalogue's conditions, all combinable. `0` means any.
pub struct Browse {
	pub order: String,
	/// Genre ids; the site takes up to four and lists the books that have all of them.
	pub tags: Vec<String>,
	pub isfull: String,
	pub anime: String,
	pub region: String,
	pub words: String,
}

impl Default for Browse {
	fn default() -> Self {
		Self {
			order: String::from("lastupdate"),
			tags: Vec::new(),
			isfull: String::from("0"),
			anime: String::from("0"),
			region: String::from("0"),
			words: String::from("0"),
		}
	}
}

/// `/wenku/{order}_{tags}_{isfull}_{anime}_{region}_{sort}_{type}_{words}_{page}_{update}.html`.
/// The sub-category fields stay 0; the last one, update time, has no effect.
pub fn browse_url(base: &str, browse: &Browse, page: i32) -> String {
	let tags = if browse.tags.is_empty() {
		String::from("0")
	} else {
		browse.tags.iter().take(4).cloned().collect::<Vec<String>>().join("-")
	};
	format!(
		"{base}/wenku/{}_{tags}_{}_{}_{}_0_0_{}_{page}_0.html",
		browse.order, browse.isfull, browse.anime, browse.region, browse.words
	)
}

/// The number in `/novel/<n>.html` (a book) or `/novel/<book>/<n>.html` (a chapter).
fn number_before(text: &str, suffix: &str) -> Option<String> {
	let end = text.find(suffix)?;
	let digits: String = text[..end]
		.chars()
		.rev()
		.take_while(|c: &char| c.is_ascii_digit())
		.collect::<Vec<char>>()
		.into_iter()
		.rev()
		.collect();
	(!digits.is_empty()).then_some(digits)
}

/// The book and chapter a link names: `/novel/2.html` → (2, None),
/// `/novel/2/403_2.html` → (2, Some(403)). Volume and catalogue links name only the book.
pub fn parse_link(url: &str) -> Option<(String, Option<String>)> {
	let (_, rest) = url.split_once("/novel/")?;
	let rest = rest.split(['?', '#']).next().unwrap_or(rest);
	let book: String = rest.chars().take_while(|c: &char| c.is_ascii_digit()).collect();
	if book.is_empty() {
		return None;
	}
	let after = &rest[book.len()..];
	if after == ".html" {
		return Some((book, None));
	}
	let tail = after.strip_prefix('/')?;
	if tail == "catalog" || tail.starts_with("vol_") {
		return Some((book, None));
	}
	let chapter: String = tail.chars().take_while(|c: &char| c.is_ascii_digit()).collect();
	let rest = &tail[chapter.len()..];
	let is_page = rest == ".html"
		|| rest
			.strip_prefix('_')
			.and_then(|r: &str| r.strip_suffix(".html"))
			.is_some_and(|n: &str| !n.is_empty() && n.bytes().all(|b: u8| b.is_ascii_digit()));
	(!chapter.is_empty() && is_page).then_some((book, Some(chapter)))
}

fn text_of(el: &Element) -> String {
	el.text().map(|t: String| String::from(t.trim())).unwrap_or_default()
}

fn meta(html: &Document, property: &str) -> Option<String> {
	html.select_first(format!("meta[property=\"{property}\"]"))
		.and_then(|el: Element| el.attr("content"))
		.map(|c: String| String::from(c.trim()))
		.filter(|c: &String| !c.is_empty())
}

/// The list pages: catalogue, rank boards and search results share `li.book-li` cards.
pub fn parse_manga_list(html: &Document, base: &str) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	for card in html.select("li.book-li > a.book-layout").into_iter().flatten() {
		let Some((key, None)) = card.attr("href").and_then(|href: String| parse_link(&href)) else {
			continue;
		};
		if entries.iter().any(|m: &Manga| m.key == key) {
			continue;
		}
		let img = card.select_first("img");
		// The heading is cut short with "..."; the cover's alt has the whole title.
		let title = img
			.as_ref()
			.and_then(|img: &Element| img.attr("alt"))
			.map(|t: String| String::from(t.trim()))
			.filter(|t: &String| !t.is_empty())
			.or_else(|| card.select_first(".book-title").map(|el: Element| text_of(&el)))
			.unwrap_or_default();
		if title.is_empty() {
			continue;
		}
		let cover = img
			.and_then(|img: Element| img.attr("data-src"))
			.filter(|src: &String| src.starts_with("http"));
		let authors: Vec<String> = card
			.select_first(".book-author")
			.map(|el: Element| text_of(&el))
			.filter(|a: &String| !a.is_empty())
			.into_iter()
			.collect();
		entries.push(Manga {
			url: Some(manga_url(base, &key)),
			key,
			title,
			cover,
			authors: (!authors.is_empty()).then_some(authors),
			viewer: Viewer::Vertical,
			..Default::default()
		});
	}
	MangaPageResult {
		has_next_page: !entries.is_empty() && has_next_page(html),
		entries,
	}
}

/// Lists number their pages with the current one in `<strong>`; search results say
/// `第1/10页`.
fn has_next_page(html: &Document) -> bool {
	if let Some(current) = html.select_first("#pagelink strong") {
		return current.next().is_some_and(|el: Element| el.tag_name().as_deref() == Some("a"));
	}
	let text = html.select_first("#pagelink span").map(|el: Element| text_of(&el)).unwrap_or_default();
	let numbers: Vec<i32> = text
		.split(|c: char| !c.is_ascii_digit())
		.filter_map(|n: &str| n.parse().ok())
		.collect();
	matches!(numbers[..], [current, total] if current < total)
}

pub fn parse_details(html: &Document, key: &str, base: &str) -> Option<Manga> {
	let title = html
		.select_first("h1.book-title")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let names = |selector: &str| -> Vec<String> {
		html.select(selector)
			.into_iter()
			.flatten()
			// An illustrator's link reads `ponkan⑧(插畫)`, the label in a ruby reading.
			.map(|el: Element| el.own_text().map(|t: String| String::from(t.trim())).unwrap_or_default())
			.filter(|n: &String| !n.is_empty())
			.collect()
	};
	let authors = names("span.authorname a");
	let artists = names("span.illname a ruby");
	let status = match meta(html, "og:novel:status").as_deref() {
		Some("完結" | "完结") => MangaStatus::Completed,
		Some("連載" | "连载") => MangaStatus::Ongoing,
		_ => MangaStatus::Unknown,
	};
	let mut tags: Vec<String> = Vec::new();
	for link in html.select("em.tag-small.red a").into_iter().flatten() {
		let name = text_of(&link);
		if !name.is_empty() && !tags.contains(&name) {
			tags.push(name);
		}
	}
	let mut description = html
		.select_first("#bookSummary content")
		.and_then(|el: Element| el.html())
		.map(|h: String| content::plain_text(&h))
		.unwrap_or_default();
	if let Some(aliases) = html
		.select_first("aside.backupname .bkname-body")
		.map(|el: Element| text_of(&el))
		.filter(|a: &String| !a.is_empty())
	{
		if !description.is_empty() {
			description.push_str("\n\n");
		}
		description.push_str(&format!("別名：{aliases}"));
	}
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(base, key)),
		title,
		cover: meta(html, "og:image"),
		authors: (!authors.is_empty()).then_some(authors),
		artists: (!artists.is_empty()).then_some(artists),
		description: (!description.is_empty()).then_some(description),
		tags: (!tags.is_empty()).then_some(tags),
		status,
		viewer: Viewer::Vertical,
		..Default::default()
	})
}

/// Key of a chapter the catalogue lists without a link (`javascript:cid(1)`, about one
/// in twenty): its volume and place there. The volume page has the real link, so the
/// id is looked up when the chapter is opened.
pub fn placeholder_key(volume: &str, index: usize) -> String {
	format!("v{volume}-{index}")
}

pub fn parse_placeholder_key(key: &str) -> Option<(&str, usize)> {
	let (volume, index) = key.strip_prefix('v')?.split_once('-')?;
	Some((volume, index.parse().ok()?))
}

/// A volume's name without the book's title: `果然我的青春戀愛喜劇搞錯了 14.5` → `14.5`.
fn volume_label<'a>(name: &'a str, book: &str) -> &'a str {
	match name.strip_prefix(book).map(str::trim) {
		Some(rest) if !rest.is_empty() => rest,
		_ => name.trim(),
	}
}

/// The catalogue, oldest first on the site; the app wants newest first. Titles carry
/// their volume (`【14.5】序章`) and chapters are numbered in catalogue order, since
/// volume names include specials and half volumes that a volume number would misstate.
/// The catalogue replaces some characters in titles with capital letters (`臉S` for
/// `臉色`); those are kept as the site gives them.
pub fn parse_catalog(html: &Document, key: &str, base: &str) -> Vec<Chapter> {
	let book = html.select_first("#chapterNav h1").map(|el: Element| text_of(&el)).unwrap_or_default();
	let mut chapters: Vec<Chapter> = Vec::new();
	for volume in html.select(".catalog-volume").into_iter().flatten() {
		let bar = volume.select_first("li.chapter-bar");
		let volume_id = bar
			.as_ref()
			.and_then(|bar: &Element| bar.select_first("a"))
			.and_then(|a: Element| a.attr("href"))
			.and_then(|href: String| number_before(&href, ".html").filter(|_| href.contains("vol_")));
		let name = bar.map(|bar: Element| text_of(&bar)).unwrap_or_default();
		let label = volume_label(&name, &book);
		for (index, link) in volume.select("li.jsChapter a").into_iter().flatten().enumerate() {
			let title = text_of(&link);
			let href = link.attr("href").unwrap_or_default();
			let (chapter_key, url) = match parse_link(&href) {
				Some((_, Some(chapter))) => {
					let url = chapter_url(base, key, &chapter);
					(chapter, url)
				}
				_ => match &volume_id {
					Some(volume_id) => (placeholder_key(volume_id, index), volume_url(base, key, volume_id)),
					None => continue,
				},
			};
			let title = if label.is_empty() { title } else { format!("【{label}】{title}") };
			chapters.push(Chapter {
				key: chapter_key,
				title: Some(title),
				url: Some(url),
				..Default::default()
			});
		}
	}
	for (number, chapter) in chapters.iter_mut().enumerate() {
		chapter.chapter_number = Some((number + 1) as f32);
	}
	chapters.reverse();
	chapters
}

/// The id of the chapter at `index` on a volume page.
pub fn volume_chapter(html: &Document, index: usize) -> Option<String> {
	html.select("li.jsChapter a")?
		.get(index)
		.and_then(|a: Element| a.attr("href"))
		.and_then(|href: String| parse_link(&href))
		.and_then(|(_, chapter)| chapter)
}

/// `url_next` of the page's `var ReadParams={...}`, written with single quotes.
pub fn next_page_url(raw: &str) -> Option<&str> {
	let params = &raw[raw.find("ReadParams")?..];
	let (_, rest) = params.split_once("url_next:'")?;
	rest.split_once('\'').map(|(url, _)| url)
}

/// Whether `url` is a further page of chapter `chapter` of book `book`
/// (`/novel/2/403_2.html`) rather than the next chapter.
pub fn is_same_chapter(url: &str, book: &str, chapter: &str) -> bool {
	url.split_once(&format!("/novel/{book}/{chapter}_"))
		.and_then(|(_, n)| n.strip_suffix(".html"))
		.is_some_and(|n: &str| !n.is_empty() && n.bytes().all(|b: u8| b.is_ascii_digit()))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, TW_URL).expect("parse")
	}

	#[aidoku_test]
	fn builds_addresses() {
		assert_eq!(
			listing_url(TW_URL, "lastupdate", 2).as_deref(),
			Some("https://tw.linovelib.com/wenku/lastupdate_0_0_0_0_0_0_0_2_0.html")
		);
		assert_eq!(
			listing_url(CN_URL, "full", 1).as_deref(),
			Some("https://www.bilinovel.com/topfull/postdate/1.html")
		);
		assert_eq!(listing_url(TW_URL, "hot", 1), None);
		let browse = Browse {
			order: String::from("words"),
			tags: Vec::from(["64", "48", "63", "27", "26"].map(String::from)),
			isfull: String::from("5"),
			anime: String::from("1"),
			region: String::from("2"),
			words: String::from("3"),
		};
		assert_eq!(
			browse_url(TW_URL, &browse, 3),
			"https://tw.linovelib.com/wenku/words_64-48-63-27_5_1_2_0_0_3_3_0.html"
		);
		assert_eq!(chapter_url(TW_URL, "2", "403"), "https://tw.linovelib.com/novel/2/403.html");
	}

	#[aidoku_test]
	fn moves_illustrations_to_the_site() {
		assert_eq!(
			attachment_url(TW_URL, "https://img3.readpai.com/0/2/109088/199061.jpg"),
			"https://tw.linovelib.com/files/article/attachment/0/2/109088/199061.jpg"
		);
		assert_eq!(
			attachment_url(CN_URL, "https://img3.readpai.com/5/5340/333607/321969.jpeg"),
			"https://www.bilinovel.com/files/article/attachment/5/5340/333607/321969.jpeg"
		);
		assert_eq!(attachment_url(TW_URL, "https://example.com/a.jpg"), "https://example.com/a.jpg");
	}

	#[aidoku_test]
	fn reads_links() {
		let book = |b: &str| Some((String::from(b), None));
		let chapter = |b: &str, c: &str| Some((String::from(b), Some(String::from(c))));
		assert_eq!(parse_link("https://tw.linovelib.com/novel/2.html"), book("2"));
		assert_eq!(parse_link("/novel/2/catalog"), book("2"));
		assert_eq!(parse_link("/novel/2/vol_400.html"), book("2"));
		assert_eq!(parse_link("/novel/2/403.html"), chapter("2", "403"));
		assert_eq!(parse_link("https://www.bilinovel.com/novel/2/403_2.html?x"), chapter("2", "403"));
		assert_eq!(parse_link("javascript:cid(1)"), None);
		assert_eq!(parse_link("/novel/2/403_x.html"), None);
		assert_eq!(parse_placeholder_key("v418-2"), Some(("418", 2)));
		assert_eq!(parse_placeholder_key("420"), None);
	}

	#[aidoku_test]
	fn parses_lists() {
		for (fixture, count, next) in [
			(include_str!("fixtures/wenku.html"), 30, true),
			(include_str!("fixtures/wenku_bili.html"), 30, true),
			(include_str!("fixtures/top.html"), 50, true),
			(include_str!("fixtures/search_bili.html"), 6, false),
		] {
			let result = parse_manga_list(&doc(fixture), TW_URL);
			assert_eq!((result.entries.len(), result.has_next_page), (count, next));
			assert!(result.entries.iter().all(|m: &Manga| m.cover.is_some() && m.authors.is_some()));
		}
		let wenku = parse_manga_list(&doc(include_str!("fixtures/wenku.html")), TW_URL);
		assert_eq!(wenku.entries[1].key, "5407");
		// The whole title, not the heading cut short.
		assert_eq!(wenku.entries[1].title, "牢籠中的囚鳥們 ～從【仙境】開始，向命運發起抗爭～");
		assert_eq!(
			wenku.entries[1].cover.as_deref(),
			Some("https://tw.linovelib.com/files/article/image/5/5407/5407s.jpg?1790926054")
		);
		let top = parse_manga_list(&doc(include_str!("fixtures/top.html")), TW_URL);
		assert_eq!(top.entries[0].title, "無職轉生 ～到了異世界就拿出真本事～");
		assert_eq!(top.entries[0].authors, Some(Vec::from([String::from("理不盡な孫の手")])));
		let search = parse_manga_list(&doc(include_str!("fixtures/search_bili.html")), TW_URL);
		assert_eq!(search.entries[0].key, "2");
		assert_eq!(search.entries[0].url.as_deref(), Some("https://tw.linovelib.com/novel/2.html"));
	}

	#[aidoku_test]
	fn reads_the_details_page() {
		let manga = parse_details(&doc(include_str!("fixtures/novel_2.html")), "2", TW_URL).expect("details");
		assert_eq!(manga.title, "果然我的青春戀愛喜劇搞錯了");
		assert_eq!(manga.authors, Some(Vec::from([String::from("渡航")])));
		assert_eq!(manga.artists, Some(Vec::from([String::from("ponkan⑧")])));
		assert_eq!(manga.status, MangaStatus::Completed);
		assert_eq!(
			manga.tags,
			Some(Vec::from(["校園", "青春", "戀愛", "歡樂向", "後宮", "妹妹"].map(String::from)))
		);
		assert_eq!(manga.cover.as_deref(), Some("https://tw.linovelib.com/files/article/image/0/2/2s.jpg"));
		let description = manga.description.unwrap_or_default();
		assert!(description.starts_with("「我的青春怎麼會變成這樣！」\n高中生八幡生性彆扭"), "{description}");
		assert!(description.ends_with("\n\n別名：我的青春戀愛物語果然有問題。, 春物"), "{description}");
	}

	#[aidoku_test]
	fn reads_the_catalogue() {
		let chapters = parse_catalog(&doc(include_str!("fixtures/catalog_2.html")), "2", TW_URL);
		assert_eq!(chapters.len(), 282);
		let oldest = &chapters[281];
		assert_eq!(oldest.key, "109088");
		assert_eq!(oldest.title.as_deref(), Some("【1】插圖"));
		assert_eq!(oldest.chapter_number, Some(1.0));
		assert_eq!(oldest.url.as_deref(), Some("https://tw.linovelib.com/novel/2/109088.html"));
		assert_eq!(chapters[0].chapter_number, Some(282.0));
		// The unlinked third chapter of volume 418 points at its volume page.
		let unlinked = chapters.iter().find(|c: &&Chapter| c.key == "v418-2").expect("placeholder");
		assert_eq!(unlinked.title.as_deref(), Some("【3】① 於是平冢靜點燃新的戰火"));
		assert_eq!(unlinked.url.as_deref(), Some("https://tw.linovelib.com/novel/2/vol_418.html"));
		assert_eq!(chapters.iter().filter(|c: &&Chapter| c.key.starts_with('v')).count(), 12);
		assert!(chapters.iter().any(|c: &Chapter| c.title.as_deref() == Some("【14.5】① 無論何時，比企谷小町都想要一個嫂子。")));
	}

	#[aidoku_test]
	fn finds_a_chapter_on_its_volume_page() {
		assert_eq!(volume_chapter(&doc(include_str!("fixtures/vol_418.html")), 2).as_deref(), Some("420"));
		assert_eq!(volume_chapter(&doc(include_str!("fixtures/vol_418.html")), 99), None);
	}

	#[aidoku_test]
	fn follows_a_chapter_across_pages() {
		let first = include_str!("fixtures/chapter_403.html");
		assert_eq!(next_page_url(first), Some("/novel/2/403_2.html"));
		assert!(is_same_chapter("/novel/2/403_2.html", "2", "403"));
		let last = next_page_url(include_str!("fixtures/chapter_403_6.html")).expect("url_next");
		assert_eq!(last, "/novel/2/404.html");
		assert!(!is_same_chapter(last, "2", "403"));
		assert!(!is_same_chapter("/novel/2/4030_2.html", "2", "403"));
		assert_eq!(next_page_url(include_str!("fixtures/chapter_bili_403.html")), Some("/novel/2/403_2.html"));
	}
}
