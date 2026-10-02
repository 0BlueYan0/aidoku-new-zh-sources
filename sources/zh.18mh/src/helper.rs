use aidoku::{
	alloc::{String, Vec},
	imports::{
		defaults::{defaults_get, defaults_get_map, defaults_set, DefaultValue},
		html::{Document, Element},
		net::Request,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Viewer,
};

pub const BASE_URL: &str = "https://18mh.org";

/// `filters.json`'s `genre` options as (id, name), in the same order. A tapped tag arrives
/// as its name, so details list tags under these names and the search maps them back.
pub const CATEGORIES: [(&str, &str); 36] = [
	("manga-genre/hanman", "韓漫"),
	("manga-genre/riman", "日漫"),
	("manga-genre/zhenrenxiezhen", "真人寫真"),
	("manga-genre/aixiezhen", "AI寫真"),
	("manga-genre/hots", "熱門漫畫"),
	("manga-tag/duoren", "多人"),
	("manga-tag/yuwang", "慾望"),
	("manga-tag/zhengmei", "正妹"),
	("manga-tag/tongju", "同居"),
	("manga-tag/nxuesheng", "女學生"),
	("manga-tag/juqing", "劇情"),
	("manga-tag/touqing", "偷情"),
	("manga-tag/xiaoyuan", "校園"),
	("manga-tag/nixi", "逆襲"),
	("manga-tag/bangongshi", "辦公室"),
	("manga-tag/youhuo", "誘惑"),
	("manga-tag/fanzhuan", "反轉"),
	("manga-tag/shun", "熟女"),
	("manga-tag/renqi", "人妻"),
	("manga-tag/chulian", "初戀"),
	("manga-tag/shaofu", "少婦"),
	("manga-tag/ciji", "刺激"),
	("manga-tag/ndaxuesheng", "女大學生"),
	("manga-tag/zhiliao", "治療"),
	("manga-tag/chaonengli", "超能力"),
	("manga-tag/langmanxiaoyuan", "浪漫校園"),
	("manga-tag/xiju", "戲劇"),
	("manga-tag/xuejie", "學姐"),
	("manga-tag/daxuesheng", "大學生"),
	("manga-tag/yongyi", "泳衣"),
	("manga-tag/aimei", "曖昧"),
	("manga-tag/xiezhen", "寫真"),
	("manga-tag/nshen", "女神"),
	("manga-tag/dachidu", "大尺度"),
	("manga-tag/chunqingjingcha", "純情警察"),
	("manga-tag/recommed", "推薦"),
];

/// The settings login item that opens the site so the reader can pass the Cloudflare
/// check by hand. The app keeps the page's cookies under this key until it is logged out.
pub const CLOUDFLARE_KEY: &str = "cloudflare";
pub const CLEARANCE_COOKIE: &str = "cf_clearance";

/// No `User-Agent`: the site sits behind a Cloudflare challenge, and the app only reuses
/// the `cf_clearance` it solved for when the request carries the app's own user agent,
/// which it adds when the source sets none.
///
/// The app's own challenge handling has failed on the device (timed out, or solved and
/// still challenged on the retry). As a fallback, a clearance the reader got through the
/// settings button is sent as well. The app puts its own jar's cookies in front of ours.
pub fn fetch_html(url: &str) -> Result<Document> {
	let mut request = Request::get(url)?;
	if let Some(clearance) = defaults_get_map(CLOUDFLARE_KEY)
		.and_then(|cookies| cookies.get(CLEARANCE_COOKIE).cloned())
		.filter(|value: &String| !value.is_empty())
	{
		request = request.header("Cookie", &format!("{CLEARANCE_COOKIE}={clearance}"));
	}
	Ok(request.html()?)
}

pub fn manga_url(slug: &str) -> String {
	format!("{BASE_URL}/manga/{slug}")
}

pub fn list_url(path: &str, page: i32) -> String {
	format!("{BASE_URL}/{path}/page/{page}")
}

pub fn chapters_url(mid: &str) -> String {
	format!("{BASE_URL}/manga/get?mid={mid}&mode=all")
}

pub fn content_url(mid: &str, cs: &str) -> String {
	format!("{BASE_URL}/chapter/getcontent?m={mid}&c={cs}")
}

/// A site path without the host or the slashes around it.
fn site_path(href: &str) -> &str {
	href.trim()
		.trim_start_matches(BASE_URL)
		.trim_matches('/')
}

/// `/manga/<slug>` (absolute or not) → `<slug>`. Reader links `/manga/<slug>/<x>` give
/// the slug too.
pub fn slug_from_href(href: &str) -> Option<String> {
	let rest = site_path(href).strip_prefix("manga/")?;
	let slug = rest.split(['/', '?', '#']).next().unwrap_or(rest);
	(!slug.is_empty()).then(|| String::from(slug))
}

/// A `genre` filter value: an option id as sent from the filter sheet, or an option
/// name as sent by a tapped tag. `None` for anything else.
pub fn category_path(value: &str) -> Option<&'static str> {
	CATEGORIES
		.iter()
		.find(|(id, name)| *id == value || *name == value)
		.map(|(id, _)| *id)
}

