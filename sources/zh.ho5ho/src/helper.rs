use aidoku::{
	alloc::{String, Vec},
	helpers::uri::{decode_uri, encode_uri_component},
	imports::{
		html::{Document, Element},
		net::{Request, Response},
		std::parse_date,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Viewer,
};

pub const BASE_URL: &str = "https://www.ho5ho.com";

/// Page images answer 403 without a Referer from the site; covers do not need it.
pub const REFERER: &str = "https://www.ho5ho.com/";

/// `中字h漫`, the path every work lives under.
const WORK_BASE: &str = "中字h漫";

/// `h漫標籤`, the tag pages. Tags cannot be searched; each has only its own page.
const TAG_BASE: &str = "h漫標籤";

/// The `分類` terms, in `res/filters.json` order. Only these work as `genre[]` in a
/// search; a tag slug there is ignored and the whole catalogue comes back.
pub const CATEGORIES: [&str; 30] = [
	"全彩",
	"巨乳",
	"痴女",
	"熟女",
	"變態",
	"多P",
	"母狗",
	"接吻",
	"肉感",
	"幻想",
	"M女",
	"可愛",
	"學生",
	"深喉",
	"強姦",
	"亂倫",
	"NTR",
	"肛交",
	"正太",
	"母子",
	"M男",
	"催眠",
	"懷孕",
	"姐姐",
	"貧乳",
	"女性向",
	"同人",
	"聖水",
	"女同",
	"韓漫",
];

/// The listings' `m_orderby` values; also the sort filter's ids.
pub const ORDERS: [&str; 4] = ["latest", "views", "rating", "comments"];

/// Kept apart from `fetch_html` so the home can send its rows together.
pub fn request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?)
}

pub fn fetch_html(url: &str) -> Result<Document> {
	read_html(request(url)?.send()?, url)
}

/// An error page must not be parsed as an empty list or a work without chapters.
pub fn read_html(response: Response, url: &str) -> Result<Document> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[ho5ho] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
}

/// A term's slug: the name percent-encoded with lowercase hex and letters (`M女` →
/// `m%e5%a5%b3`). All 160 terms on the site's filter panel follow this.
pub fn slug(name: &str) -> String {
	encode_uri_component(name.trim()).to_lowercase()
}

/// A path segment from a link, in the encoded lowercase form keys are stored in. Links
/// carry `中字h漫` raw and the work's slug encoded; pasted links may differ.
fn normalize_segment(segment: &str) -> String {
	slug(&decode_uri(segment))
}

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/{}/{key}/", slug(WORK_BASE))
}

pub fn chapter_url(manga_key: &str, key: &str) -> String {
	format!("{BASE_URL}/{}/{manga_key}/{key}/", slug(WORK_BASE))
}

fn page_path(page: i32) -> String {
	if page > 1 {
		format!("page/{page}/")
	} else {
		String::new()
	}
}

pub fn listing_url(order: &str, page: i32) -> Option<String> {
	ORDERS
		.contains(&order)
		.then(|| format!("{BASE_URL}/{}?m_orderby={order}", page_path(page)))
}

pub fn tag_url(name: &str, page: i32) -> String {
	format!(
		"{BASE_URL}/{}/{}/{}",
		slug(TAG_BASE),
		slug(name),
		page_path(page)
	)
}

#[derive(Default)]
pub struct Search {
	pub query: String,
	pub order: Option<String>,
	pub categories: Vec<String>,
	/// `op=1`: works must have every category. Without it the site returns works with any.
	pub match_all: bool,
	pub author: Option<String>,
}

pub fn search_url(search: &Search, page: i32) -> String {
	let mut url = format!(
		"{BASE_URL}/{}?s={}&post_type=wp-manga",
		page_path(page),
		encode_uri_component(search.query.trim())
	);
	if let Some(order) = search
		.order
		.as_deref()
		.filter(|o: &&str| ORDERS.contains(o))
	{
		url.push_str(&format!("&m_orderby={order}"));
	}
	for name in &search.categories {
		url.push_str(&format!("&genre%5B%5D={}", slug(name)));
	}
	if search.match_all && search.categories.len() > 1 {
		url.push_str("&op=1");
	}
	if let Some(author) = search
		.author
		.as_deref()
		.filter(|a: &&str| !a.trim().is_empty())
	{
		url.push_str(&format!("&author={}", encode_uri_component(author.trim())));
	}
	url
}

