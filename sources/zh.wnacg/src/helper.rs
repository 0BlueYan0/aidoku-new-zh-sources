use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		defaults::defaults_get,
		html::{Document, Element},
		net::Request,
		std::parse_date,
	},
	prelude::*,
	ContentRating, Manga, MangaPageResult, MangaStatus, Result, UpdateStrategy, Viewer,
};

/// Fallback base URL. Prefer [`base_url`], which also honours the user's override and
/// whichever of `info.urls` the app's base URL picker selected.
pub const BASE_URL: &str = "https://www.wnacg.com";

/// The site serves a completely different mobile DOM (`themes/mo`) to phone user
/// agents, and none of the selectors below match it, so every request claims a desktop
/// browser.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// Every timestamp the site prints is a bare `yyyy-MM-dd`, but a date-only pattern is
/// not accepted everywhere `parse_date` runs, so a zero time is appended before parsing.
const DATE_FORMAT: &str = "yyyy-MM-dd HH:mm:ss";

/// A manga carries at most this many tags.
const MAX_TAGS: usize = 255;

/// Bracketed prefixes that describe the release rather than the circle or the artist.
const RELEASE_MARKERS: [&str; 14] = [
	"無修正",
	"無修正版",
	"無修",
	"中国翻訳",
	"中國翻訳",
	"中文翻訳",
	"漢化",
	"汉化",
	"DL版",
	"フルカラー版",
	"フルカラー",
	"Digital",
	"Decensored",
	"Uncensored",
];

// ---------------------------------------------------------------------------
// Base URL and requests
// ---------------------------------------------------------------------------

/// Resolve the base URL, most specific source first: the `customBaseUrl` text setting,
/// then the domain the app's `allowsBaseUrlSelect` picker wrote to `url`, then the
/// compiled-in default.
pub fn base_url() -> String {
	for key in ["customBaseUrl", "url"] {
		if let Some(value) = defaults_get::<String>(key) {
			let trimmed = value.trim().trim_end_matches('/');
			if trimmed.starts_with("http") {
				return String::from(trimmed);
			}
		}
	}
	String::from(BASE_URL)
}

fn request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?
		.header("User-Agent", USER_AGENT)
		.header("Referer", &base_url())
		.header("Accept-Language", "zh-TW,zh;q=0.9"))
}

pub fn fetch_html(url: &str) -> Result<Document> {
	Ok(request(url)?.html()?)
}

/// `photos-gallery-aid-*.html` answers with JavaScript, so it is read as plain text.
pub fn fetch_text(url: &str) -> Result<String> {
	request(url)?.string()
}

// ---------------------------------------------------------------------------
// URL building
// ---------------------------------------------------------------------------

pub fn home_url() -> String {
	format!("{}/", base_url())
}

pub fn album_list_url(page: i32) -> String {
	format!("{}/albums-index-page-{page}.html", base_url())
}

pub fn cate_list_url(cate: &str, page: i32) -> String {
	format!("{}/albums-index-page-{page}-cate-{cate}.html", base_url())
}

pub fn tag_list_url(tag: &str, page: i32) -> String {
	format!(
		"{}/albums-index-page-{page}-tag-{}.html",
		base_url(),
		encode_uri_component(tag.trim())
	)
}

/// `period` is one of `day`, `week`, `month` or `year`.
pub fn rank_list_url(period: &str, page: i32) -> String {
	format!(
		"{}/albums-favorite_ranking-page-{page}-type-{period}.html",
		base_url()
	)
}

/// `scope` is `_all` or `tag`; `sort` is `create_time_DESC` or `create_time_ASC`.
/// Every other value the site accepts syntactically returns zero results.
pub fn search_url(keyword: &str, scope: &str, sort: &str, page: i32) -> String {
	format!(
		"{}/search/index.php?q={}&m=&syn=yes&f={scope}&s={sort}&p={page}",
		base_url(),
		encode_uri_component(keyword.trim())
	)
}

pub fn detail_url(aid: &str) -> String {
	format!("{}/photos-index-aid-{aid}.html", base_url())
}

pub fn gallery_url(aid: &str) -> String {
	format!("{}/photos-gallery-aid-{aid}.html", base_url())
}

// ---------------------------------------------------------------------------
// Small string helpers
// ---------------------------------------------------------------------------

