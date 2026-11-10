use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		html::{Document, Element},
		net::{Request, Response},
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Result, Viewer,
};

pub const BASE_URL: &str = "https://www.manwang.net";

/// The page image host answers 403 without a manwang `Referer`.
pub const REFERER: &str = "https://www.manwang.net/";

/// The rank and update boards: listing id and path. Each is a single page of 30.
pub const BOARDS: [(&str, &str); 5] = [
	("update", "/custom/update"),
	("hot", "/custom/hot"),
	("month", "/custom/month"),
	("week", "/custom/week"),
	("day", "/custom/day"),
];

/// Kept apart from `fetch_html` so the home can send its rows together.
pub fn request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?)
}

/// A non-2xx answer is an error, so an error page is never read as an empty list.
pub fn fetch_html(url: &str) -> Result<Document> {
	read_html(request(url)?.send()?, url)
}

/// `fetch_html`'s status check, shared with the home's batched rows.
pub fn read_html(response: Response, url: &str) -> Result<Document> {
	check_status(&response, url)?;
	Ok(response.get_html()?)
}

fn check_status(response: &Response, url: &str) -> Result<()> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[manwang] HTTP {status} for {url}");
	}
	Ok(())
}

/// Where the site sends a removed work: `/book/<id>` answers 302 to this page, which
/// says "该漫画不存在或章节已被删除". The boards and catalogue keep listing such works.
pub const DELISTED_PATH: &str = "/err/comic";

/// The details page, or `None` when the site has removed the work.
pub fn fetch_details_page(key: &str) -> Result<Option<Document>> {
	let url = manga_url(key);
	let response = Request::get(&url)?.send()?;
	// The removal address is decided before the status, so a removal page that stops
	// answering 200 still marks the work removed instead of failing every removed work.
	if response
		.get_url()
		.is_some_and(|url: String| url.contains(DELISTED_PATH))
	{
		return Ok(None);
	}
	check_status(&response, &url)?;
	let html = response.get_html()?;
	Ok((!is_delisted_page(&html)).then_some(html))
}

/// The removal page, recognised by its text in case the final address is not reported.
pub fn is_delisted_page(html: &Document) -> bool {
	html.select_first("h1.detail-title").is_none()
		&& html
			.select_first("body")
			.and_then(|body: Element| body.text())
			.is_some_and(|text: String| text.contains("该漫画不存在"))
}

pub fn fetch_text(url: &str) -> Result<String> {
	let response = Request::get(url)?.send()?;
	check_status(&response, url)?;
	response.get_string()
}

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/book/{key}")
}

pub fn chapter_url(manga_key: &str, key: &str) -> String {
	format!("{BASE_URL}/chapter/{manga_key}-{key}")
}

/// A board's address; `None` for unknown ids.
pub fn board_url(id: &str) -> Option<String> {
	BOARDS
		.iter()
		.find(|(board, _)| *board == id)
		.map(|(_, path)| format!("{BASE_URL}{path}"))
}

/// The catalogue. Without `order` it sorts by clicks; `addtime` sorts by update. The
/// site's region, format and status filters answer (almost) nothing, so they are not used.
pub fn browse_url(tag: Option<&str>, by_update: bool, page: i32) -> String {
	let mut url = format!("{BASE_URL}/category");
	if let Some(tag) = tag {
		url.push_str(&format!("/tags/{tag}"));
	}
	if by_update {
		url.push_str("/order/addtime");
	}
	url.push_str(&format!("/page/{page}"));
	url
}

/// The search has no usable further pages: its pager links carry no page number.
pub fn search_url(query: &str) -> String {
	format!(
		"{BASE_URL}/index.php/search?key={}",
		encode_uri_component(query.trim())
	)
}

/// The number after `/<kind>/` in a link: `/book/503735` → `503735`.
pub fn id_after(href: &str, kind: &str) -> Option<String> {
	let (_, rest) = href.split_once(&format!("/{kind}/"))?;
	let id: String = rest
		.chars()
		.take_while(|c: &char| c.is_ascii_digit())
		.collect();
	let next = rest[id.len()..].chars().next();
	(!id.is_empty() && matches!(next, None | Some('/' | '?' | '#'))).then_some(id)
}