/// A site link → (work key, chapter key). `/中字h漫/<work>/` and
/// `/中字h漫/<work>/<chapter>/`; anything else is not a work.
pub fn work_keys(href: &str) -> Option<(String, Option<String>)> {
	let path = href.split(['?', '#']).next().unwrap_or(href);
	let path = path
		.strip_prefix("https://")
		.or_else(|| path.strip_prefix("http://"))
		.and_then(|rest: &str| rest.split_once('/'))
		.map(|(_, path)| path)
		.unwrap_or(path);
	let mut segments = path.split('/').filter(|s: &&str| !s.is_empty());
	if normalize_segment(segments.next()?) != slug(WORK_BASE) {
		return None;
	}
	let work = normalize_segment(segments.next()?);
	let chapter = segments.next().map(normalize_segment);
	if segments.next().is_some() || work.is_empty() {
		return None;
	}
	Some((work, chapter))
}

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// An `article.ho5ho-v2-card`: cover link, cover image and title link.
pub fn parse_card(card: &Element) -> Option<Manga> {
	let link = card.select_first("h3.ho5ho-v2-card-title a")?;
	let (key, _) = work_keys(&link.attr("href")?)?;
	let title = text_of(&link);
	if title.is_empty() {
		return None;
	}
	let cover = card
		.select_first("img")
		.and_then(|el: Element| el.attr("src"))
		.filter(|src: &String| src.starts_with("http"));
	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		content_rating: ContentRating::NSFW,
		viewer: Viewer::RightToLeft,
		..Default::default()
	})
}

/// A listing, search or tag page. The details page also has cards (related works),
/// outside the grid.
pub fn parse_manga_list(html: &Document) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(cards) = html.select("section.ho5ho-v2-grid article.ho5ho-v2-card") {
		for card in cards {
			if let Some(manga) = parse_card(&card) {
				if !entries.iter().any(|m: &Manga| m.key == manga.key) {
					entries.push(manga);
				}
			}
		}
	}
	let has_next_page = !entries.is_empty() && html.select_first("a.nextpostslink").is_some();
	MangaPageResult {
		entries,
		has_next_page,
	}
}

/// The names under one `作者` / `分類` / `標籤` heading of the details page.
fn taxonomy(html: &Document, heading: &str) -> Vec<String> {
	let mut names: Vec<String> = Vec::new();
	let Some(groups) = html.select("section.ho5ho-v3-taxonomy-group") else {
		return names;
	};
	for group in groups {
		if group
			.select_first("h3")
			.map(|h: Element| text_of(&h))
			.as_deref()
			!= Some(heading)
		{
			continue;
		}
		if let Some(links) = group.select("a") {
			for link in links {
				let name = text_of(&link);
				if !name.is_empty() && !names.contains(&name) {
					names.push(name);
				}
			}
		}
	}
	names
}

/// `<dt>別名</dt><dd>...</dd>` in the details list.
fn data_value(html: &Document, label: &str) -> Option<String> {
	html.select("dl.ho5ho-v3-data-list > div")?
		.find_map(|row: Element| {
			let dt = row.select_first("dt").map(|el: Element| text_of(&el))?;
			(dt == label)
				.then(|| row.select_first("dd").map(|el: Element| text_of(&el)))
				.flatten()
				.filter(|v: &String| !v.is_empty())
		})
}

/// The site has no synopsis and no status. The alternative title (usually the original
/// Japanese one) is the only text there is.
pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first("h1#ho5ho-detail-title")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first(".ho5ho-v3-cover img")
		.and_then(|el: Element| el.attr("src"))
		.filter(|src: &String| src.starts_with("http"));
	let authors = taxonomy(html, "作者");
	let mut tags = taxonomy(html, "分類");
	for tag in taxonomy(html, "標籤") {
		if !tags.contains(&tag) {
			tags.push(tag);
		}
	}
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description: data_value(html, "別名").map(|alt: String| format!("別名：{alt}")),
		tags: (!tags.is_empty()).then_some(tags),
		status: MangaStatus::Unknown,
		content_rating: ContentRating::NSFW,
		// Checked 2026-10-03: even in `韓漫`, most works are pages, not strips.
		viewer: Viewer::RightToLeft,
		..Default::default()
	})
}