/// Search results wrap the matched words in `<em>` inside the anchor's `title`
/// attribute, so reading the attribute is not enough to get a clean title.
pub fn strip_em(text: &str) -> String {
	text.replace("<em>", "").replace("</em>", "")
}

/// Turn any of the link shapes the site emits into an absolute URL. Covers are written
/// with a `////host/path` prefix and thumbnails with the usual protocol-relative `//`.
pub fn normalize_url(raw: &str) -> String {
	let trimmed = raw.trim();
	if trimmed.is_empty() {
		return String::new();
	}
	if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
		return String::from(trimmed);
	}
	let rest = trimmed.trim_start_matches('/');
	let slashes = trimmed.len() - rest.len();
	if slashes >= 2 {
		format!("https://{rest}")
	} else {
		format!("{}/{rest}", base_url())
	}
}

/// `/photos-index-aid-385618.html` → `385618`.
pub fn aid_from_href(href: &str) -> Option<String> {
	let index = href.find("-aid-")?;
	let digits: String = href[index + 5..]
		.chars()
		.take_while(char::is_ascii_digit)
		.collect();
	if digits.is_empty() {
		None
	} else {
		Some(digits)
	}
}

/// Read a category id out of either a `pic_box cate-19` class or an
/// `/albums-index-cate-19.html` href.
pub fn cate_id(text: &str) -> Option<u32> {
	let index = text.find("cate-")?;
	let digits: String = text[index + 5..]
		.chars()
		.take_while(char::is_ascii_digit)
		.collect();
	digits.parse::<u32>().ok()
}

/// Korean uploads are vertical scrolls; doujinshi, tankoubon and art books are
/// right-to-left books.
pub fn viewer_for_cate(cate: Option<u32>) -> Viewer {
	match cate {
		Some(19..=21) => Viewer::Webtoon,
		_ => Viewer::RightToLeft,
	}
}

/// Pull the circle and the artist out of a doujinshi title.
///
/// `[カマボコ工房 (釜ボコ)] …` yields artists `["カマボコ工房"]` and authors
/// `["釜ボコ"]`. A leading event group such as `(C108)` is stepped over and release
/// markers such as `[無修正]` are skipped, but the scan stops at the first ordinary
/// character so a mid-title `[中国翻訳]` is never mistaken for a circle.
pub fn parse_authors(title: &str) -> (Vec<String>, Vec<String>) {
	let mut rest = title.trim_start();
	loop {
		if let Some(inner) = rest.strip_prefix('(') {
			match inner.find(')') {
				Some(end) => {
					rest = inner[end + 1..].trim_start();
					continue;
				}
				None => break,
			}
		}
		if let Some(inner) = rest.strip_prefix('[') {
			match inner.find(']') {
				Some(end) => {
					let group = inner[..end].trim();
					if RELEASE_MARKERS.contains(&group) {
						rest = inner[end + 1..].trim_start();
						continue;
					}
					return split_circle_and_artist(group);
				}
				None => break,
			}
		}
		break;
	}
	(Vec::new(), Vec::new())
}

/// `カマボコ工房 (釜ボコ)` → (circle, artist). A bare name fills both fields, which is
/// what the app expects for a solo author.
fn split_circle_and_artist(group: &str) -> (Vec<String>, Vec<String>) {
	if let Some(open) = group.rfind('(') {
		if let Some(offset) = group[open..].find(')') {
			let circle = group[..open].trim();
			let artist = group[open + 1..open + offset].trim();
			if !circle.is_empty() && !artist.is_empty() {
				return (
					aidoku::alloc::vec![String::from(circle)],
					aidoku::alloc::vec![String::from(artist)],
				);
			}
		}
	}
	let name = group.trim();
	if name.is_empty() {
		return (Vec::new(), Vec::new());
	}
	(
		aidoku::alloc::vec![String::from(name)],
		aidoku::alloc::vec![String::from(name)],
	)
}

/// Read the page count and the timestamp out of an `info_col` blurb.
///
/// The two fields swap order between pages: list pages write
/// `81張圖片，創建於2026-09-18`, home page blocks write `2026-09-18, 232張圖片` and the
/// thumbnail strip on a detail page writes `上傳於2026-09-18`, so each half is looked
/// up independently instead of by position.
pub fn parse_info_col(text: &str) -> (Option<i32>, Option<i64>) {
	let pages = find_page_count(text);
	let date = find_date(text).and_then(|value: &str| parse_date(format!("{value} 00:00:00"), DATE_FORMAT));
	(pages, date)
}

