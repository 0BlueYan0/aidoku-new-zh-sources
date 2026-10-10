use aidoku::{
	alloc::{String, Vec},
	helpers::{
		element::ElementHelpers,
		uri::{decode_uri, encode_uri_component},
	},
	imports::{
		defaults::{defaults_get, defaults_get_map},
		html::{Document, Element, Html},
		net::{Request, Response},
		std::{current_date, parse_date},
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Viewer,
};

/// The default domain, the first of `res/source.json`'s `urls`. The other four are mirrors
/// listed on the site's own address page, https://bakamh.app (checked 2026-10-09: same works,
/// and every link on a mirror points at that mirror).
pub const BASE_URL: &str = "https://bakamh.com";

/// The domain in use: a custom one typed in settings, else the one picked from `urls`.
/// Sign-in cookies belong to one domain, so after switching the first locked chapter signs
/// in again (`auth::retry_after_lock`).
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

/// The `分类` terms the site's search form offers, as the site names them, in
/// `res/filters.json` order. Their slugs are the names percent-encoded; `en-manga` is the
/// one exception, since the site names it `英文漫画`.
pub const CATEGORIES: [(&str, &str); 7] = [
	("韩漫", "韩漫"),
	("BL", "bl"),
	("全年龄", "全年龄"),
	("动画", "动画"),
	("汉化同人志", "汉化同人志"),
	("汉化日漫", "汉化日漫"),
	("英文漫画", "en-manga"),
];

/// The only category whose works are safe for all ages. Everything else on the site is
/// adult webtoons and doujinshi (tags like `无码`), so the rest is rated NSFW.
const ALL_AGES: &str = "全年龄";

/// Categories of works drawn as pages rather than strips. Checked 2026-10-09: a `汉化日漫`
/// and a `汉化同人志` work had 1350x1920 and 1280x1807 pages; Korean works are strips.
const PAGED_CATEGORIES: [&str; 2] = ["汉化日漫", "汉化同人志"];

/// The listings' and the sort filter's `m_orderby` values. All six were checked to give a
/// different order on 2026-10-09; without one the site sorts by `new-manga`.
pub const ORDERS: [&str; 6] = ["latest", "trending", "views", "rating", "new-manga", "alphabet"];

/// The `status[]` values that have works. `on-hold` and `upcoming` returned none.
pub const STATUSES: [&str; 3] = ["on-going", "end", "canceled"];

/// The settings buttons that open the site so the reader can pass the Cloudflare check by
/// hand, as zh.18mh has: one per domain in `res/source.json`'s `urls`, same order. The app
/// keeps each button's page cookies under its key, and each clearance is only sent to its own
/// domain.
///
/// One button per domain because a button can only open a fixed `url`. A `urlKey` that
/// followed the domain in use was tried on 2026-10-10: the app reads `urlKey` only for OAuth
/// logins, and a web login with only `urlKey` opened a blank sheet (read in the app's
/// `SettingView.swift`, `loginWebSheetView` uses `value.url` alone). A custom domain has no button.
pub const CLOUDFLARE_BUTTONS: [(&str, &str); 5] = [
	("cloudflare", "https://bakamh.com"),
	("cloudflare_ru", "https://bakamh.ru"),
	("cloudflare_baka3", "https://baka3.cfd"),
	("cloudflare_baka1", "https://baka1.cfd"),
	("cloudflare_baka2", "https://baka2.cfd"),
];
const CLEARANCE_COOKIE: &str = "cf_clearance";

/// Whether `key` is one of the verification buttons.
pub fn is_cloudflare_key(key: &str) -> bool {
	CLOUDFLARE_BUTTONS.iter().any(|(k, _)| *k == key)
}

/// The button's page holds a clearance cookie.
pub fn accept_clearance(cookies: &aidoku::HashMap<String, String>) -> bool {
	cookies
		.get(CLEARANCE_COOKIE)
		.is_some_and(|value: &String| !value.is_empty())
}

/// A GET to the site, carrying the button's clearance when it was obtained on the domain
/// in use (zh.18mh sends it the same way). The app puts its own jar's cookies in front;
/// which copy Cloudflare reads when both are there has not been verified.
pub fn site_get(url: &str) -> Result<Request> {
	let mut request = Request::get(url)?;
	if let Some(cookie) = clearance_cookie(url) {
		request = request.header("Cookie", &cookie);
	}
	Ok(request)
}

/// `cf_clearance=…` from the button of `url`'s domain. Decided by the request rather than
/// `base_url()`: logging out visits every signed-in mirror.
pub fn clearance_cookie(url: &str) -> Option<String> {
	let (key, _) = CLOUDFLARE_BUTTONS
		.iter()
		.find(|(_, domain)| same_domain(url, domain))?;
	defaults_get_map(key)
		.and_then(|cookies| cookies.get(CLEARANCE_COOKIE).cloned())
		.filter(|value: &String| !value.is_empty())
		.map(|value: String| format!("{CLEARANCE_COOKIE}={value}"))
}

/// Whether `url` is on `domain` (`https://bakamh.com`, no trailing slash).
pub fn same_domain(url: &str, domain: &str) -> bool {
	url == domain || url.starts_with(&format!("{domain}/"))
}

/// No `User-Agent`: the whole site is behind a Cloudflare challenge, and the clearance
/// cookie only matches the user agent the app solved it with.
///
/// A Cloudflare challenge page that still comes back after the app's own handling is an
/// error, as zh.vomicmh's `fetch_checked` does (the reader chose an error they can retry
/// over an empty list). Resending it here was tried on 2026-10-09 and removed: on device the
/// resend never got through, and each one set off another challenge that the app cancelled
/// 0.4 seconds later.
pub fn fetch_html(url: &str) -> Result<Document> {
	fetch_html_with(url, None)
}

/// `fetch_html` with a time limit on the request.
pub fn fetch_html_with(url: &str, timeout: Option<f64>) -> Result<Document> {
	let mut request = site_get(url)?;
	if let Some(seconds) = timeout {
		request = request.timeout(seconds);
	}
	let response = request.send()?;
	let html = response.get_html()?;
	if is_challenge(&html) {
		log_challenge(url, &response);
		bail!("[bakamh] Cloudflare still blocks {url}");
	}
	if is_site_page(&html) {
		crate::auth::note_site_session(is_signed_in(&html));
	}
	Ok(html)
}

/// The selector half of the app's challenge check. The app also requires `Server: cloudflare`
/// and status 403 or 503 (docs/aidoku-rs-api.md 7.6); this looks at the selectors only, which
/// is why `log_challenge` prints the status.
pub fn is_challenge(html: &Document) -> bool {
	html.select_first("#challenge-error-title, #challenge-error-text")
		.is_some()
}

/// What the site answered when the app's Cloudflare handling gave up, to line up with the
/// app's own `Failed to handle CloudFlare` lines. Cookie names only, never values.
fn log_challenge(url: &str, response: &Response) {
	let set_cookie = response.get_header("set-cookie").unwrap_or_default();
	let sets_clearance = set_cookie.contains("cf_clearance=");
	let sets_bm = set_cookie.contains("__cf_bm=");
	println!(
		"[bakamh] Cloudflare page for {url}: status={} cf-ray={:?} cf-mitigated={:?} sets cf_clearance={sets_clearance} __cf_bm={sets_bm}",
		response.status_code(),
		response.get_header("cf-ray"),
		response.get_header("cf-mitigated"),
	);
}

/// `fetch_html` for callers that need the raw text.
pub fn fetch_text(url: &str) -> Result<String> {
	let response = site_get(url)?.send()?;
	let text = response.get_string()?;
	if Html::parse(&text).map(|doc| is_challenge(&doc)).unwrap_or(false) {
		log_challenge(url, &response);
		bail!("[bakamh] Cloudflare still blocks {url}");
	}
	Ok(text)
}

/// A term's slug: the name percent-encoded with lowercase hex (`后宫` →
/// `%e5%90%8e%e5%ae%ab`). Every tag link in the fixtures follows this.
pub fn slug(name: &str) -> String {
	encode_uri_component(name.trim()).to_lowercase()
}

fn normalize_segment(segment: &str) -> String {
	slug(&decode_uri(segment))
}

pub fn manga_url(key: &str) -> String {
	format!("{}/manga/{key}/", base_url())
}

pub fn chapter_url(manga_key: &str, key: &str) -> String {
	format!("{}/manga/{manga_key}/{key}/", base_url())
}

fn page_path(page: i32) -> String {
	if page > 1 {
		format!("page/{page}/")
	} else {
		String::new()
	}
}

pub fn listing_url(order: &str, page: i32) -> Option<String> {
	ORDERS.contains(&order).then(|| {
		format!(
			"{}/{}?s&post_type=wp-manga&m_orderby={order}",
			base_url(),
			page_path(page)
		)
	})
}

pub fn tag_url(name: &str, page: i32) -> String {
	format!("{}/manga-tag/{}/{}", base_url(), slug(name), page_path(page))
}

/// The slug of a category, by the site's name for it (a tapped tag; checked 2026-10-09 that
/// the details and search pages link `BL`, `英文漫画`, `动画` under these names) or by its
/// `res/filters.json` id.
pub fn category_slug(name: &str) -> Option<String> {
	let name = name.trim();
	CATEGORIES
		.iter()
		.find(|(site, value)| *site == name || *value == name)
		.map(|(_, value)| slug(value))
}

#[derive(Default)]
pub struct Search {
	pub query: String,
	pub order: Option<String>,
	/// Category slugs, already encoded.
	pub categories: Vec<String>,
	/// `op=1`: works must be in every category. Without it the site returns works in any.
	pub match_all: bool,
	pub statuses: Vec<String>,
	pub author: Option<String>,
}

/// Every condition here combines with the others and with a keyword (checked 2026-10-09).
pub fn search_url(search: &Search, page: i32) -> String {
	let mut url = format!(
		"{}/{}?s={}&post_type=wp-manga",
		base_url(),
		page_path(page),
		encode_uri_component(search.query.trim())
	);
	if let Some(order) = search.order.as_deref().filter(|o: &&str| ORDERS.contains(o)) {
		url.push_str(&format!("&m_orderby={order}"));
	}
	for category in &search.categories {
		url.push_str(&format!("&genre%5B%5D={category}"));
	}
	if search.match_all && search.categories.len() > 1 {
		url.push_str("&op=1");
	}
	for status in &search.statuses {
		if STATUSES.contains(&status.as_str()) {
			url.push_str(&format!("&status%5B%5D={status}"));
		}
	}
	if let Some(author) = search.author.as_deref().filter(|a: &&str| !a.trim().is_empty()) {
		url.push_str(&format!("&author={}", encode_uri_component(author.trim())));
	}
	url
}

/// A site link → (work key, chapter key). `/manga/<work>/` and `/manga/<work>/<chapter>/`;
/// anything else is not a work.
pub fn work_keys(href: &str) -> Option<(String, Option<String>)> {
	let path = href.split(['?', '#']).next().unwrap_or(href);
	let path = path
		.strip_prefix("https://")
		.or_else(|| path.strip_prefix("http://"))
		.and_then(|rest: &str| rest.split_once('/'))
		.map(|(_, path)| path)
		.unwrap_or(path);
	let mut segments = path.split('/').filter(|s: &&str| !s.is_empty());
	if segments.next()? != "manga" {
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
	el.text().map(|t: String| String::from(t.trim())).unwrap_or_default()
}

fn rating_of(categories: &[String]) -> ContentRating {
	if categories.iter().any(|c: &String| c == ALL_AGES) {
		ContentRating::Safe
	} else {
		ContentRating::NSFW
	}
}

fn viewer_of(categories: &[String]) -> Viewer {
	if categories
		.iter()
		.any(|c: &String| PAGED_CATEGORIES.contains(&c.as_str()))
	{
		Viewer::RightToLeft
	} else {
		Viewer::Webtoon
	}
}

/// A card on a listing (`.page-item-detail`) or search page (`.c-tabs-item__content`).
/// Only search cards name the work's categories; the others are rated NSFW until the
/// details page is opened.
pub fn parse_card(card: &Element) -> Option<Manga> {
	let link = card.select_first(".post-title a")?;
	let (key, _) = work_keys(&link.attr("href")?)?;
	let title = text_of(&link);
	if title.is_empty() {
		return None;
	}
	let cover = card
		.select_first("img")
		.and_then(|el: Element| el.attr("src").or_else(|| el.attr("data-src")))
		.filter(|src: &String| src.starts_with("http"));
	let categories: Vec<String> = card
		.select(".mg_genres .summary-content a")
		.map(|links| links.map(|a: Element| text_of(&a)).collect())
		.unwrap_or_default();
	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		content_rating: rating_of(&categories),
		viewer: viewer_of(&categories),
		..Default::default()
	})
}

