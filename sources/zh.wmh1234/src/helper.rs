use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		html::{Document, Element},
		net::Request,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Result, Viewer,
};
use base64::{
	alphabet,
	engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
	Engine,
};

pub const BASE_URL: &str = "https://m.wmh1234.com";

/// Where `/go/<token>` sends the reader (a script redirect, 2026-10-10). Going there
/// directly saves a request per chapter; if the site moves its reader, chapters fail here.
pub const READER_URL: &str = "https://reader.hqread.cc";

/// The listings: id and the catalogue order behind it. `/custom/top` lists the same 30
/// works as the first pages of `order/hits`, so the paged catalogue replaces it.
pub const LISTINGS: [(&str, &str); 3] = [("update", "addtime"), ("hits", "hits"), ("new", "id")];

/// Where the site sends a work that does not exist, as zh.manwang's CMS does.
pub const DELISTED_PATH: &str = "/err/comic";

/// Any page but a details page. A non-2xx answer is an error, so an error page is never
/// read as an empty list.
pub fn fetch_html(url: &str) -> Result<Document> {
	let response = Request::get(url)?.send()?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[wmh1234] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
}

/// The details page, or `None` when the site has removed the work. Removal needs positive
/// evidence: a maintenance or challenge page answered with 200 must not mark the library's
/// works as removed, so any other page without a title is an error.
pub fn fetch_details_page(key: &str) -> Result<Option<Document>> {
	let url = manga_url(key);
	let response = Request::get(&url)?.send()?;
	if response
		.get_url()
		.is_some_and(|url: String| url.contains(DELISTED_PATH))
	{
		return Ok(None);
	}
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[wmh1234] HTTP {status} for {url}");
	}
	let html = response.get_html()?;
	if html.select_first("#mintWorkTitle").is_some() {
		Ok(Some(html))
	} else if is_delisted_page(&html) {
		Ok(None)
	} else {
		bail!("[wmh1234] no details on {url}")
	}
}

/// The removal page, recognised by its text in case the final address is not reported:
/// 「很遗憾，该内容不存在或已被删除。」 (2026-10-10).
pub fn is_delisted_page(html: &Document) -> bool {
	html.select_first("#mintWorkTitle").is_none()
		&& html
			.select_first("body")
			.and_then(|body: Element| body.text())
			.is_some_and(|text: String| text.contains("该内容不存在"))
}

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/comic/{key}.html")
}

/// The chapter link the site itself shows.
pub fn chapter_url(token: &str) -> String {
	format!("{BASE_URL}/go/{token}")
}

pub fn reader_url(token: &str) -> String {
	format!("{READER_URL}/r/{token}")
}

/// The catalogue. Every segment combines with the others; `order` defaults to clicks.
pub fn browse_url(tag: Option<&str>, status: Option<&str>, order: Option<&str>, page: i32) -> String {
	let mut url = format!("{BASE_URL}/category");
	if let Some(tag) = tag {
		url.push_str(&format!("/tags/{tag}"));
	}
	if let Some(status) = status {
		url.push_str(&format!("/finish/{status}"));
	}
	if let Some(order) = order {
		url.push_str(&format!("/order/{order}"));
	}
	url.push_str(&format!("/page/{page}"));
	url
}

/// A listing's address; `None` for unknown ids.
pub fn listing_url(id: &str, page: i32) -> Option<String> {
	LISTINGS
		.iter()
		.find(|(listing, _)| *listing == id)
		.map(|(_, order)| browse_url(None, None, Some(order), page))
}

/// `/search/<keyword>/<page>`; the first page has no number. Matches titles and authors.
pub fn search_url(query: &str, page: i32) -> String {
	let mut url = format!("{BASE_URL}/search/{}", encode_uri_component(query.trim()));
	if page > 1 {
		url.push_str(&format!("/{page}"));
	}
	url
}

/// `/comic/47315.html` (relative or absolute) → `47315`.
pub fn comic_id(href: &str) -> Option<String> {
	let (_, rest) = href.split_once("/comic/")?;
	let (id, _) = rest.split_once(".html")?;
	(!id.is_empty() && id.chars().all(|c: char| c.is_ascii_digit())).then(|| String::from(id))
}