fn category_name(path: &str) -> Option<&'static str> {
	CATEGORIES
		.iter()
		.find(|(id, _)| *id == path)
		.map(|(_, name)| *name)
}

/// Author pages are keyed by a pinyin slug (`鍾普林` → `zhongpulin`) that cannot be
/// derived from the name, and the site search only matches titles. The slugs are kept
/// as details pages are opened, which is always before an author can be tapped.
pub fn remember_author(name: &str, slug: &str) {
	defaults_set(&format!("author_{name}"), DefaultValue::String(String::from(slug)));
}

pub fn author_slug(name: &str) -> Option<String> {
	defaults_get::<String>(&format!("author_{}", name.trim())).filter(|s: &String| !s.is_empty())
}

/// A work's numeric id never changes, so a chapter-only library refresh can skip the
/// details page.
pub fn remember_mid(slug: &str, mid: &str) {
	defaults_set(&format!("mid_{slug}"), DefaultValue::String(String::from(mid)));
}

pub fn cached_mid(slug: &str) -> Option<String> {
	defaults_get::<String>(&format!("mid_{slug}")).filter(|s: &String| !s.is_empty())
}

fn text_of(el: &Element) -> String {
	el.text().map(|t: String| String::from(t.trim())).unwrap_or_default()
}

/// One cover card: `a[href=/manga/<slug>]` holding `img` and `h3`. The list pages and
/// the home rows share it, and so do the home page's `近期更新` slides.
pub fn parse_card(link: &Element) -> Option<Manga> {
	let key = slug_from_href(&link.attr("href")?)?;
	let title = link.select_first("h3").map(|el: Element| text_of(&el))?;
	if title.is_empty() {
		return None;
	}
	let cover = link
		.select_first("img")
		.and_then(|el: Element| el.attr("src"))
		.filter(|src: &String| src.starts_with("http"));
	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		content_rating: ContentRating::NSFW,
		..Default::default()
	})
}

pub fn parse_cards(root: &Element, selector: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(links) = root.select(selector) {
		for link in links {
			if let Some(manga) = parse_card(&link) {
				if !entries.iter().any(|m: &Manga| m.key == manga.key) {
					entries.push(manga);
				}
			}
		}
	}
	entries
}

/// A list, search, genre, tag or author page. The paginator shows a `下一頁` button only
/// when another page follows.
pub fn parse_manga_list(html: Document) -> MangaPageResult {
	let root: Element = html.into();
	let entries = parse_cards(&root, ".container > .cardlist .pb-2 a");
	let has_next_page =
		!entries.is_empty() && root.select_first("a[aria-label='下一頁'] button").is_some();
	MangaPageResult {
		entries,
		has_next_page,
	}
}