fn find_page_count(text: &str) -> Option<i32> {
	let marker = text.find("張圖片")?;
	let digits: String = text[..marker]
		.chars()
		.rev()
		.take_while(char::is_ascii_digit)
		.collect();
	let forward: String = digits.chars().rev().collect();
	forward.parse::<i32>().ok()
}

/// Locate the first `yyyy-MM-dd` run in a blurb.
fn find_date(text: &str) -> Option<&str> {
	let bytes = text.as_bytes();
	for (start, window) in bytes.windows(10).enumerate() {
		let shaped = window[..4].iter().all(u8::is_ascii_digit)
			&& window[4] == b'-'
			&& window[5..7].iter().all(u8::is_ascii_digit)
			&& window[7] == b'-'
			&& window[8..].iter().all(u8::is_ascii_digit);
		if shaped {
			return text.get(start..start + 10);
		}
	}
	None
}

// ---------------------------------------------------------------------------
// Image list
// ---------------------------------------------------------------------------

/// Pull the image URLs out of `photos-gallery-aid-*.html`.
///
/// That endpoint answers with JavaScript rather than HTML:
/// `document.writeln("var imglist = [{ url: fast_img_host+\"//host/001.jpg?verify=…\"…")`
/// so the quotes arrive escaped. The `verify` signature expires within hours, which is
/// why this is re-fetched on every read instead of cached.
pub fn parse_imglist(js: &str) -> Vec<String> {
	let host = quoted_value_after(js, "fast_img_host=").unwrap_or_default();
	let mut images: Vec<String> = Vec::new();
	let mut rest = match js.find("imglist") {
		Some(index) => &js[index..],
		None => return images,
	};

	while let Some(index) = rest.find("url:") {
		let (raw, tail) = match take_escaped_quoted(&rest[index + 4..]) {
			Some(pair) => pair,
			None => break,
		};
		rest = tail;

		let value = unescape(raw);
		if value.is_empty() {
			continue;
		}
		// Every gallery ends with a "please bookmark us" card built from a theme asset
		// rather than a real page, so site chrome is dropped.
		if value.contains("/themes/") {
			continue;
		}
		// `fast_img_host` is usually empty and the entries are protocol-relative, but
		// when the site does set a host the entries become plain paths.
		let joined = if value.starts_with("//") || value.starts_with("http") {
			value
		} else {
			format!("{host}{value}")
		};
		let url = normalize_url(&joined);
		if !url.is_empty() {
			images.push(url);
		}
	}

	images
}

/// Read the next `\"…\"` run, returning its contents and the remainder of the text.
fn take_escaped_quoted(text: &str) -> Option<(&str, &str)> {
	const QUOTE: &str = "\\\"";
	let open = text.find(QUOTE)? + QUOTE.len();
	let body = &text[open..];
	let close = body.find(QUOTE)?;
	Some((&body[..close], &body[close + QUOTE.len()..]))
}

fn quoted_value_after(text: &str, marker: &str) -> Option<String> {
	let index = text.find(marker)?;
	take_escaped_quoted(&text[index + marker.len()..]).map(|(value, _)| unescape(value))
}

fn unescape(value: &str) -> String {
	value.replace("\\/", "/")
}

// ---------------------------------------------------------------------------
// List pages
// ---------------------------------------------------------------------------

/// Build a `Manga` from one `li.gallary_item`. The layout is identical on the update,
/// category, tag, ranking, search and home pages.
pub fn parse_manga_item(item: &Element) -> Option<Manga> {
	let link = item.select_first("div.pic_box a")?;
	let href = link.attr("href")?;
	let key = aid_from_href(&href)?;

	// The anchor's `title` attribute holds the untruncated name; the visible text is
	// only a fallback because it can be clipped.
	let title = link
		.attr("title")
		.or_else(|| link.text())
		.map(|raw: String| String::from(strip_em(&raw).trim()))
		.filter(|value: &String| !value.is_empty())?;

	let cover = item
		.select_first("div.pic_box img")
		.and_then(|el: Element| el.attr("src"))
		.map(|src: String| normalize_url(&src))
		.filter(|url: &String| !url.is_empty());

	// Home page blocks omit the `cate-N` class, so those entries fall back to the
	// right-to-left default until the detail page corrects it.
	let cate = item
		.select_first("div.pic_box")
		.and_then(|el: Element| el.attr("class"))
		.and_then(|class: String| cate_id(&class));

	let (artists, authors) = parse_authors(&title);

	Some(Manga {
		key: key.clone(),
		title,
		cover,
		artists: (!artists.is_empty()).then_some(artists),
		authors: (!authors.is_empty()).then_some(authors),
		url: Some(detail_url(&key)),
		status: MangaStatus::Completed,
		content_rating: ContentRating::NSFW,
		viewer: viewer_for_cate(cate),
		// An album is a finished one-shot, so it never needs a library refresh.
		update_strategy: UpdateStrategy::Never,
		..Default::default()
	})
}