/// The token after `/go/` or `/r/`: the details page links `/go/`, the reader `/r/`.
pub fn chapter_token(href: &str) -> Option<String> {
	let rest = href
		.split_once("/go/")
		.or_else(|| href.split_once("/r/"))
		.map(|(_, rest)| rest)?;
	let token: String = rest
		.chars()
		.take_while(|c: &char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '='))
		.collect();
	let next = rest[token.len()..].chars().next();
	(!token.is_empty() && matches!(next, None | Some('/' | '?' | '#'))).then_some(token)
}

/// A chapter token is base64 of `<work id>-<chapter id>-<hash>`; the hash is the
/// server's, so the token is the chapter key and the work id is read back from it.
pub fn token_comic_id(token: &str) -> Option<String> {
	let engine = GeneralPurpose::new(
		&alphabet::STANDARD,
		GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
	);
	let normalised: String = token
		.chars()
		.map(|c: char| match c {
			'-' => '+',
			'_' => '/',
			c => c,
		})
		.collect();
	let bytes = engine.decode(normalised).ok()?;
	let text = String::from_utf8(bytes).ok()?;
	let id = text.split('-').next()?;
	(!id.is_empty() && id.chars().all(|c: char| c.is_ascii_digit())).then(|| String::from(id))
}

fn text_of(el: &Element) -> String {
	el.text().map(|t: String| String::from(t.trim())).unwrap_or_default()
}

/// Image addresses carry `?st=<signature>&e=<expiry>`, valid a day or two, and answer 410
/// once expired. The same path without the query serves the same file (2026-10-10), so
/// covers drop it and stay valid in the library. Pages keep it: they are fetched afresh
/// for every read (the user's choice).
pub fn cover_url(src: &str) -> String {
	let src = src.trim();
	match src.split_once('?') {
		Some((path, query)) if query.starts_with("st=") || query.contains("&st=") => String::from(path),
		_ => String::from(src),
	}
}

/// Tags that raise a work's rating. The site has no adult flag, so the tags are the only
/// evidence; a work without one of them is `Safe`. Chosen by the user (2026-10-10) after
/// zh.komiic and zh.manwang. Each name is in `tags::TAGS`.
pub const NSFW_TAGS: [&str; 1] = ["SM"];
pub const SUGGESTIVE_TAGS: [&str; 6] = ["耽美", "百合", "后宫", "蔷薇", "ABO", "多攻"];

pub fn content_rating<S: AsRef<str>>(tags: &[S]) -> ContentRating {
	if tags.iter().any(|t: &S| NSFW_TAGS.contains(&t.as_ref())) {
		ContentRating::NSFW
	} else if tags.iter().any(|t: &S| SUGGESTIVE_TAGS.contains(&t.as_ref())) {
		ContentRating::Suggestive
	} else {
		ContentRating::Safe
	}
}

/// Japanese works are page-sized images (900x1344); Korean and most Chinese ones are long
/// strips (690x2000). Chosen by the user (2026-10-10), as zh.bakamh does by category.
pub fn viewer<S: AsRef<str>>(tags: &[S]) -> Viewer {
	if tags.iter().any(|t: &S| t.as_ref() == "日漫") {
		Viewer::RightToLeft
	} else {
		Viewer::Webtoon
	}
}

/// Whether the pager names a page after the current one. Its last-page and next-page
/// buttons point at the current page on the last page, so the numbered links decide.
fn has_next_page(html: &Document) -> bool {
	let Some(current) = html
		.select_first(".pagination-wrapper a.on")
		.and_then(|a: Element| text_of(&a).parse::<i32>().ok())
	else {
		return false;
	};
	html.select(".pagination-wrapper a")
		.into_iter()
		.flatten()
		.filter_map(|a: Element| text_of(&a).parse::<i32>().ok())
		.any(|n: i32| n > current)
}

