use aidoku::{
	alloc::{format, String, Vec},
	imports::{
		defaults::defaults_get,
		html::{Document, Element},
		net::Request,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result,
};

pub const DEFAULT_BASE_URL: &str = "https://www.favcomic.com";
pub const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

/// Image CDN hosts the site itself hands out. All three serve byte-identical content, so the
/// choice is purely about which route reaches the reader fastest.
pub const CDN_HOSTS: [&str; 3] = ["cdn.favcomic.com", "cdn.favcomic.xyz", "ccdeoo.popwow.top"];

/// Setting key for the image route picker in `res/settings.json`.
pub const IMAGE_LINE_KEY: &str = "image_line";

/// The base url the user picked in the app, falling back to the primary domain.
pub fn base_url() -> String {
	defaults_get::<String>("url")
		.map(|url| String::from(url.trim_end_matches('/')))
		.filter(|url| !url.is_empty())
		.unwrap_or_else(|| String::from(DEFAULT_BASE_URL))
}

/// Swaps the CDN host of an image url for the one the user picked.
///
/// Only hosts we know belong to the site are rewritten, so unrelated urls pass through
/// untouched. Path, filename and the per-file nonce are identical across hosts.
pub fn rewrite_image_host(url: &str, preferred: &str) -> String {
	if preferred.is_empty() || !CDN_HOSTS.contains(&preferred) {
		return String::from(url);
	}
	let Some(rest) = url.strip_prefix("https://") else {
		return String::from(url);
	};
	let Some((host, path)) = rest.split_once('/') else {
		return String::from(url);
	};
	if !CDN_HOSTS.contains(&host) {
		return String::from(url);
	}
	format!("https://{preferred}/{path}")
}

/// The image route the user picked, or an empty string to keep whatever the page served.
pub fn preferred_image_host() -> String {
	defaults_get::<String>(IMAGE_LINE_KEY).unwrap_or_default()
}

/// Every request needs the site's own Referer: the primary CDN answers 403 without it.
pub fn request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?
		.header("User-Agent", USER_AGENT)
		.header("Referer", &format!("{}/", base_url())))
}

pub fn fetch_html(url: &str) -> Result<Document> {
	// Renews the token cookie if it is about to lapse; a no-op when nobody is logged in.
	crate::auth::ensure_session();
	Ok(request(url)?.html()?)
}

/// Pulls the manga key out of a `/comic/detail/<id>` href.
pub fn manga_key_from_href(href: &str) -> Option<String> {
	let rest = href.split("/comic/detail/").nth(1)?;
	let key = rest.split(['/', '?', '#']).next()?;
	if key.is_empty() {
		None
	} else {
		Some(String::from(key))
	}
}

/// Pulls the chapter key out of a `/comic/chapter/<id>` href.
pub fn chapter_key_from_href(href: &str) -> Option<String> {
	let rest = href.split("/comic/chapter/").nth(1)?;
	let key = rest.split(['/', '?', '#']).next()?;
	if key.is_empty() {
		None
	} else {
		Some(String::from(key))
	}
}

/// The site writes status in simplified Chinese; accept the traditional spellings too.
fn status_from_text(text: &str) -> MangaStatus {
	if text.contains("完结") || text.contains("完結") {
		MangaStatus::Completed
	} else if text.contains("连载") || text.contains("連載") {
		MangaStatus::Ongoing
	} else {
		MangaStatus::Unknown
	}
}

/// Reads the number out of a chapter title such as `第113话` or `第1-2话`.
pub fn chapter_number_from_title(title: &str) -> Option<f32> {
	let start = title.find(|c: char| c.is_ascii_digit())?;
	let rest = &title[start..];
	let end = rest
		.find(|c: char| !c.is_ascii_digit() && c != '.')
		.unwrap_or(rest.len());
	rest[..end].trim_end_matches('.').parse::<f32>().ok()
}

/// Whether the listing page offers a next page.
pub fn has_next_page(document: &Document) -> bool {
	document
		.select(".pagination_box a")
		.map(|links| {
			links.into_iter().any(|link| {
				link.attr("title")
					.is_some_and(|title| title.contains("下一页") || title.contains("下一頁"))
			})
		})
		.unwrap_or(false)
}