pub fn parse_manga_list(html: &Document) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	if let Some(cards) = html.select(".page-item-detail, .c-tabs-item__content") {
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

/// The `.summary-content` under a details heading (`作者`, `分类`, `状态` …).
fn detail_item(html: &Document, heading: &str) -> Option<Element> {
	html.select(".post-content_item")?.find_map(|item: Element| {
		let h5 = item.select_first(".summary-heading h5")?;
		(text_of(&h5) == heading)
			.then(|| item.select_first(".summary-content"))
			.flatten()
	})
}

fn link_names(item: Option<Element>) -> Vec<String> {
	let mut names: Vec<String> = Vec::new();
	if let Some(links) = item.and_then(|el: Element| el.select("a")) {
		for link in links {
			let name = text_of(&link);
			if !name.is_empty() && !names.contains(&name) {
				names.push(name);
			}
		}
	}
	names
}

pub fn parse_status(text: &str) -> MangaStatus {
	match text.trim() {
		"连载中" => MangaStatus::Ongoing,
		"完结" => MangaStatus::Completed,
		t if t.contains("取消") => MangaStatus::Cancelled,
		_ => MangaStatus::Unknown,
	}
}

/// Whether the page is a work's details page, as opposed to an error or block page.
pub fn is_details_page(html: &Document) -> bool {
	html.select_first("#manga-title h1").is_some()
}

pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first("#manga-title h1")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first(".summary_image img")
		.and_then(|el: Element| el.attr("src").or_else(|| el.attr("data-src")))
		.filter(|src: &String| src.starts_with("http"));
	let authors = link_names(detail_item(html, "作者"));
	let categories = link_names(detail_item(html, "分类"));
	let mut tags = categories.clone();
	for tag in link_names(detail_item(html, "标籤")) {
		if !tags.contains(&tag) {
			tags.push(tag);
		}
	}
	let status = detail_item(html, "状态")
		.map(|el: Element| parse_status(&text_of(&el)))
		.unwrap_or_default();
	let description = html
		.select_first(".mkjp-summary__text")
		// The test runner has no `text_with_newlines`.
		.and_then(|el: Element| el.text_with_newlines().or_else(|| el.text()))
		.map(|t: String| String::from(t.trim()))
		.filter(|t: &String| !t.is_empty());
	Some(Manga {
		key: String::from(key),
		url: Some(manga_url(key)),
		title,
		cover,
		authors: (!authors.is_empty()).then_some(authors),
		description,
		tags: (!tags.is_empty()).then_some(tags),
		status,
		content_rating: rating_of(&categories),
		viewer: viewer_of(&categories),
		..Default::default()
	})
}