/// `/chapter/503735-185808` → (`503735`, `185808`).
pub fn chapter_keys(href: &str) -> Option<(String, String)> {
	let (_, rest) = href.split_once("/chapter/")?;
	let (manga, rest) = rest.split_once('-')?;
	let chapter: String = rest
		.chars()
		.take_while(|c: &char| c.is_ascii_digit())
		.collect();
	let next = rest[chapter.len()..].chars().next();
	let valid = !manga.is_empty()
		&& manga.chars().all(|c: char| c.is_ascii_digit())
		&& !chapter.is_empty()
		&& matches!(next, None | Some('/' | '?' | '#'));
	valid.then(|| (String::from(manga), chapter))
}

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// The site writes a few covers as `http://www.manwang.net/...`, which only redirects.
fn cover_url(src: String) -> String {
	match src.strip_prefix("http://") {
		Some(rest) => format!("https://{rest}"),
		None => src,
	}
}

/// Tags that raise a work's rating. The site has no adult flag, so the tags are the
/// only evidence; a work without one of them is `Safe`. The lists follow zh.komiic2 and
/// were chosen by the user (2026-10-09). Each name is in `tags::TAGS`.
pub const NSFW_TAGS: [&str; 4] = ["本子", "限制", "伦理", "人妻"];
pub const SUGGESTIVE_TAGS: [&str; 8] = [
	"橘里橘气",
	"轻橘",
	"橘味",
	"后宫",
	"耽美",
	"百合",
	"血腥",
	"猎奇",
];

pub fn content_rating<S: AsRef<str>>(tags: &[S]) -> ContentRating {
	if tags.iter().any(|t: &S| NSFW_TAGS.contains(&t.as_ref())) {
		ContentRating::NSFW
	} else if tags
		.iter()
		.any(|t: &S| SUGGESTIVE_TAGS.contains(&t.as_ref()))
	{
		ContentRating::Suggestive
	} else {
		ContentRating::Safe
	}
}

/// The highest page the pager's last-page link names; 1 when it names none.
fn last_page(html: &Document) -> i32 {
	html.select(".mod-page a.pagelink_a")
		.into_iter()
		.flatten()
		.find(|a: &Element| text_of(a) == "尾页")
		.and_then(|a: Element| a.attr("href"))
		.and_then(|href: String| {
			let (_, number) = href.rsplit_once("/page/")?;
			number.parse::<i32>().ok()
		})
		.unwrap_or(1)
}

/// The catalogue, board, tag and search pages: `a.comic-item` cards.
pub fn parse_manga_list(html: &Document, page: i32) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	for card in html.select("a.comic-item").into_iter().flatten() {
		let Some(key) = card
			.attr("href")
			.and_then(|href: String| id_after(&href, "book"))
		else {
			continue;
		};
		let title = card
			.select_first("h2")
			.map(|el: Element| text_of(&el))
			.unwrap_or_default();
		if title.is_empty() || entries.iter().any(|m: &Manga| m.key == key) {
			continue;
		}
		let cover = card
			.select_first("img[data-src]")
			.and_then(|img: Element| img.attr("data-src"))
			.map(cover_url);
		// Cards name their tags as plain text separated by spaces.
		let tags: Vec<String> = card
			.select_first(".tag-list")
			.map(|el: Element| text_of(&el))
			.unwrap_or_default()
			.split_whitespace()
			.map(String::from)
			.collect();
		entries.push(Manga {
			url: Some(manga_url(&key)),
			key,
			title,
			cover,
			content_rating: content_rating(&tags),
			viewer: Viewer::Webtoon,
			..Default::default()
		});
	}
	let has_next_page = !entries.is_empty() && last_page(html) > page;
	MangaPageResult {
		entries,
		has_next_page,
	}
}

fn meta(html: &Document, property: &str) -> Option<String> {
	html.select_first(format!("meta[property='{property}']"))
		.and_then(|el: Element| el.attr("content"))
		.map(|c: String| String::from(c.trim()))
		.filter(|c: &String| !c.is_empty())
}

pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first("h1.detail-title")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())
		.or_else(|| meta(html, "og:novel:book_name"))?;
	let cover = html
		.select_first(".banner-img img[data-src]")
		.and_then(|el: Element| el.attr("data-src"))
		.or_else(|| meta(html, "og:image"))
		.map(cover_url);
	let authors: Vec<String> = html
		.select_first(".detail-info p.author")
		.map(|el: Element| text_of(&el))
		.or_else(|| meta(html, "og:novel:author"))
		.map(|names: String| {
			names
				.split(['&', '/', '、', ','])
				.map(|n: &str| String::from(n.trim()))
				.filter(|n: &String| !n.is_empty())
				.collect()
		})
		.unwrap_or_default();
	let status = match meta(html, "og:novel:status").as_deref() {
		Some(s) if s.contains("完结") => MangaStatus::Completed,
		Some(s) if s.contains("连载") => MangaStatus::Ongoing,
		_ => MangaStatus::Unknown,
	};
	let mut tags: Vec<String> = Vec::new();
	for link in html
		.select(".detail-info .tag-list a")
		.into_iter()
		.flatten()
	{
		let name = text_of(&link);
		if !name.is_empty() && !tags.contains(&name) {
			tags.push(name);
		}
	}
	let description = html
		.select_first(".detail-desc")
		.map(|el: Element| text_of(&el))
		.filter(|d: &String| !d.is_empty())
		.or_else(|| meta(html, "og:description"));
	let content_rating = content_rating(&tags);
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		tags: (!tags.is_empty()).then_some(tags),
		status,
		content_rating,
		// Most works are long strips cut into slices; a few use page-sized images.
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