pub fn parse_manga_items(root: &Element) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(items) = root.select("li.gallary_item") {
		for item in items {
			if let Some(manga) = parse_manga_item(&item) {
				entries.push(manga);
			}
		}
	}
	entries
}

/// Parse a whole list page and work out whether another page follows.
///
/// The update, category, tag and ranking pages close their paginator with a
/// `span.next` arrow. The search results page does not, so there the page numbers in
/// the paginator links are compared against the current page instead.
pub fn parse_list_page(html: &Document, page: i32, is_search: bool) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(items) = html.select("li.gallary_item") {
		for item in items {
			if let Some(manga) = parse_manga_item(&item) {
				entries.push(manga);
			}
		}
	}

	let has_next_page = if entries.is_empty() {
		false
	} else if is_search {
		paginator_has_page_after(html, page)
	} else {
		html.select_first(".paginator span.next").is_some()
	};

	MangaPageResult {
		entries,
		has_next_page,
	}
}

fn paginator_has_page_after(html: &Document, page: i32) -> bool {
	let Some(links) = html.select(".paginator a") else {
		return false;
	};
	for link in links {
		if let Some(href) = link.attr("href") {
			if let Some(number) = paginator_page_number(&href) {
				if number > page {
					return true;
				}
			}
		}
	}
	false
}

/// Read the `p=` query parameter off a search paginator link.
fn paginator_page_number(href: &str) -> Option<i32> {
	let index = href.rfind("p=")?;
	let digits: String = href[index + 2..]
		.chars()
		.take_while(char::is_ascii_digit)
		.collect();
	digits.parse::<i32>().ok()
}

// ---------------------------------------------------------------------------
// Detail page
// ---------------------------------------------------------------------------

/// Fill `manga` in from `photos-index-aid-*.html` and return the album's upload date.
///
/// The detail page has no "created on" field — that only exists on list pages, which
/// `get_manga_update` never sees — so the date comes from the first entry of the
/// thumbnail strip, which is labelled `上傳於YYYY-MM-DD`.
pub fn apply_detail(html: &Document, manga: &mut Manga) -> Option<i64> {
	if let Some(title) = html
		.select_first("#bodywrap h2")
		.and_then(|el: Element| el.text())
		.map(|raw: String| String::from(strip_em(&raw).trim()))
		.filter(|value: &String| !value.is_empty())
	{
		let (artists, authors) = parse_authors(&title);
		manga.artists = (!artists.is_empty()).then_some(artists);
		manga.authors = (!authors.is_empty()).then_some(authors);
		manga.title = title;
	}

	if let Some(cover) = html
		.select_first("div.uwthumb img")
		.and_then(|el: Element| el.attr("src"))
		.map(|src: String| normalize_url(&src))
		.filter(|url: &String| !url.is_empty())
	{
		manga.cover = Some(cover);
	}

	let mut category = String::new();
	let mut page_count = String::new();
	let mut serial = String::new();
	if let Some(labels) = html.select("div.uwconn label") {
		for label in labels {
			let Some(text) = label.text() else { continue };
			let text = text.trim();
			if let Some(rest) = text.strip_prefix("分類：") {
				category = String::from(rest.trim());
			} else if let Some(rest) = text.strip_prefix("頁數：") {
				page_count = String::from(rest.trim());
			} else if let Some(rest) = text.strip_prefix("編號：") {
				serial = String::from(rest.trim());
			}
		}
	}

	let uploader = html
		.select_first("div.uwuinfo p")
		.and_then(|el: Element| el.text())
		.map(|text: String| String::from(text.trim()))
		.filter(|text: &String| !text.is_empty());

	// The site's own summary field is almost always blank, so fall back to the
	// metadata rather than leaving the detail sheet empty.
	let summary = html
		.select_first("div.uwconn p")
		.and_then(|el: Element| el.text())
		.map(|text: String| String::from(text.trim().trim_start_matches("簡介：").trim()))
		.filter(|text: &String| !text.is_empty());
	manga.description = Some(match summary {
		Some(text) => text,
		None => metadata_description(&category, &page_count, &serial, uploader.as_deref()),
	});

	manga.tags = Some(collect_tags(html, &category));
	manga.status = MangaStatus::Completed;
	manga.content_rating = ContentRating::NSFW;
	manga.update_strategy = UpdateStrategy::Never;
	manga.viewer = viewer_for_cate(breadcrumb_cate(html));
	if manga.url.is_none() {
		manga.url = Some(detail_url(&manga.key));
	}

	html.select_first("div.gallary_wrap div.info_col")
		.and_then(|el: Element| el.text())
		.and_then(|text: String| parse_info_col(&text).1)
}