/// `第21话` → 21, `第1卷` → none (a volume, not a chapter).
pub fn chapter_number(title: &str) -> Option<f32> {
	let rest = title.trim().strip_prefix('第')?;
	let end = rest.find(['话', '話'])?;
	rest[..end].trim().parse::<f32>().ok()
}

/// `2026 年 10 月 1 日`, or relative: `9 分 前`, `9 小时 前`, `3 天 前`.
pub fn release_date(text: &str, now: i64) -> Option<i64> {
	let text = text.trim();
	if let Some(ago) = text.strip_suffix('前') {
		let ago = ago.trim();
		let digits: String = ago.chars().take_while(|c: &char| c.is_ascii_digit()).collect();
		let count = digits.parse::<i64>().ok()?;
		let unit = ago[digits.len()..].trim();
		let seconds = match unit {
			"秒" => 1,
			"分" | "分钟" => 60,
			"小时" => 3600,
			"天" => 86_400,
			"周" => 604_800,
			_ => return None,
		};
		return Some(now - count * seconds);
	}
	let numbers: Vec<u32> = text
		.split(['年', '月', '日'])
		.map(str::trim)
		.filter(|s: &&str| !s.is_empty())
		.filter_map(|s: &str| s.parse::<u32>().ok())
		.collect();
	let [year, month, day] = numbers.as_slice() else {
		return None;
	};
	// The site's dates are in its own time zone, UTC+8 (`data-offset="480"` on its times).
	parse_date(
		format!("{year:04}-{month:02}-{day:02} 00:00:00"),
		"yyyy-MM-dd HH:mm:ss",
	)
	.map(|utc: i64| utc - 8 * 3600)
}