/// `第529话 巨手（下）` → 529, `第1回 重生` → 1, `163话 渔翁得利！` → 163 (some titles
/// leave out the `第`). Titles without a number give `None`.
pub fn chapter_number(title: &str) -> Option<f32> {
	let title = title.trim();
	let rest = title.strip_prefix('第').unwrap_or(title);
	let end = rest.find(['话', '話', '回'])?;
	let number = rest[..end].trim();
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))
}

fn chinese_number(text: &str) -> Option<f32> {
	let digit = |c: char| "零一二三四五六七八九".chars().position(|d: char| d == c);
	let chars: Vec<char> = text.chars().collect();
	match chars.as_slice() {
		['十'] => Some(10.0),
		[c] => digit(*c).map(|d: usize| d as f32),
		['十', ones] => digit(*ones).map(|d: usize| 10.0 + d as f32),
		[tens, '十'] => digit(*tens).map(|d: usize| d as f32 * 10.0),
		[tens, '十', ones] => Some((digit(*tens)? * 10 + digit(*ones)?) as f32),
		_ => None,
	}
}

/// `第2季 第5话` → (Some(5), Some(2)), `第二季` → (None, Some(2)), `第3卷` → (None, Some(3)),
/// `第529话` → (Some(529), None). A leading volume, season, part or book is the volume, as
/// the official zh.copymanga does. A volume followed by text that is not a chapter
/// (`第3卷 後記`) is an appendix of that volume and gets none, as the official zh.zaimanhua
/// does (`is_extra`): it would share the volume's number with `第3卷` itself.
/// A list where no title has a number is numbered by position in `parse_chapters`.
pub fn numbers(title: &str) -> (Option<f32>, Option<f32>) {
	let title = title.trim();
	if let Some(rest) = volume_marker_rest(title) {
		if let Some(end) = rest.find(['卷', '季', '部', '冊', '册']) {
			let number = rest[..end].trim();
			if let Some(volume) = number
				.parse::<f32>()
				.ok()
				.or_else(|| chinese_number(number))
			{
				let unit = rest[end..].chars().next().unwrap_or('卷');
				let after = rest[end + unit.len_utf8()..].trim();
				let chapter = chapter_number(after);
				if unit == '卷' && !after.is_empty() && chapter.is_none() {
					return (None, None);
				}
				return (chapter, Some(volume));
			}
		}
	}
	(chapter_number(title), bare_volume(title))
}

/// The text a volume number is read from. `第3卷` gives `3卷`. The `第` may be left out
/// (`3卷 第5話`, `三季 第2話`) when the number is followed by a volume unit and then the end of
/// the title, a space or `第`, as in the official zh.boylove (`chapter_list/mod.rs:111`) and
/// zh.zaimanhua. `3卷後記`, `三部曲` and `一部分` have text right behind the unit, so they are
/// not volumes.
fn volume_marker_rest(title: &str) -> Option<&str> {
	if let Some(rest) = title.strip_prefix('第') {
		return Some(rest);
	}
	let end = title.find(['卷', '季', '部', '冊', '册'])?;
	// Without 第, a space between the number and the unit is not a volume, so the title
	// keeps the result it had before 第 became optional.
	let number = &title[..end];
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))?;
	let after = &title[end + title[end..].chars().next()?.len_utf8()..];
	(after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with('第'))
		.then_some(title)
}

/// `3卷` → 3 and `三卷` → 3. Only a title that is just the number and `卷` counts, as with
/// `第3卷`; `3卷后记` is an appendix and gives `None`.
fn bare_volume(title: &str) -> Option<f32> {
	let number = title.trim().strip_suffix('卷')?.trim();
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))
}