fn metadata_description(
	category: &str,
	page_count: &str,
	serial: &str,
	uploader: Option<&str>,
) -> String {
	let mut parts: Vec<String> = Vec::new();
	if !category.is_empty() {
		parts.push(format!("分類：{category}"));
	}
	if !page_count.is_empty() {
		parts.push(format!("頁數：{page_count}"));
	}
	if !serial.is_empty() {
		parts.push(format!("編號：{serial}"));
	}
	if let Some(name) = uploader {
		parts.push(format!("上傳者：{name}"));
	}
	parts.join("\n")
}

fn collect_tags(html: &Document, category: &str) -> Vec<String> {
	let mut tags: Vec<String> = Vec::new();
	// `分類：同人誌／漢化` names both the parent and the child category.
	for part in category.split(['／', '/']) {
		let part = part.trim();
		if !part.is_empty() {
			tags.push(String::from(part));
		}
	}
	if let Some(list) = html.select("div.uwconn a.tagshow") {
		for tag in list {
			if let Some(text) = tag.text() {
				let text = text.trim();
				if !text.is_empty() && tags.len() < MAX_TAGS {
					tags.push(String::from(text));
				}
			}
		}
	}
	tags
}

/// The breadcrumb links to the parent and child category, which is the only place the
/// detail page names the category as an id.
fn breadcrumb_cate(html: &Document) -> Option<u32> {
	let links = html.select("div.bread a")?;
	let mut found: Option<u32> = None;
	for link in links {
		if let Some(cate) = link.attr("href").and_then(|href: String| cate_id(&href)) {
			// Korean categories decide the viewer, so they win over the parent.
			if matches!(cate, 19..=21) {
				return Some(cate);
			}
			found = found.or(Some(cate));
		}
	}
	found
}

// ---------------------------------------------------------------------------
// Home page
// ---------------------------------------------------------------------------

pub struct HomeSection {
	pub title: String,
	/// `source.json` listing id, or `None` when no listing covers this block.
	pub listing_id: Option<String>,
	pub entries: Vec<Manga>,
}

/// Split the landing page into its labelled blocks.
///
/// Each block is a `div.title_sort` header followed by a sibling `div.bodywrap` holding
/// the items. The header's "更多>>" anchor carries both the clean name and the target
/// listing, which beats reading `div.title_h2` because the first header also contains a
/// banner image.
pub fn parse_home_sections(html: &Document) -> Vec<HomeSection> {
	let mut sections: Vec<HomeSection> = Vec::new();
	let (Some(headers), Some(blocks)) = (
		html.select("div.title_sort div.r a"),
		html.select("div.bodywrap"),
	) else {
		return sections;
	};

	for (index, header) in headers.into_iter().enumerate() {
		let Some(block) = blocks.get(index) else { break };
		let Some(href) = header.attr("href") else {
			continue;
		};
		let listing_id = home_listing_id(&href);
		let title = match listing_id.and_then(listing_name) {
			Some(name) => String::from(name),
			None => header
				.attr("title")
				.or_else(|| header.text())
				.map(|text: String| String::from(text.trim()))
				.unwrap_or_default(),
		};
		let entries = parse_manga_items(&block);
		if title.is_empty() || entries.is_empty() {
			continue;
		}
		sections.push(HomeSection {
			title,
			listing_id: listing_id.map(String::from),
			entries,
		});
	}

	sections
}