/// The details page. Returns the work's numeric id alongside, which the chapter list and
/// the pages are keyed by. Both `#firstchap` and `#mangachapters` carry it; either one
/// is enough.
pub fn parse_details(html: &Document, slug: &str) -> (Manga, Option<String>) {
	let mid = ["#firstchap", "#mangachapters"].iter().find_map(|sel: &&str| {
		html.select_first(sel)
			.and_then(|el: Element| el.attr("data-mid"))
			.map(|m: String| String::from(m.trim()))
			.filter(|m: &String| !m.is_empty())
	});

	// The age warning dialog has its own `h1`, outside `main`.
	let heading = html.select_first("main h1.text-xl");
	let title = heading
		.as_ref()
		.and_then(|h: &Element| h.own_text())
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default();
	let status = match heading
		.as_ref()
		.and_then(|h: &Element| h.select_first("span"))
		.map(|el: Element| text_of(&el))
		.as_deref()
	{
		Some("連載中") => MangaStatus::Ongoing,
		Some("完結") => MangaStatus::Completed,
		Some("停止更新") => MangaStatus::Cancelled,
		Some("休刊") => MangaStatus::Hiatus,
		_ => MangaStatus::Unknown,
	};

	let mut authors: Vec<String> = Vec::new();
	if let Some(links) = html.select("main a[href*='/manga-author/']") {
		for link in links {
			let name = String::from(text_of(&link).trim_end_matches(',').trim());
			let Some(href) = link.attr("href") else {
				continue;
			};
			let author_slug = site_path(&href).trim_start_matches("manga-author/");
			if name.is_empty() || authors.contains(&name) {
				continue;
			}
			remember_author(&name, author_slug);
			authors.push(name);
		}
	}

	let mut tags: Vec<String> = Vec::new();
	let mut is_hanman = false;
	if let Some(links) = html.select("main a[href*='/manga-genre/'], main a[href*='/manga-tag/']") {
		for link in links {
			let Some(href) = link.attr("href") else {
				continue;
			};
			let path = site_path(&href);
			// The breadcrumb repeats the genre link; repeats are dropped below.
			is_hanman |= path == "manga-genre/hanman";
			let name = match category_name(path) {
				Some(name) => String::from(name),
				None => String::from(
					text_of(&link)
						.trim_end_matches(',')
						.trim()
						.trim_start_matches('#'),
				),
			};
			if !name.is_empty() && !tags.contains(&name) {
				tags.push(name);
			}
		}
	}

	let description = html
		.select_first("main p.text-medium")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	let cover = html
		.select_first("main img.object-cover")
		.and_then(|el: Element| el.attr("src"))
		.or_else(|| {
			html.select_first("meta[property='og:image']")
				.and_then(|el: Element| el.attr("content"))
		})
		.filter(|src: &String| src.starts_with("http"));

	let manga = Manga {
		key: String::from(slug),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		url: Some(manga_url(slug)),
		tags: (!tags.is_empty()).then_some(tags),
		status,
		content_rating: ContentRating::NSFW,
		// Korean titles are long vertical strips. The reading direction of the others
		// has not been checked, so they keep the app default.
		viewer: if is_hanman {
			Viewer::Webtoon
		} else {
			Viewer::Unknown
		},
		..Default::default()
	};
	(manga, mid)
}

/// `第118話` → 118. `最終話` and other titles without a number give `None`.
pub fn chapter_number(title: &str) -> Option<f32> {
	let rest = title.trim().strip_prefix('第')?;
	let end = rest
		.find(|c: char| !(c.is_ascii_digit() || c == '.'))
		.unwrap_or(rest.len());
	rest[..end].parse::<f32>().ok()
}

/// `/manga/get?mid=&mode=all`, oldest first on the site, returned newest first. The key
/// is `<mid>/<chapter id>` because the page list needs both and only gets the chapter.
/// The site's dates are `Apr 7` with no year (the import date of the whole work), so
/// none is set rather than a guessed year.
pub fn parse_chapters(html: &Document, mid: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();
	if let Some(links) = html.select(".chapteritem a[data-cs]") {
		for link in links {
			let Some(cs) = link.attr("data-cs").filter(|c: &String| !c.trim().is_empty()) else {
				continue;
			};
			let title = link
				.attr("data-ct")
				.map(|t: String| String::from(t.trim()))
				.filter(|t: &String| !t.is_empty())
				.or_else(|| link.select_first(".chaptertitle").map(|el: Element| text_of(&el)));
			let url = link
				.attr("href")
				.map(|href: String| format!("{BASE_URL}/{}", site_path(&href)));
			chapters.push(Chapter {
				key: format!("{mid}/{}", cs.trim()),
				chapter_number: title.as_deref().and_then(chapter_number),
				title,
				url,
				..Default::default()
			});
		}
	}
	chapters.reverse();
	chapters
}