/// `Server 1 - 第一話` → `第一話`. The prefix names the image host (there is only one)
/// and is sometimes `Colored Hentai Story 1`.
pub fn chapter_title(raw: &str) -> String {
	let raw = raw.trim();
	if let Some((prefix, rest)) = raw.split_once(" - ") {
		let label = prefix.trim_end_matches(|c: char| c.is_ascii_digit());
		if label.len() < prefix.len()
			&& label.ends_with(' ')
			&& label
				.chars()
				.all(|c: char| c.is_ascii_alphabetic() || c == ' ')
			&& !rest.trim().is_empty()
		{
			return String::from(rest.trim());
		}
	}
	String::from(raw)
}

fn chinese_number(text: &str) -> Option<f32> {
	let digit = |c: char| "零一二三四五六七八九".chars().position(|d: char| d == c);
	let chars: Vec<char> = text.chars().collect();
	match chars.as_slice() {
		[c] if *c == '十' => Some(10.0),
		[c] => digit(*c).map(|d| d as f32),
		['十', ones] => digit(*ones).map(|d| 10.0 + d as f32),
		[tens, '十'] => digit(*tens).map(|d| d as f32 * 10.0),
		[tens, '十', ones] => Some((digit(*tens)? * 10 + digit(*ones)?) as f32),
		_ => None,
	}
}

/// `第三話` → 3, `第12話` → 12. Ranges (`第1-3話`), `全集` and `上篇` get none.
pub fn chapter_number(title: &str) -> Option<f32> {
	let rest = title.trim().strip_prefix('第')?;
	let end = rest.find(['話', '话', '回'])?;
	let number = &rest[..end];
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))
}

/// `July 6, 2024` → seconds, UTC. The month is turned into a number here because the
/// test runner's date parser does not read month names.
pub fn release_date(text: &str) -> Option<i64> {
	const MONTHS: [&str; 12] = [
		"January",
		"February",
		"March",
		"April",
		"May",
		"June",
		"July",
		"August",
		"September",
		"October",
		"November",
		"December",
	];
	let (month, rest) = text.trim().split_once(' ')?;
	let (day, year) = rest.split_once(',')?;
	let month = MONTHS.iter().position(|m: &&str| *m == month)? + 1;
	let day = day.trim().parse::<u32>().ok()?;
	let year = year.trim().parse::<u32>().ok()?;
	parse_date(
		format!("{year:04}-{month:02}-{day:02} 00:00:00"),
		"yyyy-MM-dd HH:mm:ss",
	)
}

/// The details page lists chapters oldest first; the app wants newest first.
pub fn parse_chapters(html: &Document, manga_key: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();
	let Some(items) = html.select("li.wp-manga-chapter") else {
		return chapters;
	};
	for item in items {
		let Some(link) = item.select_first("a") else {
			continue;
		};
		let Some(key) = link
			.attr("href")
			.and_then(|href: String| work_keys(&href))
			.and_then(|(work, chapter)| (work == manga_key).then_some(chapter).flatten())
		else {
			continue;
		};
		if chapters.iter().any(|c: &Chapter| c.key == key) {
			continue;
		}
		let title = chapter_title(&text_of(&link));
		let date_uploaded = item
			.select_first("span.chapter-release-date")
			.map(|el: Element| text_of(&el))
			.and_then(|date: String| release_date(&date));
		chapters.push(Chapter {
			url: Some(chapter_url(manga_key, &key)),
			key,
			chapter_number: chapter_number(&title),
			title: (!title.is_empty()).then_some(title),
			date_uploaded,
			..Default::default()
		});
	}
	chapters.reverse();
	chapters
}

/// The quoted strings of a JSON string array, with `\/` and `\\` undone.
fn json_strings(json: &str) -> Vec<String> {
	let mut out: Vec<String> = Vec::new();
	let mut chars = json.chars();
	while let Some(c) = chars.next() {
		if c != '"' {
			continue;
		}
		let mut value = String::new();
		while let Some(c) = chars.next() {
			match c {
				'"' => break,
				'\\' => match chars.next() {
					Some(escaped) => value.push(escaped),
					None => break,
				},
				_ => value.push(c),
			}
		}
		out.push(value);
	}
	out
}