/// The catalogue and search pages. Only the first `.comic-grid` holds results: the search
/// page appends six recommended works in a second grid, also on pages past the last.
pub fn parse_manga_list(html: &Document) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(grid) = html.select_first(".comic-grid") {
		for card in grid.select("article.comic-card").into_iter().flatten() {
			let Some(key) = card
				.select_first("a[href]")
				.and_then(|a: Element| a.attr("href"))
				.and_then(|href: String| comic_id(&href))
			else {
				continue;
			};
			let title = card
				.select_first(".comic-card__title")
				.map(|el: Element| text_of(&el))
				.unwrap_or_default();
			if title.is_empty() || entries.iter().any(|m: &Manga| m.key == key) {
				continue;
			}
			let cover = card
				.select_first("img[data-src]")
				.and_then(|img: Element| img.attr("data-src"))
				.map(|src: String| cover_url(&src));
			entries.push(Manga {
				url: Some(manga_url(&key)),
				key,
				title,
				cover,
				// Cards name no tags; the details page sets both.
				content_rating: ContentRating::Safe,
				viewer: Viewer::Webtoon,
				..Default::default()
			});
		}
	}
	let has_next_page = !entries.is_empty() && has_next_page(html);
	MangaPageResult {
		entries,
		has_next_page,
	}
}

pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first("#mintWorkTitle")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first("#mintWorkCover")
		.and_then(|el: Element| el.attr("src"))
		.map(|src: String| cover_url(&src));

	// `.mint-work-info` has `<p>作者,作者 著</p>` and `<p>题材 题材 <span class="mint-tag">状态</span></p>`.
	let mut authors: Vec<String> = Vec::new();
	let mut tags: Vec<String> = Vec::new();
	let mut status = MangaStatus::Unknown;
	for p in html.select(".mint-work-info p").into_iter().flatten() {
		if p.has_class("mint-work-meta") || p.has_class("mint-last-chapter") {
			continue;
		}
		let text = text_of(&p);
		if let Some(state) = p.select_first(".mint-tag").map(|el: Element| text_of(&el)) {
			status = match state.as_str() {
				s if s.contains("完结") => MangaStatus::Completed,
				s if s.contains("连载") => MangaStatus::Ongoing,
				_ => MangaStatus::Unknown,
			};
			for name in text.split_whitespace() {
				if name != state && !tags.iter().any(|t: &String| t == name) {
					tags.push(String::from(name));
				}
			}
		} else if let Some(names) = text.strip_suffix('著') {
			authors = names
				.split([',', '，', '、', '&', '/'])
				.map(|n: &str| String::from(n.trim()))
				.filter(|n: &String| !n.is_empty())
				.collect();
		}
	}
	let description = html
		.select_first("#mintIntroPanel div")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty());
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		content_rating: content_rating(&tags),
		viewer: viewer(&tags),
		tags: (!tags.is_empty()).then_some(tags),
		status,
		..Default::default()
	})
}

/// `第40话 暴风雨前夕` → 40; early chapters are bare numbers (`9`). `特别篇` gets none.
pub fn chapter_number(title: &str) -> Option<f32> {
	let title = title.trim();
	if let Some(rest) = title.strip_prefix('第') {
		let end = rest.find(['话', '話', '回', '章'])?;
		return rest[..end].trim().parse::<f32>().ok();
	}
	let number = title.split_whitespace().next()?;
	number.parse::<f32>().ok()
}

/// The details page lists every chapter, newest first, as the app wants. Titles repeat
/// (two `第35话`); the site's order and titles are kept as they are.
pub fn parse_chapters(html: &Document) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();
	for link in html
		.select(".mint-chapter-grid a[data-chapter-id]")
		.into_iter()
		.flatten()
	{
		let Some(key) = link.attr("href").and_then(|href: String| chapter_token(&href)) else {
			continue;
		};
		if chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		let title = text_of(&link);
		chapters.push(Chapter {
			url: Some(chapter_url(&key)),
			chapter_number: chapter_number(&title),
			title: (!title.is_empty()).then_some(title),
			key,
			..Default::default()
		});
	}
	chapters
}