/// Map a "更多>>" target to the matching `source.json` listing id.
fn home_listing_id(href: &str) -> Option<&'static str> {
	if href.contains("/albums.html") {
		return Some("update");
	}
	match cate_id(href)? {
		5 => Some("cate_5"),
		6 => Some("cate_6"),
		7 => Some("cate_7"),
		19 => Some("cate_19"),
		_ => None,
	}
}

/// Listing names, kept identical to `res/source.json` because the home screen matches
/// streamed components back to the skeleton by title.
pub fn listing_name(id: &str) -> Option<&'static str> {
	match id {
		"update" => Some("最新更新"),
		"rank_day" => Some("日榜"),
		"rank_week" => Some("週榜"),
		"rank_month" => Some("月榜"),
		"rank_year" => Some("年榜"),
		"cate_5" => Some("同人誌"),
		"cate_6" => Some("單行本"),
		"cate_7" => Some("雜誌&短篇"),
		"cate_19" => Some("韓漫"),
		_ => None,
	}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// Trimmed copy of a real `photos-gallery-aid-*.html` response, including the
	/// bookmark promo the site appends as the final entry of every gallery.
	const GALLERY_JS: &str = r#"document.writeln("		var fast_img_host=\"\";");
document.writeln("		var imglist = [{ url: fast_img_host+\"//img5.qy0.ru/data/3856/18/001.jpg?verify=1789711200-aaa\", caption: \"[001]\"},{ url: fast_img_host+\"//img5.qy0.ru/data/3856/18/002.jpg?verify=1789711200-bbb\", caption: \"[002]\"},{ url: fast_img_host+\"/themes/weitu/images/bg/shoucang.jpg\", caption: \"收藏\"}];");"#;

	/// Same shape, but with the site serving a host prefix and plain paths.
	const GALLERY_JS_WITH_HOST: &str = r#"document.writeln("var fast_img_host=\"//cdn.example.com\";");