fn manga_from_cover_box(element: &Element, base: &str) -> Option<Manga> {
	let link = element.select_first("a")?;
	let key = manga_key_from_href(&link.attr("href")?)?;
	let image = element.select_first("img");

	// The cover's alt text is "<title> 连载中漫画封面", so it carries the status as well.
	let alt = image
		.as_ref()
		.and_then(|img| img.attr("alt"))
		.unwrap_or_default();

	let title = link
		.attr("title")
		.or_else(|| link.text())
		.filter(|t| !t.is_empty())?;

	Some(Manga {
		url: Some(format!("{base}/comic/detail/{key}")),
		key,
		title,
		cover: image.and_then(|img| img.attr("data-src").or_else(|| img.attr("src"))),
		status: status_from_text(&alt),
		..Default::default()
	})
}

/// Parses any category or search listing page; they all share the same card markup.
pub fn parse_listing(document: &Document) -> MangaPageResult {
	let base = base_url();
	let entries = document
		.select(".cover_box")
		.map(|boxes| {
			boxes
				.into_iter()
				.filter_map(|el| manga_from_cover_box(&el, &base))
				.collect::<Vec<Manga>>()
		})
		.unwrap_or_default();

	MangaPageResult {
		has_next_page: !entries.is_empty() && has_next_page(document),
		entries,
	}
}

/// The ranking page uses its own card markup and is never paginated.
pub fn parse_rank(document: &Document) -> MangaPageResult {
	let base = base_url();
	let entries = document
		.select("ul.rank_list li.rank_item a")
		.map(|links| {
			links
				.into_iter()
				.filter_map(|link| {
					let key = manga_key_from_href(&link.attr("href")?)?;
					let image = link.select_first("img");
					let title = image
						.as_ref()
						.and_then(|img| img.attr("alt"))
						.filter(|t| !t.is_empty())?;
					Some(Manga {
						url: Some(format!("{base}/comic/detail/{key}")),
						key,
						title,
						cover: image.and_then(|img| img.attr("data-src")),
						..Default::default()
					})
				})
				.collect::<Vec<Manga>>()
		})
		.unwrap_or_default();

	MangaPageResult {
		entries,
		has_next_page: false,
	}
}

/// Fills in details from a `/comic/detail/<id>` document.
pub fn parse_detail(document: &Document, manga: &mut Manga) {
	if let Some(title) = document
		.select_first(".info_box h1")
		.and_then(|el| el.text())
		.filter(|t| !t.is_empty())
	{
		manga.title = title;
	}

	if let Some(cover) = document
		.select_first("img.comic_cover")
		.and_then(|el| el.attr("data-src"))
	{
		manga.cover = Some(cover);
	}

	let authors = document
		.select(".author a")
		.map(|list| {
			list.into_iter()
				.filter_map(|el| el.text())
				.filter(|name| !name.is_empty())
				.collect::<Vec<String>>()
		})
		.unwrap_or_default();
	if !authors.is_empty() {
		manga.authors = Some(authors);
	}

	if let Some(description) = document
		.select_first(".intro_box .txt")
		.and_then(|el| el.text())
	{
		let cleaned = description
			.trim_start_matches("作品介绍：")
			.trim_start_matches("作品介紹：")
			.trim();
		if !cleaned.is_empty() {
			manga.description = Some(String::from(cleaned));
		}
	}

	if let Some(state) = document.select_first(".state_box").and_then(|el| el.text()) {
		manga.status = status_from_text(&state);
	}

	// Tag links point back at the section they belong to, which is also how the site separates
	// adult titles from everything else -- use that for the per-manga rating.
	let mut is_adult = false;
	let tags = document
		.select(".tag_box a")
		.map(|list| {
			list.into_iter()
				.filter_map(|el| {
					if el.attr("href").is_some_and(|href| href.contains("/r18")) {
						is_adult = true;
					}
					el.text().filter(|t| !t.is_empty())
				})
				.collect::<Vec<String>>()
		})
		.unwrap_or_default();
	if !tags.is_empty() {
		manga.tags = Some(tags);
	}

	manga.content_rating = if is_adult {
		ContentRating::NSFW
	} else {
		ContentRating::Suggestive
	};

	// The reading direction only exists on the chapter page (its `direction` attribute), and the
	// site mixes paged manga with vertical strips inside every section, so nothing here can tell
	// them apart. Leave `viewer` at its default and let the app's own setting decide instead of
	// forcing every title into webtoon mode.
}