/// The reader page's HTML has three `<img>`s; every page is in a JSON manifest the
/// site's script pages through. `chapter_preloaded_images` holds the same list.
pub fn parse_pages(html: &Document) -> Vec<Page> {
	let manifest = html
		.select_first("script#ho5ho-reader-image-manifest")
		// `data()` is the documented accessor for scripts; the test runner only has `html()`.
		.and_then(|el: Element| el.data().or_else(|| el.html()))
		.or_else(|| {
			let el = html.select_first("script#chapter_preloaded_images")?;
			let script = el.data().or_else(|| el.html())?;
			let (_, rest) = script.split_once('[')?;
			let (array, _) = rest.split_once(']')?;
			Some(String::from(array))
		})
		.unwrap_or_default();
	json_strings(&manifest)
		.into_iter()
		.filter(|url: &String| url.starts_with("http"))
		.map(|url: String| Page {
			content: PageContent::url(url),
			..Default::default()
		})
		.collect()
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	const WORK: &str = "%e7%84%a1%e4%ba%ba%e5%b3%b6%e9%81%87%e9%9b%a3%e7%9a%84%e4%b8%89%e4%bd%8d%e8%be%a3%e5%a6%b9%e5%be%8c%e5%ae%ae";

	#[aidoku_test]
	fn builds_addresses() {
		assert_eq!(
			listing_url("views", 2).as_deref(),
			Some("https://www.ho5ho.com/page/2/?m_orderby=views")
		);
		assert_eq!(listing_url("hot", 1), None);
		assert_eq!(
			tag_url("無碼", 1),
			"https://www.ho5ho.com/h%e6%bc%ab%e6%a8%99%e7%b1%a4/%e7%84%a1%e7%a2%bc/"
		);
		let search = Search {
			query: String::from("巨乳"),
			order: Some(String::from("views")),
			categories: Vec::from([String::from("巨乳"), String::from("NTR")]),
			match_all: true,
			..Default::default()
		};
		assert_eq!(
			search_url(&search, 2),
			"https://www.ho5ho.com/page/2/?s=%E5%B7%A8%E4%B9%B3&post_type=wp-manga&m_orderby=views&genre%5B%5D=%e5%b7%a8%e4%b9%b3&genre%5B%5D=ntr&op=1"
		);
		let author = Search {
			author: Some(String::from("ホウホケキヨ")),
			..Default::default()
		};
		assert_eq!(
			search_url(&author, 1),
			"https://www.ho5ho.com/?s=&post_type=wp-manga&author=%E3%83%9B%E3%82%A6%E3%83%9B%E3%82%B1%E3%82%AD%E3%83%A8"
		);
	}

	#[aidoku_test]
	fn reads_work_links() {
		let raw = format!("https://www.ho5ho.com/中字h漫/{WORK}/");
		assert_eq!(work_keys(&raw), Some((String::from(WORK), None)));
		let decoded = "https://www.ho5ho.com/中字h漫/無人島遇難的三位辣妹後宮/server-1_1/?x=1";
		assert_eq!(
			work_keys(decoded),
			Some((String::from(WORK), Some(String::from("server-1_1"))))
		);
		let upper = format!(
			"https://www.ho5ho.com/%E4%B8%AD%E5%AD%97H%E6%BC%AB/{}/",
			WORK.to_uppercase()
		);
		assert_eq!(work_keys(&upper), Some((String::from(WORK), None)));
		assert_eq!(work_keys("https://www.ho5ho.com/h漫分類/ntr/"), None);
		assert_eq!(work_keys("https://www.ho5ho.com/"), None);
		assert_eq!(work_keys("https://www.ho5ho.com/page/2/"), None);
	}

	#[aidoku_test]
	fn cleans_chapter_titles() {
		assert_eq!(chapter_title("Server 1 - 第一話"), "第一話");
		assert_eq!(chapter_title("Colored Hentai Story 1 - 全集"), "全集");
		assert_eq!(chapter_title("Server 1 - 1-3"), "1-3");
		assert_eq!(chapter_title("第1-3話"), "第1-3話");
		assert_eq!(chapter_title("上篇 - 下篇"), "上篇 - 下篇");
		assert_eq!(release_date("August 18, 2026"), Some(1787011200));
		assert_eq!(release_date("Aug 18, 2026"), None);
		assert_eq!(chapter_number("第五話"), Some(5.0));
		assert_eq!(chapter_number("第十二話"), Some(12.0));
		assert_eq!(chapter_number("第二十話"), Some(20.0));
		assert_eq!(chapter_number("第12話"), Some(12.0));
		assert_eq!(chapter_number("第1-3話"), None);
		assert_eq!(chapter_number("全集"), None);
		assert_eq!(chapter_number("上篇"), None);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