/// The lock badge next to a chapter in the list.
/// `🔒 需登录` goes away once signed in; `🔒 10月16日 13:00 解锁` stays, since an invite
/// code is needed to read early.
pub fn badge_suffix(badge: &str) -> Option<String> {
	let badge = badge.trim_start_matches('🔒').trim();
	if badge.contains("需登录") {
		return Some(String::from("需登入"));
	}
	let date = badge.strip_suffix("解锁")?.trim();
	let date = date.split_whitespace().next()?;
	let (month, rest) = date.split_once('月')?;
	let day = rest.strip_suffix('日')?;
	Some(format!("{}/{} 解鎖", month.trim(), day.trim()))
}

/// The details page lists chapters newest first, as the app wants them.
pub fn parse_chapters(html: &Document, manga_key: &str) -> Vec<Chapter> {
	let now = current_date();
	let mut chapters: Vec<Chapter> = Vec::new();
	let Some(items) = html.select("ul.version-chap li") else {
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
		let name = text_of(&link);
		let chapter_number = chapter_number(&name);
		let title = match item
			.select_first(".mkjp-ea-badge")
			.and_then(|el: Element| badge_suffix(&text_of(&el)))
		{
			Some(suffix) => format!("{name}・{suffix}"),
			None => name,
		};
		let date_uploaded = item
			.select_first(".chapter-release-date i")
			.and_then(|el: Element| release_date(&text_of(&el), now));
		chapters.push(Chapter {
			url: Some(chapter_url(manga_key, &key)),
			key,
			chapter_number,
			title: (!title.is_empty()).then_some(title),
			date_uploaded,
			..Default::default()
		});
	}
	chapters
}