/// Parses the chapter list, which ships inline on the detail page.
///
/// The site lists chapters oldest first; Aidoku expects newest first.
pub fn parse_chapters(document: &Document, base: &str) -> Vec<Chapter> {
	let mut chapters = document
		.select("a.item_box")
		.map(|list| {
			list.into_iter()
				.filter_map(|el| {
					let key = chapter_key_from_href(&el.attr("href")?)?;
					let title = el
						.select_first("span.title")
						.and_then(|t| t.text())
						.filter(|t| !t.is_empty());

					// The sibling span holds the chapter's price tier. It is fixed metadata --
					// it reads identically signed in or out -- so it cannot decide whether
					// *this* reader may open the chapter. `locked` therefore stays false,
					// because marking a chapter locked stops Aidoku opening it at all, even
					// after a login unlocks it or the reader buys it.
					let price = el
						.select("span")
						.and_then(|spans| {
							spans
								.into_iter()
								.find(|span| !span.has_class("title"))
								.and_then(|span| span.text())
						})
						.and_then(|raw| price_label(&raw));

					let chapter_number = title.as_deref().and_then(chapter_number_from_title);
					let title = match (title, price) {
						(Some(title), Some(price)) => Some(format!("{title}・{price}")),
						(title, price) => title.or(price),
					};

					Some(Chapter {
						chapter_number,
						url: Some(format!("{base}/comic/chapter/{key}")),
						key,
						title,
						..Default::default()
					})
				})
				.collect::<Vec<Chapter>>()
		})
		.unwrap_or_default();

	chapters.reverse();
	chapters
}

/// Why a chapter refused to hand over its pages, in the site's own terms.
fn locked_reason(code: &str) -> &'static str {
	match code {
		"1" => "需要登入才能閱讀。請到「設定 → 喜漫漫畫 → 帳號」登入後再試一次。",
		"3" => "金幣不足。請先到喜漫漫畫網站儲值，或改看免費章節。",
		"4" => "這一話需要購買，或訂閱會員後才能閱讀。",
		"444" => "今日的免費閱讀額度已用完，明天 00:00 重置。",
		_ => "這一話目前無法閱讀。",
	}
}

/// Turns the price span next to a chapter title into a short suffix for that title.
///
/// The reader cannot be told why a chapter refused to open -- Aidoku replaces a source's error
/// text with its own generic message, and a text page would instead mark the chapter as read.
/// So the cost is surfaced in the chapter list, before anyone taps.
///
/// Returns `None` for free chapters, which need no marker.
fn price_label(raw: &str) -> Option<String> {
	let raw = raw.trim();
	if raw.is_empty() || raw.contains("￥0") || raw.contains("¥0") {
		return None;
	}
	if raw.contains("会员专享") || raw.contains("會員專享") {
		return Some(String::from("會員"));
	}
	// Anything else the site shows here is a coin price, e.g. "0.6".
	if raw.chars().all(|c| c.is_ascii_digit() || c == '.') {
		return Some(format!("{raw} 金幣"));
	}
	Some(String::from(raw))
}

/// The chapter container's `code` attribute. `"0"` means unlocked.
pub fn chapter_lock_code(document: &Document) -> String {
	document
		.select_first(".comic_chapter_box")
		.and_then(|el| el.attr("code"))
		.unwrap_or_default()
}

