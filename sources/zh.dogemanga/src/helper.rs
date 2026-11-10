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
	let response = Request::get(url)?.send()?;
	let status = response.status_code();
	// The site rate-limits with a 429 notice page. Parsed as content it would read as an
	// empty list, a title without chapters, or a chapter without pages.
	if !(200..300).contains(&status) {
		bail!("[dogemanga] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
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
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// `連載狀態：連載中` / `連載狀態：連載完結`, inside the card's small print.
fn parse_status(text: &str) -> MangaStatus {
	let Some((_, rest)) = text.split_once("連載狀態：") else {
		return MangaStatus::Unknown;
	};
	let rest = rest.trim_start();
	if rest.starts_with("連載中") {
		MangaStatus::Ongoing
	} else if rest.starts_with("連載完結") || rest.starts_with("已完結") || rest.starts_with("完結")
	{
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

/// Read a Chinese numeral up to 99 (`三`, `十二`, `二十`, `五十六`).
fn chinese_number(text: &str) -> Option<f32> {
	let digit = |c: char| "零一二三四五六七八九".chars().position(|d: char| d == c);
	let chars: Vec<char> = text.chars().collect();
	match chars.as_slice() {
		['十'] => Some(10.0),
		[c] => digit(*c).map(|d| d as f32),
		['十', ones] => digit(*ones).map(|d| 10.0 + d as f32),
		[tens, '十'] => digit(*tens).map(|d| d as f32 * 10.0),
		[tens, '十', ones] => Some((digit(*tens)? * 10 + digit(*ones)?) as f32),
		_ => None,
	}
}

/// Read a number at the start of `text` (`281`, `10.5`, `十二`) and return it with the text
/// after it. `1-5` and `163~164` stop at the dash or tilde, so a range has no unit behind
/// the first number.
fn leading_number(text: &str) -> Option<(f32, &str)> {
	let text = text.trim_start();
	let end = text
		.find(|c: char| !(c.is_ascii_digit() || c == '.'))
		.unwrap_or(text.len());
	if end > 0 {
		let number = text[..end].trim_end_matches('.');
		return Some((number.parse::<f32>().ok()?, &text[end..]));
	}
	let end = text
		.find(|c: char| !"零一二三四五六七八九十".contains(c))
		.unwrap_or(text.len());
	Some((chinese_number(&text[..end])?, &text[end..]))
}

const CHAPTER_UNITS: [char; 4] = ['话', '話', '回', '章'];
const VOLUME_UNITS: [char; 5] = ['卷', '季', '部', '冊', '册'];

/// Whether `after` starts with a volume unit that is followed by the end of the title, a
/// space or `第`. `部分` and `部曲` have text right behind the unit, so they are no volume.
fn volume_unit_ends_word(after: &str) -> bool {
	let Some(unit) = after.chars().next() else {
		return false;
	};
	if !VOLUME_UNITS.contains(&unit) {
		return false;
	}
	let rest = &after[unit.len_utf8()..];
	rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('第')
}

/// `(chapter_number, volume_number)` of a title. `第281话试看`, `311話` and `第1012回` are
/// chapters, `第06卷`, `28卷` and `单行本第36卷` are volumes. A bare number is a chapter
/// (`001`). A leading season, part or book is the volume, as the official zh.copymanga
/// reads it: `第二季 第3话` is chapter 3 of volume 2. A season after the chapter only says
/// where the season ends. Side stories (`重制版284`, `老版本247：搭档`) and ranges
/// (`第1-5卷`, `第245-246话`) carry no number: a number picked out of their text would
/// collide with the real chapter or volume that has the same number.
pub fn chapter_numbers(title: &str) -> (Option<f32>, Option<f32>) {
	let title = title.trim();
	let title = title.strip_prefix("单行本").unwrap_or(title);
	let (number, after) = match title.strip_prefix('第') {
		Some(rest) => match leading_number(rest) {
			Some(found) => found,
			None => return (None, None),
		},
		None => match leading_number(title) {
			// A Chinese numeral without 第 is part of a word (`十年之後`), except before a
			// volume unit that ends the title or is followed by a space or 第
			// (`三卷`, `三卷 第五話`), as in the official zh.boylove
			// (`chapter_list/mod.rs:111`, 第 is optional before the volume).
			Some((number, after)) if title.starts_with(|c: char| c.is_ascii_digit()) => {
				(number, after)
			}
			Some((number, after)) if volume_unit_ends_word(after) => (number, after),
			_ => return (None, None),
		},
	};
	let after = after.trim_start();
	let Some(unit) = after.chars().next() else {
		// A bare number such as `001`.
		return if title.starts_with('第') {
			(None, None)
		} else {
			(Some(number), None)
		};
	};
	if CHAPTER_UNITS.contains(&unit) {
		return (Some(number), None);
	}
	if !VOLUME_UNITS.contains(&unit) {
		return (None, None);
	}
	// The chapter, when there is one, follows the season as `第3话`.
	let rest = &after[unit.len_utf8()..];
	let chapter =
		rest.find('第')
			.and_then(|at| match leading_number(&rest[at + '第'.len_utf8()..]) {
				Some((n, tail)) if tail.trim_start().starts_with(CHAPTER_UNITS) => Some(n),
				_ => None,
			});
	// A volume followed by text that is not a chapter (`第3卷 後記`, `01卷後記`) is an
	// appendix of that volume, as in the official zh.zaimanhua (`is_extra`). It would
	// share the volume's number with `第3卷` itself, so it gets none.
	if unit == '卷' && !rest.trim().is_empty() && chapter.is_none() {
		return (None, None);
	}
	(chapter, Some(number))
}

/// The page ids listed under one of the details page's tabs.
fn tab_keys(html: &Document, tab: &str) -> Vec<String> {
	let mut keys: Vec<String> = Vec::new();
	if let Some(links) = html.select(format!(
		"#site-manga__tab-pane-{tab} a.site-manga-thumbnail__link"
	)) {
		for link in links {
			if let Some(key) = link
				.attr("href")
				.and_then(|href: String| id_after(&href, "/p/"))
			{
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
///
/// Empty when a listed entry has no page id: the page shape broke, and a shorter list
/// would replace the library's complete one.
pub fn parse_chapters(html: &Document) -> Vec<Chapter> {
	let volumes = tab_keys(html, "tankobon");
	let extras = tab_keys(html, "bangaihen");
	let mut chapters: Vec<Chapter> = Vec::new();
	let Some(links) = html.select("#site-manga__tab-pane-all a.site-manga-thumbnail__link") else {
		return chapters;
	};
	for link in links {
		let Some(key) = link
			.attr("href")
			.and_then(|href: String| id_after(&href, "/p/"))
		else {
			return Vec::new();
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
		let (chapter, volume) = title
			.as_deref()
			.map(chapter_numbers)
			.unwrap_or((None, None));
		let (chapter_number, volume_number) = if volumes.contains(&key) {
			(None, volume)
		} else if extras.contains(&key) {
			// Some works file split chapters (`第16.2話`) under 番外篇 too. The chapter
			// number is kept here and dropped below if a non-extra entry has the same one.
			(chapter, None)
		} else {
			// A season in front of a chapter is kept. A `卷` title outside the 單行本 tab
			// (`第05卷特典`) gets no volume, so it does not collide with the real volume 5.
			(
				chapter,
				volume.filter(|_| !title.as_deref().unwrap_or("").contains('卷')),
			)
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
	drop_colliding_extras(&mut chapters, &extras);
	number_by_position(&mut chapters);
	chapters
}

/// An entry under 番外篇 keeps its chapter number only when no entry outside that tab has
/// the same one, as the official zh.zaimanhua drops an extra's number when it repeats a
/// chapter's (`chapter.rs` `is_extra`). `山,農田和cosplay姐姐` lists `第16.2話` and the
/// other half chapters under 番外篇; a side story titled `第1話` next to the real 第1話
/// would collide and loses its number.
fn drop_colliding_extras(chapters: &mut [Chapter], extras: &[String]) {
	let regular: Vec<f32> = chapters
		.iter()
		.filter(|c| !extras.contains(&c.key))
		.filter_map(|c| c.chapter_number)
		.collect();
	for chapter in chapters.iter_mut() {
		if extras.contains(&chapter.key)
			&& chapter.chapter_number.is_some_and(|n| regular.contains(&n))
		{
			chapter.chapter_number = None;
		}
	}
}

/// When no entry of the whole `全部` list has a chapter or volume number, the tracker has
/// nothing to sync on, so `chapter_number` becomes the position in that one list, oldest
/// first. The list is newest first, so the position is `len - index`; volumes from the
/// 單行本 tab sit in the same list and are counted by that same order. Once any entry has a
/// number the others stay `None`: a position would collide with a real chapter number.
fn number_by_position(chapters: &mut [Chapter]) {
	if chapters
		.iter()
		.any(|c| c.chapter_number.is_some() || c.volume_number.is_some())
	{
		return;
	}
	let len = chapters.len();
	for (index, chapter) in chapters.iter_mut().enumerate() {
		chapter.chapter_number = Some((len - index) as f32);
	}
}

/// Every page of the chapter is listed on any of its reader pages, in
/// `data-page-index` order.
///
/// An image whose index does not parse is an error rather than skipped: without its
/// index the order of the chapter is unknown.
fn reader_images(html: &Document) -> Result<Vec<(i32, Element)>> {
	let mut images: Vec<(i32, Element)> = Vec::new();
	if let Some(els) = html.select("img.site-reader__image[data-page-index]") {
		for el in els {
			let raw = el.attr("data-page-index").unwrap_or_default();
			let Ok(index) = raw.trim().parse::<i32>() else {
				bail!("[dogemanga] unreadable page index {raw:?}");
			};
			images.push((index, el));
		}
	}
	images.sort_by_key(|(index, _)| *index);
	Ok(images)
}

/// A page without a usable image url fails the whole chapter: dropping it would
/// hand the reader a chapter with a page silently missing.
pub fn parse_pages(html: &Document) -> Result<Vec<Page>> {
	reader_images(html)?
		.into_iter()
		.map(|(index, el)| {
			let Some(url) = el
				.attr("data-page-image-url")
				.filter(|url: &String| url.starts_with("http"))
			else {
				bail!("[dogemanga] page {index} has no image url");
			};
			Ok(Page {
				content: PageContent::url(url),
				..Default::default()
			})
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
	let (_, first) = reader_images(html).ok()?.into_iter().next()?;
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

	#[aidoku_test]
	fn volumes_and_their_appendices() {
		assert_eq!(chapter_numbers("第3卷"), (None, Some(3.0)));
		assert_eq!(chapter_numbers("3卷"), (None, Some(3.0)));
		assert_eq!(chapter_numbers("三卷"), (None, Some(3.0)));
		assert_eq!(chapter_numbers("第3卷 後記"), (None, None));
		assert_eq!(chapter_numbers("第3卷 新的旅程"), (None, None));
		assert_eq!(chapter_numbers("01卷後記"), (None, None));
		assert_eq!(chapter_numbers("06卷番外"), (None, None));
		assert_eq!(chapter_numbers("第3卷 第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(chapter_numbers("第2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(chapter_numbers("第一季 後記"), (None, Some(1.0)));
		assert_eq!(chapter_numbers("第十二話"), (Some(12.0), None));
	}

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, BASE_URL).expect("parse")
	}

	#[aidoku_test]
	fn reads_ids_from_links() {
		assert_eq!(
			id_after("https://dogemanga.com/m/zw7V_YJF", "/m/").as_deref(),
			Some("zw7V_YJF")
		);
		assert_eq!(
			id_after("https://dogemanga.com/p/-ljHIJQj?x=1", "/p/").as_deref(),
			Some("-ljHIJQj")
		);
		assert_eq!(id_after("https://dogemanga.com/?s=1", "/m/"), None);
		assert_eq!(id_after("https://dogemanga.com/m/", "/m/"), None);
	}

	#[aidoku_test]
	fn builds_search_addresses() {
		assert_eq!(
			search_url("海賊", 1),
			"https://dogemanga.com/?q=%E6%B5%B7%E8%B3%8A"
		);
		assert_eq!(
			search_url("海賊", 3),
			"https://dogemanga.com/?q=%E6%B5%B7%E8%B3%8A&o=48"
		);
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
	fn numbers_the_other_title_shapes() {
		assert_eq!(chapter_numbers("311話"), (Some(311.0), None));
		assert_eq!(chapter_numbers("第1012回"), (Some(1012.0), None));
		assert_eq!(chapter_numbers("第十二话"), (Some(12.0), None));
		assert_eq!(chapter_numbers("001"), (Some(1.0), None));
		assert_eq!(chapter_numbers("28卷"), (None, Some(28.0)));
		assert_eq!(chapter_numbers("单行本37卷"), (None, Some(37.0)));
		assert_eq!(chapter_numbers("单行本第36卷"), (None, Some(36.0)));
		// A leading season is the volume; one after the chapter is not.
		assert_eq!(chapter_numbers("第二季 第3话"), (Some(3.0), Some(2.0)));
		assert_eq!(chapter_numbers("第2季"), (None, Some(2.0)));
		assert_eq!(chapter_numbers("第24话 第一季完"), (Some(24.0), None));
		// Side stories and ranges.
		assert_eq!(chapter_numbers("重制版284"), (None, None));
		assert_eq!(chapter_numbers("老版本247：搭档"), (None, None));
		assert_eq!(chapter_numbers("2020番外"), (None, None));
		assert_eq!(chapter_numbers("第1-5卷"), (None, None));
		assert_eq!(chapter_numbers("第245-246话"), (None, None));
		assert_eq!(chapter_numbers("第163~164话"), (None, None));
		assert_eq!(chapter_numbers("十年之後"), (None, None));
	}

	#[aidoku_test]
	fn volume_numbers_may_omit_the_marker() {
		assert_eq!(chapter_numbers("3卷 第5話"), (Some(5.0), Some(3.0)));
		// An Arabic number may have a space before the unit; only a Chinese numeral needs the
		// unit right behind it.
		assert_eq!(chapter_numbers("1 卷 第5話"), (Some(5.0), Some(1.0)));
		assert_eq!(chapter_numbers("3卷第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(chapter_numbers("三卷 第五話"), (Some(5.0), Some(3.0)));
		assert_eq!(chapter_numbers("2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(chapter_numbers("3卷 後記"), (None, None));
		assert_eq!(chapter_numbers("3卷"), (None, Some(3.0)));
		assert_eq!(chapter_numbers("三卷"), (None, Some(3.0)));
		assert_eq!(chapter_numbers("01卷後記"), (None, None));
		assert_eq!(chapter_numbers("3卷卷首"), (None, None));
		assert_eq!(chapter_numbers("06卷番外"), (None, None));
		assert_eq!(chapter_numbers("一部分"), (None, None));
		assert_eq!(chapter_numbers("三部曲 第2話"), (None, None));
		assert_eq!(chapter_numbers("卷三"), (None, None));
		assert_eq!(chapter_numbers("卷 3"), (None, None));
		assert_eq!(chapter_numbers("第3卷 第5話"), (Some(5.0), Some(3.0)));
	}

	#[aidoku_test]
	fn a_list_without_any_number_is_numbered_by_position_oldest_first() {
		let html = doc(concat!(
			r#"<div id="site-manga__tab-pane-all">"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/aaa"><img alt="新作"></a>"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/bbb"><img alt="番外"></a>"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/ccc"><img alt="開篇"></a>"#,
			r#"</div>"#,
		));
		let numbers: Vec<Option<f32>> = parse_chapters(&html)
			.iter()
			.map(|c: &Chapter| c.chapter_number)
			.collect();
		assert_eq!(numbers, [Some(3.0), Some(2.0), Some(1.0)]);
	}

	#[aidoku_test]
	fn side_stories_stay_unnumbered_when_other_entries_have_numbers() {
		let html = doc(concat!(
			r#"<div id="site-manga__tab-pane-all">"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/aaa"><img alt="第2話"></a>"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/bbb"><img alt="番外"></a>"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/ccc"><img alt="第1-2話"></a>"#,
			r#"</div>"#,
		));
		let numbers: Vec<Option<f32>> = parse_chapters(&html)
			.iter()
			.map(|c: &Chapter| c.chapter_number)
			.collect();
		assert_eq!(numbers, [Some(2.0), None, None]);
	}

	#[aidoku_test]
	fn a_chapter_without_a_page_id_rejects_the_list() {
		let html = doc(concat!(
			r#"<div id="site-manga__tab-pane-all">"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/aaa"><img alt="第1話"></a>"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/bbb"><img alt="第2話"></a>"#,
			r#"</div>"#,
		));
		assert_eq!(parse_chapters(&html).len(), 2);
		let html = doc(concat!(
			r#"<div id="site-manga__tab-pane-all">"#,
			r#"<a class="site-manga-thumbnail__link" href="/p/aaa"><img alt="第1話"></a>"#,
			r#"<a class="site-manga-thumbnail__link" href="/m/zzz"><img alt="第2話"></a>"#,
			r#"</div>"#,
		));
		assert!(parse_chapters(&html).is_empty());
	}

	#[aidoku_test]
	fn a_page_without_an_image_url_fails_the_chapter() {
		let html = doc(concat!(
			r#"<img class="site-reader__image" data-page-index="0" data-page-image-url="https://dogemanga.com/images/pages/a.jpg">"#,
			r#"<img class="site-reader__image" data-page-index="1" data-page-image-url="">"#,
		));
		assert!(parse_pages(&html).is_err());
	}

	#[aidoku_test]
	fn an_unreadable_page_index_fails_the_chapter() {
		let html = doc(concat!(
			r#"<img class="site-reader__image" data-page-index="0" data-page-image-url="https://dogemanga.com/images/pages/a.jpg">"#,
			r#"<img class="site-reader__image" data-page-index="x" data-page-image-url="https://dogemanga.com/images/pages/b.jpg">"#,
		));
		assert!(parse_pages(&html).is_err());
	}

	#[aidoku_test]
	fn reads_status_text() {
		assert_eq!(
			parse_status("連載狀態：連載中最近更新：2 天前"),
			MangaStatus::Ongoing
		);
		assert_eq!(
			parse_status("連載狀態：連載完結最近更新：昨天"),
			MangaStatus::Completed
		);
		assert_eq!(parse_status("最近更新：昨天"), MangaStatus::Unknown);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