/// The details page lists every chapter, oldest first; the app wants newest first.
/// An entry without a usable id gives an empty list, which the caller treats as unreadable,
/// so the library is not handed a list with chapters missing.
pub fn parse_chapters(html: &Document, manga_key: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();
	for item in html
		.select("#j_chapter_list li[data-chapter]")
		.into_iter()
		.flatten()
	{
		let Some(key) = item
			.attr("data-chapter")
			.filter(|k: &String| !k.is_empty() && k.chars().all(|c: char| c.is_ascii_digit()))
		else {
			return Vec::new();
		};
		if chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		let title = item
			.select_first("a")
			.and_then(|a: Element| a.attr("title").or_else(|| a.text()))
			.map(|t: String| String::from(t.trim()))
			.unwrap_or_default();
		let (chapter_number, volume_number) = numbers(&title);
		chapters.push(Chapter {
			url: Some(chapter_url(manga_key, &key)),
			chapter_number,
			volume_number,
			title: (!title.is_empty()).then_some(title),
			key,
			..Default::default()
		});
	}
	// When no title in the list has a number, the position in the list (oldest is 1) is the
	// chapter number, so the tracker still has one to sync. A list that mixes numbered and
	// unnumbered titles keeps `None` for the unnumbered ones, as the official zh.jmcomic does.
	// The list is oldest first here, so the position is `index + 1`.
	if chapters
		.iter()
		.all(|c: &Chapter| c.chapter_number.is_none() && c.volume_number.is_none())
	{
		for (index, chapter) in chapters.iter_mut().enumerate() {
			chapter.chapter_number = Some((index + 1) as f32);
		}
	}
	chapters.reverse();
	chapters
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
		assert_eq!(
			browse_url(None, false, 1),
			"https://www.manwang.net/category/page/1"
		);
		assert_eq!(
			browse_url(Some("2571"), true, 3),
			"https://www.manwang.net/category/tags/2571/order/addtime/page/3"
		);
		assert_eq!(
			board_url("week").as_deref(),
			Some("https://www.manwang.net/custom/week")
		);
		assert_eq!(board_url("rank"), None);
		assert_eq!(
			search_url(" 妖神记 "),
			"https://www.manwang.net/index.php/search?key=%E5%A6%96%E7%A5%9E%E8%AE%B0"
		);
		assert_eq!(
			chapter_url("503735", "185808"),
			"https://www.manwang.net/chapter/503735-185808"
		);
	}

	#[aidoku_test]
	fn volume_numbers_may_omit_the_marker() {
		assert_eq!(numbers("3卷 第5話"), (Some(5.0), Some(3.0)));
		// A space between the number and the unit is not read as a volume.
		assert_eq!(numbers("1 卷 第5話"), (None, None));
		assert_eq!(numbers("3卷第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("三卷 第五話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("3卷 後記"), (None, None));
		assert_eq!(numbers("3卷卷首"), (None, None));
		assert_eq!(numbers("06卷番外"), (None, None));
		assert_eq!(numbers("一部分"), (None, None));
		assert_eq!(numbers("三部曲 第2話"), (None, None));
		assert_eq!(numbers("卷三"), (None, None));
		assert_eq!(numbers("卷 3"), (None, None));
		assert_eq!(numbers("01卷後記"), (None, None));
		assert_eq!(numbers("第3卷 第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("第2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("第十二話"), (Some(12.0), None));
	}

	#[aidoku_test]
	fn reads_ids() {
		assert_eq!(id_after("/book/503735", "book").as_deref(), Some("503735"));
		assert_eq!(id_after("/category/tags/2571", "book"), None);
		assert_eq!(id_after("/book/12a", "book"), None);
		assert_eq!(
			chapter_keys("https://www.manwang.net/chapter/503735-185808"),
			Some((String::from("503735"), String::from("185808")))
		);
		assert_eq!(chapter_keys("/chapter/503735"), None);
		assert_eq!(chapter_keys("/chapter/abc-1"), None);
		assert_eq!(
			cover_url(String::from("http://www.manwang.net/a.jpg")),
			"https://www.manwang.net/a.jpg"
		);
	}

	#[aidoku_test]
	fn a_bad_chapter_id_rejects_the_list() {
		let html = r#"<ul id="j_chapter_list"><li data-chapter="12"><a title="第1话">第1话</a></li><li data-chapter="x"><a>第2话</a></li></ul>"#;
		assert!(parse_chapters(&doc(html), "1").is_empty());
		let html = r#"<ul id="j_chapter_list"><li data-chapter="12"><a title="第1话">第1话</a></li><li data-chapter="13"><a>第2话</a></li></ul>"#;
		let keys: Vec<String> = parse_chapters(&doc(html), "1")
			.into_iter()
			.map(|c| c.key)
			.collect();
		assert_eq!(keys, ["13", "12"]);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