/// Parses a chapter reader page into pages.
///
/// A locked chapter still answers 200, with three teaser images, so the `code` attribute has to
/// be checked first -- otherwise the reader silently shows a three page "chapter".
pub fn parse_pages(document: &Document) -> Result<Vec<Page>> {
	let code = chapter_lock_code(document);

	// Failing here shows Aidoku's own generic message rather than this text, but it is the only
	// outcome that leaves the chapter unread -- a text page explaining the lock would count as
	// having read the chapter and push the reader's progress forward. The cost is already in the
	// chapter title, and `DynamicSettings` reports the account state.
	if !code.is_empty() && code != "0" {
		bail!("{}", locked_reason(&code));
	}

	// The reader's chosen image route is applied later, in `ImageRequestProvider`, so that
	// covers go through the same single code path.
	let pages = document
		.select("#content img")
		.map(|images| {
			images
				.into_iter()
				.filter_map(|img| img.attr("data-src"))
				.filter(|url| url.contains("/app/comic/"))
				.map(|url| Page {
					content: PageContent::url(url),
					..Default::default()
				})
				.collect::<Vec<Page>>()
		})
		.unwrap_or_default();

	if pages.is_empty() {
		bail!("找不到任何頁面，網站結構可能已變更");
	}

	Ok(pages)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn reads_keys_out_of_hrefs() {
		assert_eq!(
			manga_key_from_href("/comic/detail/1034095446143934464").as_deref(),
			Some("1034095446143934464")
		);
		assert_eq!(
			manga_key_from_href("https://www.favcomic.com/comic/detail/123?x=1").as_deref(),
			Some("123")
		);
		assert_eq!(
			chapter_key_from_href("/comic/chapter/1034160850564423680").as_deref(),
			Some("1034160850564423680")
		);
		assert_eq!(manga_key_from_href("/about").as_deref(), None);
	}

	#[aidoku_test]
	fn reads_chapter_numbers() {
		assert_eq!(chapter_number_from_title("第1话"), Some(1.0));
		assert_eq!(chapter_number_from_title("第113话"), Some(113.0));
		assert_eq!(chapter_number_from_title("第1-2话"), Some(1.0));
		assert_eq!(chapter_number_from_title("第10.5话"), Some(10.5));
		assert_eq!(chapter_number_from_title("序章"), None);
	}

	#[aidoku_test]
	fn swaps_only_known_cdn_hosts() {
		let url = "https://cdn.favcomic.com/file/e-media/app/comic/1/1/1-a.jpg";
		assert_eq!(
			rewrite_image_host(url, "ccdeoo.popwow.top"),
			"https://ccdeoo.popwow.top/file/e-media/app/comic/1/1/1-a.jpg"
		);
		// An unknown preference, an empty preference and foreign hosts pass through untouched.
		assert_eq!(rewrite_image_host(url, "evil.example"), url);
		assert_eq!(rewrite_image_host(url, ""), url);
		assert_eq!(
			rewrite_image_host("https://example.com/a.jpg", "cdn.favcomic.xyz"),
			"https://example.com/a.jpg"
		);
	}

	#[aidoku_test]
	fn reads_status_words() {
		assert_eq!(
			status_from_text("某作品 连载中漫画封面"),
			MangaStatus::Ongoing
		);
		assert_eq!(
			status_from_text("某作品 完结漫画封面"),
			MangaStatus::Completed
		);
		assert_eq!(status_from_text("沒有狀態"), MangaStatus::Unknown);
	}

	#[aidoku_test]
	fn labels_only_chapters_that_cost_something() {
		// Free chapters get no marker at all.
		assert_eq!(price_label("￥0"), None);
		assert_eq!(price_label(" ￥0 "), None);
		assert_eq!(price_label(""), None);

		assert_eq!(price_label("会员专享").as_deref(), Some("會員"));
		assert_eq!(price_label("0.6").as_deref(), Some("0.6 金幣"));
		// The site pads the span with a non-breaking space.
		assert_eq!(price_label("\u{a0}0.6").as_deref(), Some("0.6 金幣"));
		// Anything unrecognised is passed through rather than dropped.
		assert_eq!(price_label("限時免費").as_deref(), Some("限時免費"));
	}

	#[aidoku_test]
	fn explains_every_lock_code() {
		assert!(locked_reason("1").contains("登入"));
		assert!(locked_reason("3").contains("金幣"));
		assert!(locked_reason("4").contains("購買"));
		assert!(locked_reason("444").contains("額度"));
		assert!(!locked_reason("99").is_empty());
	}
}