pub fn parse_pages(html: &Document) -> Vec<Page> {
	let Some(images) = html.select(".reading-content img.wp-manga-chapter-img") else {
		return Vec::new();
	};
	images
		.filter_map(|img: Element| {
			img.attr("data-src")
				.or_else(|| img.attr("src"))
				.map(|src: String| String::from(src.trim()))
		})
		.filter(|src: &String| src.starts_with("http"))
		.map(|src: String| Page {
			content: PageContent::url(src),
			..Default::default()
		})
		.collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lock {
	/// `本章节需要登录后阅读`: any signed-in reader can read it.
	SignIn,
	/// `本章节将于 … 对所有用户开放`: early access, which needs an invite code even when
	/// signed in. Signing in again does not help.
	EarlyAccess,
}

/// Why a reader page has no images, when the site says so.
pub fn chapter_lock(html: &Document) -> Option<Lock> {
	let title = html.select_first(".mkjp-ea-lock .mkjp-ea-lock__title")?;
	if text_of(&title).contains("需要登录") {
		Some(Lock::SignIn)
	} else {
		Some(Lock::EarlyAccess)
	}
}

fn body_has_class(html: &Document, class: &str) -> bool {
	html.select_first("body")
		.map(|body: Element| body.has_class(class))
		.unwrap_or(false)
}

/// Whether the page came from the site's theme at all. A Cloudflare challenge or block page
/// does not, and must not be read as "signed out".
pub fn is_site_page(html: &Document) -> bool {
	body_has_class(html, "wp-theme-madara")
}

/// WordPress marks every page it renders for a signed-in user with `body.logged-in`.
pub fn is_signed_in(html: &Document) -> bool {
	body_has_class(html, "logged-in")
}

pub fn user_name(html: &Document) -> Option<String> {
	html.select_first(".c-user_name")
		.map(|el: Element| text_of(&el))
		.filter(|name: &String| !name.is_empty())
}

/// The logout link carries a nonce tied to the session, so it has to be read from a page.
pub fn logout_url(html: &Document) -> Option<String> {
	html.select_first("a[href*=\"action=logout\"]")
		.and_then(|el: Element| el.attr("href"))
		.filter(|href: &String| href.starts_with("http"))
}

/// The login nonce from `var wpMangaLogin = {…,"nonce":"…",…}`, an inline script on
/// every page.
pub fn login_nonce(page: &str) -> Option<String> {
	let (_, rest) = page.split_once("var wpMangaLogin")?;
	let (_, rest) = rest.split_once("\"nonce\":\"")?;
	let (nonce, _) = rest.split_once('"')?;
	(!nonce.is_empty() && nonce.chars().all(|c: char| c.is_ascii_alphanumeric()))
		.then(|| String::from(nonce))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, BASE_URL).expect("parse")
	}

	/// `魔法少女退役後`, a work whose recent chapters are locked.
	const LOCKED: &str = "%e9%ad%94%e6%b3%95%e5%b0%91%e5%a5%b3%e9%80%80%e5%bd%b9%e5%be%8c-2";
	/// `解禁:初始的快感`, a finished work with nothing locked.
	const FREE: &str = "%e8%a7%a3%e7%a6%81%e5%88%9d%e5%a7%8b%e7%9a%84%e5%bf%ab%e6%84%9f";

	#[aidoku_test]
	fn matches_the_clearance_domain() {
		assert!(same_domain("https://bakamh.com/", "https://bakamh.com"));
		assert!(same_domain("https://bakamh.com/manga/x/", "https://bakamh.com"));
		assert!(!same_domain("https://bakamh.ru/", "https://bakamh.com"));
		assert!(!same_domain("https://bakamh.com.evil/", "https://bakamh.com"));
	}

	#[aidoku_test]
	fn tidies_base_urls() {
		assert_eq!(base_url(), BASE_URL);
		assert_eq!(normalize_base_url(" baka9.cfd/ ").as_deref(), Some("https://baka9.cfd"));
		assert_eq!(normalize_base_url("https://bakamh.ru").as_deref(), Some("https://bakamh.ru"));
		assert_eq!(normalize_base_url("  "), None);
		let json = include_str!("../res/source.json");
		assert!(json.contains(&format!("\"urls\": [\n\t\t\t\"{BASE_URL}\"")));
	}

	#[aidoku_test]
	fn builds_addresses() {
		assert_eq!(
			listing_url("views", 2).as_deref(),
			Some("https://bakamh.com/page/2/?s&post_type=wp-manga&m_orderby=views")
		);
		assert_eq!(listing_url("hot", 1), None);
		assert_eq!(
			tag_url("巨乳", 2),
			"https://bakamh.com/manga-tag/%e5%b7%a8%e4%b9%b3/page/2/"
		);
		let search = Search {
			query: String::from("魔法"),
			order: Some(String::from("rating")),
			categories: Vec::from([category_slug("BL").unwrap(), category_slug("全年龄").unwrap()]),
			match_all: true,
			statuses: Vec::from([String::from("end"), String::from("on-hold")]),
			..Default::default()
		};
		assert_eq!(
			search_url(&search, 1),
			"https://bakamh.com/?s=%E9%AD%94%E6%B3%95&post_type=wp-manga&m_orderby=rating&genre%5B%5D=bl&genre%5B%5D=%e5%85%a8%e5%b9%b4%e9%be%84&op=1&status%5B%5D=end"
		);
		let author = Search {
			author: Some(String::from("Bandi")),
			..Default::default()
		};
		assert_eq!(
			search_url(&author, 3),
			"https://bakamh.com/page/3/?s=&post_type=wp-manga&author=Bandi"
		);
		assert_eq!(category_slug("英文漫画").as_deref(), Some("en-manga"));
		assert_eq!(category_slug("巨乳"), None);
	}

	/// The category slugs must be the ones the site's own search form sends.
	#[aidoku_test]
	fn category_slugs_match_the_search_form() {
		let html = doc(include_str!("fixtures/search.html"));
		let inputs = html.select("input[name=\"genre[]\"]").expect("form");
		let mut form: Vec<(String, String)> = Vec::new();
		for input in inputs {
			let value = input.attr("value").expect("value");
			let label = html
				.select_first(&format!("label[for=\"{value}\"]"))
				.map(|el: Element| text_of(&el))
				.expect("label");
			form.push((label, value));
		}
		assert_eq!(form.len(), CATEGORIES.len());
		for (name, _) in CATEGORIES {
			let value = category_slug(name).unwrap();
			assert!(form.contains(&(String::from(name), value)), "{name}");
		}
	}

	/// Tag links on the details pages are the tag names, encoded.
	#[aidoku_test]
	fn tag_slugs_match_the_links() {
		for page in [
			include_str!("fixtures/manga_locked.html"),
			include_str!("fixtures/manga_free.html"),
		] {
			let html = doc(page);
			let mut checked = 0;
			for link in html.select(".tags-content a, .genres-content a").expect("tags") {
				let href = link.attr("href").expect("href");
				let name = text_of(&link);
				assert!(href.ends_with(&format!("/{}/", slug(&name))), "{name} {href}");
				checked += 1;
			}
			assert!(checked >= 4);
		}
	}

	#[aidoku_test]
	fn reads_work_links() {
		let raw = format!("https://bakamh.com/manga/{LOCKED}/");
		assert_eq!(work_keys(&raw), Some((String::from(LOCKED), None)));
		assert_eq!(
			work_keys("https://bakamh.com/manga/魔法少女退役後-2/c-21/?style=list"),
			Some((String::from(LOCKED), Some(String::from("c-21"))))
		);
		assert_eq!(
			work_keys("https://bakamh.com/manga/not-sober/"),
			Some((String::from("not-sober"), None))
		);
		assert_eq!(
			work_keys("https://baka3.cfd/manga/not-sober/c-2/"),
			Some((String::from("not-sober"), Some(String::from("c-2"))))
		);
		assert_eq!(work_keys("https://bakamh.com/manga-tag/bl/"), None);
		assert_eq!(work_keys("https://bakamh.com/"), None);
	}

	#[aidoku_test]
	fn parses_listing_pages() {
		for (fixture, count) in [
			(include_str!("fixtures/home.html"), 36),
			(include_str!("fixtures/latest.html"), 12),
			(include_str!("fixtures/search.html"), 12),
			(include_str!("fixtures/search_p2.html"), 12),
			(include_str!("fixtures/allages.html"), 18),
			(include_str!("fixtures/tag.html"), 12),
		] {
			let result = parse_manga_list(&doc(fixture));
			assert_eq!(result.entries.len(), count);
			assert!(result.has_next_page);
			for manga in &result.entries {
				assert!(manga.cover.is_some(), "{}", manga.title);
			}
		}
		let home = parse_manga_list(&doc(include_str!("fixtures/home.html")));
		assert_eq!(home.entries[0].title, "Not Sober");
		assert_eq!(home.entries[0].key, "not-sober");
		assert_eq!(home.entries[0].content_rating, ContentRating::NSFW);

		// Search cards name the category, so all-ages works are rated there already.
		let search = parse_manga_list(&doc(include_str!("fixtures/search.html")));
		let first = &search.entries[0];
		assert_eq!(first.title, "魔法少年");
		assert_eq!(first.content_rating, ContentRating::Safe);
		assert!(search
			.entries
			.iter()
			.any(|m: &Manga| m.content_rating == ContentRating::NSFW));
	}

	#[aidoku_test]
	fn reads_the_details_page() {
		let html = doc(include_str!("fixtures/manga_locked.html"));
		assert!(is_details_page(&html));
		let manga = parse_details(&html, LOCKED).expect("details");
		assert_eq!(manga.title, "魔法少女退役後");
		assert_eq!(manga.authors, Some(Vec::from([String::from("HONGJJANGJJANG")])));
		assert_eq!(
			manga.tags,
			Some(Vec::from(["韩漫", "后宫", "奇幻", "无码"].map(String::from)))
		);
		assert_eq!(manga.status, MangaStatus::Ongoing);
		assert_eq!(manga.content_rating, ContentRating::NSFW);
		assert_eq!(manga.viewer, Viewer::Webtoon);
		assert_eq!(
			manga.cover.as_deref(),
			Some("https://bakamh.com/wp-content/uploads/2026/09/19563068206a9a3face25637.jpg")
		);
		let description = manga.description.expect("description");
		assert!(description.starts_with("因为诅咒而一分为二的魔法少女"), "{description}");
		assert!(description.contains('\n'));

		let free = parse_details(&doc(include_str!("fixtures/manga_free.html")), FREE).expect("details");
		assert_eq!(free.status, MangaStatus::Completed);

		assert!(!is_details_page(&doc(include_str!("fixtures/home.html"))));
	}

	#[aidoku_test]
	fn labels_locked_chapters() {
		let chapters = parse_chapters(&doc(include_str!("fixtures/manga_locked.html")), LOCKED);
		assert_eq!(chapters.len(), 21);
		assert_eq!(chapters[0].key, "c-21");
		assert_eq!(chapters[0].title.as_deref(), Some("第21话・10/16 解鎖"));
		assert_eq!(chapters[0].chapter_number, Some(21.0));
		assert_eq!(chapters[1].title.as_deref(), Some("第20话・需登入"));
		assert_eq!(chapters[4].title.as_deref(), Some("第17话"));
		assert_eq!(chapters[20].key, "c-1");
		assert_eq!(
			chapters[20].url.as_deref(),
			Some("https://bakamh.com/manga/%e9%ad%94%e6%b3%95%e5%b0%91%e5%a5%b3%e9%80%80%e5%bd%b9%e5%be%8c-2/c-1/")
		);
		// 2026 年 10 月 1 日, UTC+8
		assert_eq!(chapters[1].date_uploaded, Some(1790784000));

		// Signed in, the sign-in badges are gone and only early access is marked.
		let signed_in = parse_chapters(&doc(include_str!("fixtures/manga_signed_in.html")), LOCKED);
		assert!(signed_in.len() >= 21);
		assert!(signed_in
			.iter()
			.all(|c: &Chapter| !c.title.as_deref().unwrap_or("").contains("需登入")));
		assert!(signed_in[0].title.as_deref().unwrap_or("").ends_with("解鎖"));

		let free = parse_chapters(&doc(include_str!("fixtures/manga_free.html")), FREE);
		assert_eq!(free.len(), 103);
		assert_eq!(free[0].title.as_deref(), Some("第103话 最终话 幸福的方法"));
		assert_eq!(free[0].chapter_number, Some(103.0));
		assert_eq!(free[102].chapter_number, Some(1.0));
	}

	#[aidoku_test]
	fn reads_dates_and_numbers() {
		let now = 1_800_000_000;
		assert_eq!(release_date("9 小时 前", now), Some(now - 9 * 3600));
		assert_eq!(release_date("10 分 前", now), Some(now - 600));
		assert_eq!(release_date("3 天 前", now), Some(now - 3 * 86_400));
		assert_eq!(release_date("2024 年 11 月 27 日", now), Some(1732636800));
		assert_eq!(release_date("昨天", now), None);
		assert_eq!(chapter_number("第5话"), Some(5.0));
		assert_eq!(chapter_number("第1卷"), None);
		assert_eq!(chapter_number("番外"), None);
		assert_eq!(badge_suffix("🔒 需登录").as_deref(), Some("需登入"));
		assert_eq!(badge_suffix("🔒 10月16日 13:00 解锁").as_deref(), Some("10/16 解鎖"));
		assert_eq!(badge_suffix("热门"), None);
		assert_eq!(parse_status("已取消"), MangaStatus::Cancelled);
	}

	#[aidoku_test]
	fn reads_reader_pages() {
		let reader = doc(include_str!("fixtures/reader.html"));
		let pages = parse_pages(&reader);
		assert_eq!(pages.len(), 286);
		assert_eq!(
			pages[0].content,
			PageContent::url("https://t1.bakamh.de/manga_6a9a3fad08614/fdca81536b19985cbd04bcfb00205a94/c785a671003386c48eec43d250fee5eb.jpg")
		);
		assert_eq!(chapter_lock(&reader), None);

		let sign_in = doc(include_str!("fixtures/reader_login.html"));
		assert!(parse_pages(&sign_in).is_empty());
		assert_eq!(chapter_lock(&sign_in), Some(Lock::SignIn));

		assert!(!is_challenge(&reader));
		let challenge = doc("<html><head><title>x</title></head><body><h1 id=\"challenge-error-title\">x</h1></body></html>");
		assert!(is_challenge(&challenge));

		let early = doc(include_str!("fixtures/reader_early.html"));
		assert!(parse_pages(&early).is_empty());
		assert_eq!(chapter_lock(&early), Some(Lock::EarlyAccess));
	}

	#[aidoku_test]
	fn reads_the_session() {
		let signed_out = doc(include_str!("fixtures/manga_locked.html"));
		assert!(is_site_page(&signed_out));
		assert!(!is_site_page(&doc("<html><body class=\"no-js\"></body></html>")));
		assert!(!is_signed_in(&signed_out));
		assert_eq!(logout_url(&signed_out), None);
		let signed_in = doc(include_str!("fixtures/manga_signed_in.html"));
		assert!(is_signed_in(&signed_in));
		assert_eq!(user_name(&signed_in).as_deref(), Some("reader01"));
		let logout = logout_url(&signed_in).expect("logout");
		assert!(logout.starts_with("https://bakamh.com/bmpcc/?action=logout&"), "{logout}");
		assert!(logout.ends_with("_wpnonce=0123456789"), "{logout}");

		assert_eq!(
			login_nonce(include_str!("fixtures/home.html")).as_deref(),
			Some("09a2d227cb")
		);
		assert_eq!(login_nonce("<html></html>"), None);
	}
}