/// `/chapter/getcontent`: the first image has a real `src`, the rest are lazy with a
/// `data:` placeholder in `src` and the address in `data-src`. Only direct children of
/// the page wrappers count: each lazy image is repeated inside a `<noscript>`, and an ad
/// banner sits in `#chapcontent` too.
pub fn parse_pages(html: &Document) -> Vec<Page> {
	let mut pages: Vec<Page> = Vec::new();
	if let Some(images) = html.select("#chapcontent > div > img") {
		for img in images {
			let url = img
				.attr("data-src")
				.filter(|u: &String| u.starts_with("http"))
				.or_else(|| img.attr("src").filter(|u: &String| u.starts_with("http")));
			if let Some(url) = url {
				let url = String::from(url.trim());
				if pages.iter().any(|p: &Page| p.content == PageContent::url(url.clone())) {
					continue;
				}
				pages.push(Page {
					content: PageContent::url(url),
					..Default::default()
				});
			}
		}
	}
	pages
}

/// A reader page (`/manga/<slug>/<x>`) names its chapter only in `#chapterContent`.
pub fn reader_chapter_key(html: &Document) -> Option<String> {
	let el = html.select_first("#chapterContent")?;
	let ms = el.attr("data-ms").filter(|v: &String| !v.is_empty())?;
	let cs = el.attr("data-cs").filter(|v: &String| !v.is_empty())?;
	Some(format!("{ms}/{cs}"))
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
	fn reads_slugs_from_every_link_shape() {
		assert_eq!(
			slug_from_href("https://18mh.org/manga/pengyoudejiejie").as_deref(),
			Some("pengyoudejiejie")
		);
		assert_eq!(
			slug_from_href("/manga/jimuyujijie-827").as_deref(),
			Some("jimuyujijie-827")
		);
		assert_eq!(
			slug_from_href("https://18mh.org/manga/toutouai/144-3-0").as_deref(),
			Some("toutouai")
		);
		assert_eq!(slug_from_href("/manga-tag/yuwang"), None);
		assert_eq!(slug_from_href("/manga/"), None);
	}

	#[aidoku_test]
	fn maps_filter_ids_and_tag_names() {
		assert_eq!(category_path("manga-tag/yuwang"), Some("manga-tag/yuwang"));
		assert_eq!(category_path("韓漫"), Some("manga-genre/hanman"));
		assert_eq!(category_path("校園"), Some("manga-tag/xiaoyuan"));
		assert_eq!(category_path("不存在"), None);
		assert_eq!(category_path(""), None);
	}

	#[aidoku_test]
	fn categories_match_filters_json() {
		let json = include_str!("../res/filters.json");
		for (id, name) in CATEGORIES {
			assert!(json.contains(&format!("\"{id}\"")), "{id}");
			assert!(json.contains(&format!("\"{name}\"")), "{name}");
		}
	}

	#[aidoku_test]
	fn parses_a_list_page_with_a_next_page() {
		let result = parse_manga_list(doc(include_str!("fixtures/list.html")));
		assert_eq!(result.entries.len(), 18);
		assert!(result.has_next_page);
		let first = &result.entries[0];
		assert_eq!(first.key, "pengyoudejiejie");
		assert_eq!(first.title, "朋友的姐姐");
		assert!(first.cover.as_deref().is_some_and(|c: &str| c.starts_with("https://host-cover.")));
	}

	#[aidoku_test]
	fn the_last_list_page_has_no_next_page() {
		let result = parse_manga_list(doc(include_str!("fixtures/list_last.html")));
		assert_eq!(result.entries.len(), 16);
		assert!(!result.has_next_page);
	}

	#[aidoku_test]
	fn reads_details_with_mangachapters() {
		let (manga, mid) = parse_details(&doc(include_str!("fixtures/detail_pengyoudejiejie.html")), "pengyoudejiejie");
		assert_eq!(mid.as_deref(), Some("16"));
		assert_eq!(manga.title, "朋友的姐姐");
		assert_eq!(manga.status, MangaStatus::Completed);
		assert_eq!(manga.viewer, Viewer::Webtoon);
		assert_eq!(
			manga.authors,
			Some(Vec::from([String::from("Sean Kim"), String::from("鍾普林")]))
		);
		assert_eq!(
			manga.tags,
			Some(Vec::from([String::from("韓漫"), String::from("慾望"), String::from("推薦")]))
		);
		assert!(manga.description.as_deref().is_some_and(|d: &str| d.starts_with("喜歡偷看")));
		assert_eq!(author_slug("鍾普林").as_deref(), Some("zhongpulin"));
	}

	#[aidoku_test]
	fn reads_another_works_details() {
		let (manga, mid) = parse_details(&doc(include_str!("fixtures/detail_toutouai.html")), "toutouai");
		assert_eq!(mid.as_deref(), Some("144"));
		assert_eq!(manga.title, "偷偷愛");
		assert_eq!(manga.authors, Some(Vec::from([String::from("90's magazine")])));
		assert_eq!(
			manga.tags,
			Some(Vec::from([
				String::from("韓漫"),
				String::from("熱門漫畫"),
				String::from("同居"),
				String::from("劇情"),
				String::from("純情警察"),
				String::from("大尺度"),
				String::from("戲劇"),
			]))
		);
	}

	#[aidoku_test]
	fn falls_back_to_mangachapters_for_the_id() {
		let html = doc(r#"<main><div id="mangachapters" data-mid="7"></div></main>"#);
		assert_eq!(parse_details(&html, "x").1.as_deref(), Some("7"));
		let html = doc("<main></main>");
		assert_eq!(parse_details(&html, "x").1, None);
	}

	#[aidoku_test]
	fn lists_chapters_newest_first() {
		let chapters = parse_chapters(&doc(include_str!("fixtures/chapters.html")), "16");
		assert_eq!(chapters.len(), 42);
		assert_eq!(chapters[0].key, "16/33470");
		assert_eq!(chapters[0].title.as_deref(), Some("最終話"));
		assert_eq!(chapters[0].chapter_number, None);
		let first = chapters.last().expect("first chapter");
		assert_eq!(first.key, "16/33413");
		assert_eq!(first.chapter_number, Some(1.0));
		assert_eq!(
			first.url.as_deref(),
			Some("https://18mh.org/manga/pengyoudejiejie/16-16059-0")
		);
	}

	#[aidoku_test]
	fn reads_every_page_image() {
		let pages = parse_pages(&doc(include_str!("fixtures/content.html")));
		assert_eq!(pages.len(), 40);
		assert_eq!(
			pages[0].content,
			PageContent::url("https://s3-nl-01.mangabuddy.in/mxs/pengyoudejiejie/0/0_405137.webp")
		);
		assert_eq!(
			pages[1].content,
			PageContent::url("https://s3-nl-01.mangabuddy.in/mxs/pengyoudejiejie/0/1_405141.webp")
		);
	}

	#[aidoku_test]
	fn finds_the_chapter_on_a_reader_page() {
		assert_eq!(
			reader_chapter_key(&doc(include_str!("fixtures/reader.html"))).as_deref(),
			Some("16/33413")
		);
	}

	#[aidoku_test]
	fn numbers_chapters_from_titles() {
		assert_eq!(chapter_number("第118話"), Some(118.0));
		assert_eq!(chapter_number("第1.5話"), Some(1.5));
		assert_eq!(chapter_number("最終話"), None);
		assert_eq!(chapter_number("後記"), None);
	}
}