document.writeln("var imglist = [{ url: fast_img_host+\"/data/1/001.jpg\", caption: \"[001]\"}];");"#;

	#[aidoku_test]
	fn parses_image_list_without_the_bookmark_promo() {
		let images = parse_imglist(GALLERY_JS);
		assert_eq!(images.len(), 2);
		assert_eq!(
			images[0],
			"https://img5.qy0.ru/data/3856/18/001.jpg?verify=1789711200-aaa"
		);
		assert_eq!(
			images[1],
			"https://img5.qy0.ru/data/3856/18/002.jpg?verify=1789711200-bbb"
		);
	}

	#[aidoku_test]
	fn joins_image_list_onto_fast_img_host() {
		let images = parse_imglist(GALLERY_JS_WITH_HOST);
		assert_eq!(images, ["https://cdn.example.com/data/1/001.jpg"]);
	}

	#[aidoku_test]
	fn returns_no_images_without_a_list() {
		assert!(parse_imglist("document.writeln(\"nothing here\");").is_empty());
	}

	#[aidoku_test]
	fn reads_circle_and_artist_from_title() {
		let (artists, authors) = parse_authors("[カマボコ工房 (釜ボコ)] 異世界催眠おじさん");
		assert_eq!(artists, ["カマボコ工房"]);
		assert_eq!(authors, ["釜ボコ"]);
	}

	#[aidoku_test]
	fn steps_over_a_leading_event_group() {
		let (artists, authors) = parse_authors("(C108) [Milk+ (みなつきひな)] 結婚初夜 (サマーポケッツ)");
		assert_eq!(artists, ["Milk+"]);
		assert_eq!(authors, ["みなつきひな"]);
	}

	#[aidoku_test]
	fn skips_release_markers() {
		let (artists, authors) = parse_authors("[無修正] [きつぎ] お嬢様の品格");
		assert_eq!(artists, ["きつぎ"]);
		assert_eq!(authors, ["きつぎ"]);
	}

	#[aidoku_test]
	fn ignores_brackets_that_follow_the_title() {
		assert_eq!(
			parse_authors("コミック エグゼ 73 [中国翻訳] [DL版]"),
			(Vec::new(), Vec::new())
		);
		assert_eq!(parse_authors("為民服務App 25-26話"), (Vec::new(), Vec::new()));
	}

	#[aidoku_test]
	fn fills_both_fields_for_a_solo_author() {
		let (artists, authors) = parse_authors("[井雲くす] 村又さんの愛情[フルカラー版][DL版]");
		assert_eq!(artists, ["井雲くす"]);
		assert_eq!(authors, ["井雲くす"]);
	}

	#[aidoku_test]
	fn reads_info_col_in_either_order() {
		let (list_pages, list_date) = parse_info_col("81張圖片，創建於2026-09-18");
		let (home_pages, home_date) = parse_info_col("2026-09-18, 232張圖片");
		assert_eq!(list_pages, Some(81));
		assert_eq!(home_pages, Some(232));
		assert!(list_date.is_some());
		assert_eq!(list_date, home_date);
	}

	#[aidoku_test]
	fn reads_info_col_without_a_page_count() {
		let (pages, date) = parse_info_col("上傳於2026-09-18");
		assert_eq!(pages, None);
		assert!(date.is_some());
	}

	#[aidoku_test]
	fn reads_info_col_with_a_time_component() {
		let (pages, date) = parse_info_col("225張圖片，創建於2026-09-18 02:25:03");
		assert_eq!(pages, Some(225));
		assert!(date.is_some());
	}

	#[aidoku_test]
	fn strips_search_highlighting() {
		assert_eq!(strip_em("爆乳<em>姉妹</em>に教えこむ"), "爆乳姉妹に教えこむ");
	}

	#[aidoku_test]
	fn reads_ids_out_of_links_and_classes() {
		assert_eq!(
			aid_from_href("/photos-index-aid-385618.html").as_deref(),
			Some("385618")
		);
		assert_eq!(aid_from_href("/albums.html"), None);
		assert_eq!(cate_id("pic_box cate-19"), Some(19));
		assert_eq!(cate_id("/albums-index-cate-5.html"), Some(5));
		assert_eq!(cate_id("pic_box"), None);
	}

	#[aidoku_test]
	fn picks_the_viewer_from_the_category() {
		assert_eq!(viewer_for_cate(Some(19)), Viewer::Webtoon);
		assert_eq!(viewer_for_cate(Some(21)), Viewer::Webtoon);
		assert_eq!(viewer_for_cate(Some(5)), Viewer::RightToLeft);
		assert_eq!(viewer_for_cate(None), Viewer::RightToLeft);
	}

	#[aidoku_test]
	fn absolutises_every_link_shape() {
		// Thumbnails are protocol-relative and covers arrive with a "////" prefix.
		assert_eq!(
			normalize_url("//t4.qy0.ru/data/t/1.jpg"),
			"https://t4.qy0.ru/data/t/1.jpg"
		);
		assert_eq!(
			normalize_url("////t4.qy0.ru/data/t/1.jpg"),
			"https://t4.qy0.ru/data/t/1.jpg"
		);
		assert_eq!(
			normalize_url("/photos-index-aid-1.html"),
			format!("{}/photos-index-aid-1.html", base_url())
		);
		assert_eq!(normalize_url("https://x.test/a.jpg"), "https://x.test/a.jpg");
		assert_eq!(normalize_url("  "), "");
	}

	#[aidoku_test]
	fn builds_list_urls() {
		let base = base_url();
		assert_eq!(album_list_url(2), format!("{base}/albums-index-page-2.html"));
		assert_eq!(
			cate_list_url("5", 3),
			format!("{base}/albums-index-page-3-cate-5.html")
		);
		assert_eq!(
			rank_list_url("week", 4),
			format!("{base}/albums-favorite_ranking-page-4-type-week.html")
		);
	}

	#[aidoku_test]
	fn percent_encodes_search_and_tag_urls() {
		let base = base_url();
		assert_eq!(
			tag_list_url("巨乳", 1),
			format!("{base}/albums-index-page-1-tag-%E5%B7%A8%E4%B9%B3.html")
		);
		assert_eq!(
			search_url("姉妹", "_all", "create_time_DESC", 2),
			format!(
				"{base}/search/index.php?q=%E5%A7%89%E5%A6%B9&m=&syn=yes&f=_all&s=create_time_DESC&p=2"
			)
		);
	}

	#[aidoku_test]
	fn listing_names_match_source_json() {
		assert_eq!(listing_name("update"), Some("最新更新"));
		assert_eq!(listing_name("cate_7"), Some("雜誌&短篇"));
		assert_eq!(listing_name("nope"), None);
	}
}