/// The reader's images in order. The page names the next lazily loaded image in
/// `data-src`; `src` is a placeholder.
pub fn parse_pages(html: &Document) -> Vec<String> {
	html.select("img.reader-image[data-src]")
		.into_iter()
		.flatten()
		.filter_map(|img: Element| img.attr("data-src"))
		.map(|src: String| String::from(src.trim()))
		.filter(|src: &String| src.starts_with("http"))
		.collect()
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
	fn builds_addresses() {
		assert_eq!(browse_url(None, None, None, 1), "https://m.wmh1234.com/category/page/1");
		assert_eq!(
			browse_url(Some("17"), Some("2"), Some("addtime"), 3),
			"https://m.wmh1234.com/category/tags/17/finish/2/order/addtime/page/3"
		);
		assert_eq!(
			listing_url("new", 2).as_deref(),
			Some("https://m.wmh1234.com/category/order/id/page/2")
		);
		assert_eq!(listing_url("top", 1), None);
		assert_eq!(search_url(" 恋爱 ", 1), "https://m.wmh1234.com/search/%E6%81%8B%E7%88%B1");
		assert_eq!(search_url("恋爱", 2), "https://m.wmh1234.com/search/%E6%81%8B%E7%88%B1/2");
		assert_eq!(manga_url("47315"), "https://m.wmh1234.com/comic/47315.html");
		assert_eq!(
			reader_url("NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh"),
			"https://reader.hqread.cc/r/NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh"
		);
	}

	#[aidoku_test]
	fn reads_ids() {
		assert_eq!(comic_id("/comic/47315.html").as_deref(), Some("47315"));
		assert_eq!(comic_id("https://m.wmh1234.com/comic/47315.html?x=1").as_deref(), Some("47315"));
		assert_eq!(comic_id("/comic/12a.html"), None);
		assert_eq!(comic_id("/category/tags/17"), None);
		assert_eq!(
			chapter_token("/go/NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh").as_deref(),
			Some("NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh")
		);
		assert_eq!(
			chapter_token("https://reader.hqread.cc/r/NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh#top").as_deref(),
			Some("NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh")
		);
		assert_eq!(chapter_token("/go/"), None);
		assert_eq!(token_comic_id("NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh").as_deref(), Some("47315"));
		// 19天's first chapter: 31 characters, so the padding is missing.
		assert_eq!(token_comic_id("MTY4MzgtNjMzNzk3LTdlN2RlMDIyMGI").as_deref(), Some("16838"));
		assert_eq!(token_comic_id("not base64!"), None);
	}

	#[aidoku_test]
	fn drops_cover_signatures() {
		assert_eq!(
			cover_url("https://wmh1234.wszwhg.net/fmtu/47315/b4.jpg?st=4d39&e=1791763200"),
			"https://wmh1234.wszwhg.net/fmtu/47315/b4.jpg"
		);
		assert_eq!(cover_url("https://example.com/a.jpg?v=2"), "https://example.com/a.jpg?v=2");
		assert_eq!(cover_url("https://example.com/a.jpg"), "https://example.com/a.jpg");
	}

	#[aidoku_test]
	fn parses_listing_pages() {
		for (fixture, count, next) in [
			(include_str!("fixtures/category.html"), 21, true),
			(include_str!("fixtures/category_last.html"), 6, false),
			(include_str!("fixtures/search.html"), 12, true),
			(include_str!("fixtures/search_last.html"), 6, false),
			(include_str!("fixtures/search_past.html"), 0, false),
		] {
			let result = parse_manga_list(&doc(fixture));
			assert_eq!((result.entries.len(), result.has_next_page), (count, next));
			assert!(result.entries.iter().all(|m: &Manga| m
				.cover
				.as_deref()
				.is_some_and(|c: &str| c.starts_with("https://") && !c.contains('?'))));
		}
		let search = parse_manga_list(&doc(include_str!("fixtures/search.html")));
		assert_eq!(search.entries[0].key, "9487");
		assert_eq!(search.entries[0].title, "恋爱的无休止境");
		// The recommended works after the results are not part of them.
		assert!(!search.entries.iter().any(|m: &Manga| m.title == "天丛云"));
	}

	#[aidoku_test]
	fn recognises_the_removal_page() {
		assert!(is_delisted_page(&doc(include_str!("fixtures/delisted.html"))));
		assert!(!is_delisted_page(&doc(include_str!("fixtures/manga.html"))));
		// A page that is neither, such as the reader's 503, is not a removal.
		assert!(!is_delisted_page(&doc(include_str!("fixtures/syncing.html"))));
	}

	#[aidoku_test]
	fn rates_by_tags() {
		for tag in NSFW_TAGS.iter().chain(SUGGESTIVE_TAGS.iter()) {
			assert!(crate::tags::tag_id(tag).is_some(), "{tag}");
		}
		assert!(crate::tags::tag_id("日漫").is_some());
		assert_eq!(content_rating::<&str>(&[]), ContentRating::Safe);
		assert_eq!(content_rating(&["恋爱", "校园"]), ContentRating::Safe);
		assert_eq!(content_rating(&["耽美", "SM"]), ContentRating::NSFW);
		assert_eq!(content_rating(&["百合"]), ContentRating::Suggestive);
		assert_eq!(viewer(&["日漫", "热血"]), Viewer::RightToLeft);
		assert_eq!(viewer(&["韩漫"]), Viewer::Webtoon);
	}

	#[aidoku_test]
	fn reads_the_details_page() {
		let manga = parse_details(&doc(include_str!("fixtures/manga.html")), "47315").expect("details");
		assert_eq!(manga.title, "恋爱四选一");
		assert_eq!(
			manga.authors,
			Some(Vec::from([String::from("Neip"), String::from("Team")]))
		);
		assert_eq!(
			manga.tags,
			Some(Vec::from([String::from("恋爱"), String::from("校园")]))
		);
		assert_eq!(manga.status, MangaStatus::Ongoing);
		assert_eq!(manga.viewer, Viewer::Webtoon);
		assert_eq!(
			manga.cover.as_deref(),
			Some("https://wmh1234.wszwhg.net/fmtu/47315/b44744c83d8e4ffdb8c244826de5c7af.jpg")
		);
		assert!(manga.description.as_deref().unwrap_or_default().starts_with("认为恋爱是奢侈"));

		let jp = parse_details(&doc(include_str!("fixtures/manga_jp.html")), "26111").expect("details");
		assert_eq!(jp.title, "蓝色监狱");
		assert_eq!(jp.authors.as_ref().map(|a: &Vec<String>| a.len()), Some(4));
		assert_eq!(jp.viewer, Viewer::RightToLeft);
		assert_eq!(jp.content_rating, ContentRating::Safe);

		let empty = parse_details(&doc(include_str!("fixtures/manga_empty.html")), "1302").expect("details");
		assert_eq!(empty.title, "降妖伏魔录");
		assert_eq!(empty.status, MangaStatus::Completed);
		assert!(parse_chapters(&doc(include_str!("fixtures/manga_empty.html"))).is_empty());
	}

	#[aidoku_test]
	fn lists_chapters_newest_first() {
		let chapters = parse_chapters(&doc(include_str!("fixtures/manga.html")));
		assert_eq!(chapters.len(), 45);
		assert_eq!(chapters[0].key, "NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh");
		assert_eq!(chapters[0].title.as_deref(), Some("第40话 暴风雨前夕"));
		assert_eq!(chapters[0].chapter_number, Some(40.0));
		assert_eq!(
			chapters[0].url.as_deref(),
			Some("https://m.wmh1234.com/go/NDczMTUtMzkyNjEwNi1hNWNlMWU3YTZh")
		);
		assert_eq!(chapters[44].title.as_deref(), Some("1"));
		assert_eq!(chapters[44].chapter_number, Some(1.0));
		assert_eq!(chapter_number("特别篇"), None);
		assert_eq!(chapter_number("第154话 (AI翻译)"), Some(154.0));
		assert_eq!(chapter_number("最终话 来战吧！怪物"), None);
	}

	#[aidoku_test]
	fn reads_the_reader_page() {
		let pages = parse_pages(&doc(include_str!("fixtures/reader.html")));
		assert_eq!(pages.len(), 159);
		assert!(pages[0].starts_with("https://wmh1234.wszwhg.net/kantu/"));
		// Page images keep their signature.
		assert!(pages[0].contains("?st="));
		assert!(parse_pages(&doc(include_str!("fixtures/syncing.html"))).is_empty());
	}
}
