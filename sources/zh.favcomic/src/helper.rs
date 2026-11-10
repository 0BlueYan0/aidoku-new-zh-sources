use aidoku::{
	alloc::{format, String, Vec},
	imports::{
		defaults::defaults_get,
		html::{Document, Element},
		net::{Request, Response},
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result,
};

pub const DEFAULT_BASE_URL: &str = "https://www.favcomic.com";

/// Tag ids (shared by every section) that mark a title outside `/r18` as adult: 节操 (41),
/// TL (33), NTR (129), 被NTR (132), 扶他 (131). Chosen by the user on 2026-10-09 after
/// 搭伙变现 (节操) and 新刊太过××× (TL) turned out explicit.
const ADULT_TAG_IDS: [u32; 5] = [41, 33, 129, 132, 131];
/// Tag ids that mark a title as suggestive: BL (19), ABO (125), 百合 (118, 21), 后宫 (40),
/// 后宫·宫廷 (28), 性转换 (45, 25), 伪娘 (83, 82).
const SUGGESTIVE_TAG_IDS: [u32; 10] = [19, 125, 118, 21, 40, 28, 45, 25, 83, 82];

/// `/girl?tag=33` → 33.
fn tag_id_from_href(href: &str) -> Option<u32> {
	let (_, rest) = href.split_once("tag=")?;
	let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
	digits.parse().ok()
}

/// A title is rated by the evidence the site gives; one without any is Safe.
fn content_rating(in_r18: bool, tag_ids: &[u32]) -> ContentRating {
	if in_r18 || tag_ids.iter().any(|id| ADULT_TAG_IDS.contains(id)) {
		ContentRating::NSFW
	} else if tag_ids.iter().any(|id| SUGGESTIVE_TAG_IDS.contains(id)) {
		ContentRating::Suggestive
	} else {
		ContentRating::Safe
	}
}
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
	read_html(request(url)?.send()?, url)
}

/// A locked chapter still answers 200, so anything else is an error page, not content.
pub fn read_html(response: Response, url: &str) -> Result<Document> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[favcomic] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
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

/// Reads the number out of a chapter title such as `第113话` or `第十二话`.
///
/// A range such as `第1-2话` has no single number, so it is `None`. A number followed by `卷`
/// (`3卷`, `01卷後記`) is a volume, not a chapter, so it is `None` too.
pub fn chapter_number_from_title(title: &str) -> Option<f32> {
	let Some(start) = title.find(|c: char| c.is_ascii_digit()) else {
		// Chinese numerals only count in the `第N话` form, so a word is not read as a number.
		let rest = title.trim().strip_prefix('第')?;
		let end = rest.find(['話', '话', '回'])?;
		return chinese_number(rest[..end].trim());
	};
	let rest = &title[start..];
	let end = rest
		.find(|c: char| !c.is_ascii_digit() && c != '.')
		.unwrap_or(rest.len());
	if rest[end..].trim_start().starts_with('卷') {
		return None;
	}
	let mut after = rest[end..].chars();
	if matches!(after.next(), Some('-' | '~' | '～' | '－' | '—'))
		&& after.next().is_some_and(|c| c.is_ascii_digit())
	{
		return None;
	}
	rest[..end].trim_end_matches('.').parse::<f32>().ok()
}

/// Read a Chinese numeral up to 99 (`三`, `十二`, `二十三`).
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

/// Put the optional leading `第` back on a volume title such as `3卷 第5話`, but only when
/// the unit is followed by the end, a space or `第`. `01卷後記`, `一部分` and `三部曲` have
/// text right behind the unit and stay as they are.
fn with_optional_di(title: &str) -> Option<String> {
	let end = title
		.find(|c: char| !(c.is_ascii_digit() || c == '.' || "零一二三四五六七八九十".contains(c)))
		.unwrap_or(title.len());
	let (number, after) = title.split_at(end);
	if number.is_empty() {
		return None;
	}
	let rest = after.strip_prefix(['季', '部', '卷', '冊', '册'])?;
	let boundary =
		rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('第');
	boundary.then(|| format!("第{title}"))
}

/// `(chapter_number, volume_number)` of a chapter title. `第2季 第5话` is (5, 2), `第3卷` is
/// (None, 3), `第113话` is (113, None). A leading volume, season, part or book is the volume,
/// as the official zh.copymanga does. A 卷 followed by text that is not a chapter
/// (`第3卷 後記`) is an extra of that volume and gets neither number, as the official
/// zh.zaimanhua (`is_extra`) does: it would share the volume's number with `第3卷` itself.
pub fn numbers(title: &str) -> (Option<f32>, Option<f32>) {
	let title = title.trim();
	// `3卷 第5話` reads like `第3卷 第5話`, as in the official zh.boylove
	// `chapter_list/mod.rs:111` and zh.zaimanhua where the leading `第` is optional.
	if let Some(prefixed) = with_optional_di(title) {
		return numbers(&prefixed);
	}
	// `卷 3` and `卷3` are not read (official zh.* sources do not parse a 卷 in front of the
	// number). Without this the `3` would be picked up as a chapter number.
	if let Some(rest) = title.strip_prefix('卷') {
		if rest.starts_with(char::is_whitespace) || rest.starts_with(|c: char| c.is_ascii_digit()) {
			return (None, None);
		}
	}
	if let Some(rest) = title.strip_prefix('第') {
		if let Some(end) = rest.find(['卷', '季', '部', '冊', '册']) {
			let number = rest[..end].trim();
			if let Some(volume) = number
				.parse::<f32>()
				.ok()
				.or_else(|| chinese_number(number))
			{
				let unit = rest[end..].chars().next().unwrap_or('卷');
				let after = rest[end + unit.len_utf8()..].trim();
				let chapter = chapter_number_from_title(after);
				if unit == '卷' && !after.is_empty() && chapter.is_none() {
					return (None, None);
				}
				return (chapter, Some(volume));
			}
		}
	}
	(chapter_number_from_title(title), bare_volume(title))
}

/// `3卷` → 3 and `三卷` → 3. Only a title that is just the number and `卷` counts, as with
/// `第3卷`; `3卷後記` is an appendix and gives `None`.
fn bare_volume(title: &str) -> Option<f32> {
	let number = title.trim().strip_suffix('卷')?.trim();
	number
		.parse::<f32>()
		.ok()
		.or_else(|| chinese_number(number))
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

	// Tag links point back at the section they belong to (`/r18?tag=116`), which is how the
	// site separates adult titles. The boy and girl sections hold explicit titles too, under
	// tags such as 节操 and TL, so the tag ids are checked as well.
	let mut is_adult = false;
	let mut tag_ids: Vec<u32> = Vec::new();
	let tags = document
		.select(".tag_box a")
		.map(|list| {
			list.into_iter()
				.filter_map(|el| {
					if let Some(href) = el.attr("href") {
						if href.contains("/r18") {
							is_adult = true;
						}
						if let Some(id) = tag_id_from_href(&href) {
							tag_ids.push(id);
						}
					}
					el.text().filter(|t| !t.is_empty())
				})
				.collect::<Vec<String>>()
		})
		.unwrap_or_default();
	if !tags.is_empty() {
		manga.tags = Some(tags);
	}

	manga.content_rating = content_rating(is_adult, &tag_ids);

	// The reading direction only exists on the chapter page (its `direction` attribute), and the
	// site mixes paged manga with vertical strips inside every section, so nothing here can tell
	// them apart. Leave `viewer` at its default and let the app's own setting decide instead of
	// forcing every title into webtoon mode.
}

/// Parses the chapter list, which ships inline on the detail page.
///
/// The site lists chapters oldest first; Aidoku expects newest first.
///
/// `None` when an entry has no chapter link: a list with a chapter missing would replace
/// the library's complete one.
pub fn parse_chapters(document: &Document, base: &str, logged_in: bool) -> Option<Vec<Chapter>> {
	let mut chapters = document
		.select("a.item_box")
		.map(|list| {
			list.into_iter()
				.map(|el| {
					let key = chapter_key_from_href(&el.attr("href")?)?;
					let title = el
						.select_first("span.title")
						.and_then(|t| t.text())
						.filter(|t| !t.is_empty());

					// The sibling span holds the chapter's price tier. It is fixed metadata -- it
					// reads identically signed in or out -- so it cannot show whether this account
					// has bought the chapter. A signed-in reader is therefore never locked out here
					// and the site decides at the reader. A signed-out reader cannot open a member
					// or coin chapter for certain, so those are locked. Signing in and refreshing
					// the manga page unlocks them again.
					let price = el
						.select("span")
						.and_then(|spans| {
							spans
								.into_iter()
								.find(|span| !span.has_class("title"))
								.and_then(|span| span.text())
						})
						.and_then(|raw| price_label(&raw));

					let locked = !logged_in && price.as_deref().is_some_and(needs_payment);
					let (chapter_number, volume_number) =
						title.as_deref().map(numbers).unwrap_or((None, None));
					let title = match (title, price) {
						(Some(title), Some(price)) => Some(format!("{title}・{price}")),
						(title, price) => title.or(price),
					};

					Some(Chapter {
						chapter_number,
						volume_number,
						url: Some(format!("{base}/comic/chapter/{key}")),
						key,
						title,
						locked,
						..Default::default()
					})
				})
				.collect::<Option<Vec<Chapter>>>()
		})
		.unwrap_or_else(|| Some(Vec::new()))?;

	// Only a list where no title carries a chapter or volume number is numbered by place (oldest is 1). In a
	// mixed list the extras and ranges stay `None`, so they never share a number with a real
	// chapter; the official zh.jmcomic does the same.
	if chapters
		.iter()
		.all(|c| c.chapter_number.is_none() && c.volume_number.is_none())
	{
		for (index, chapter) in chapters.iter_mut().enumerate() {
			chapter.chapter_number = Some((index + 1) as f32);
		}
	}

	chapters.reverse();
	Some(chapters)
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

/// Whether a price label from `price_label` stands for a member or coin chapter, which a
/// signed-out reader can not open. Other labels, such as a limited-time free offer, can.
fn needs_payment(label: &str) -> bool {
	label == "會員" || label.ends_with(" 金幣")
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

	/// In a mixed list the unnumbered titles stay `None`; a list with no number at all is
	/// numbered by place, oldest first.
	#[aidoku_test]
	fn unnumbered_titles_are_numbered_by_place_only_when_no_title_has_a_number() {
		let parse = |html: &str| {
			let document = aidoku::imports::html::Html::parse(html).expect("parse");
			let chapters = parse_chapters(&document, "https://www.favcomic.com", true).unwrap();
			chapters
				.iter()
				.map(|c| c.chapter_number)
				.collect::<aidoku::alloc::Vec<Option<f32>>>()
		};
		let mixed = parse(concat!(
			r#"<a class="item_box" href="/comic/chapter/1"><span class="title">序章</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/2"><span class="title">第1话</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/3"><span class="title">第2-3话</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/4"><span class="title">番外</span></a>"#,
		));
		// Newest first: 番外, 第2-3话, 第1话, 序章.
		assert_eq!(mixed, [None, None, Some(1.0), None]);
		let none = parse(concat!(
			r#"<a class="item_box" href="/comic/chapter/1"><span class="title">序章</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/2"><span class="title">番外</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/3"><span class="title">後記</span></a>"#,
		));
		assert_eq!(none, [Some(3.0), Some(2.0), Some(1.0)]);
	}

	#[aidoku_test]
	fn volumes_seasons_and_appendices() {
		assert_eq!(numbers("第113话"), (Some(113.0), None));
		assert_eq!(numbers("第3卷"), (None, Some(3.0)));
		assert_eq!(numbers("第二季"), (None, Some(2.0)));
		assert_eq!(numbers("第2季 第5话"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("第3卷 後記"), (None, None));
		assert_eq!(numbers("第3卷 新的旅程"), (None, None));
		assert_eq!(numbers("第1-2话"), (None, None));
		assert_eq!(numbers("序章"), (None, None));
		assert_eq!(numbers("3卷"), (None, Some(3.0)));
		assert_eq!(numbers("三卷"), (None, Some(3.0)));
		assert_eq!(numbers("01卷後記"), (None, None));
		assert_eq!(numbers("06卷番外"), (None, None));
		assert_eq!(numbers("第十二话"), (Some(12.0), None));
		assert_eq!(numbers("第2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("第十二話"), (Some(12.0), None));
		assert_eq!(numbers("第二季 第十二话"), (Some(12.0), Some(2.0)));
	}

	/// Volumes count as numbers, so an extra next to them stays unnumbered.
	#[aidoku_test]
	fn a_list_of_volumes_and_an_extra_is_not_numbered_by_place() {
		let document = aidoku::imports::html::Html::parse(concat!(
			r#"<a class="item_box" href="/comic/chapter/1"><span class="title">第1卷</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/2"><span class="title">番外</span></a>"#,
		))
		.expect("parse");
		let chapters = parse_chapters(&document, "https://www.favcomic.com", true).unwrap();
		assert_eq!(chapters[0].chapter_number, None);
		assert_eq!(chapters[0].volume_number, None);
		assert_eq!(chapters[1].volume_number, Some(1.0));
	}

	#[aidoku_test]
	fn a_chapter_without_a_link_fails_the_list() {
		let parse = |html: &str| {
			let document = aidoku::imports::html::Html::parse(html).expect("parse");
			parse_chapters(&document, "https://www.favcomic.com", true)
		};
		let chapters = parse(concat!(
			r#"<a class="item_box" href="/comic/chapter/1"><span class="title">第1话</span></a>"#,
			r#"<a class="item_box" href="/comic/chapter/2"><span class="title">第2话</span></a>"#,
		))
		.expect("complete list");
		assert_eq!(chapters.len(), 2);
		assert_eq!(chapters[0].key, "2");
		assert!(parse(concat!(
			r#"<a class="item_box" href="/comic/chapter/1"><span class="title">第1话</span></a>"#,
			r#"<a class="item_box" href="/comic/"><span class="title">第2话</span></a>"#,
		))
		.is_none());
	}

	#[aidoku_test]
	fn rates_by_section_and_tags() {
		assert_eq!(tag_id_from_href("/girl?tag=33"), Some(33));
		assert_eq!(tag_id_from_href("/boy?tag=41&page=2"), Some(41));
		assert_eq!(tag_id_from_href("/boy"), None);
		assert_eq!(content_rating(true, &[]), ContentRating::NSFW);
		assert_eq!(content_rating(false, &[36, 41]), ContentRating::NSFW);
		assert_eq!(content_rating(false, &[33, 46]), ContentRating::NSFW);
		assert_eq!(content_rating(false, &[19]), ContentRating::Suggestive);
		assert_eq!(content_rating(false, &[36]), ContentRating::Safe);
		assert_eq!(content_rating(false, &[]), ContentRating::Safe);
	}

	#[aidoku_test]
	fn tag_ids_match_filters_json() {
		let json = include_str!("../res/filters.json");
		for id in ADULT_TAG_IDS.iter().chain(SUGGESTIVE_TAG_IDS.iter()) {
			assert!(json.contains(&format!("\"{id}\"")), "{id}");
		}
	}

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
		assert_eq!(chapter_number_from_title("第1-2话"), None);
		assert_eq!(chapter_number_from_title("第10.5话"), Some(10.5));
		assert_eq!(chapter_number_from_title("序章"), None);
		// The leading `第` of a volume may be left out.
		assert_eq!(numbers("3卷 第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("3卷第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("三卷 第五話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("3卷 後記"), (None, None));
		assert_eq!(numbers("3卷卷首"), (None, None));
		assert_eq!(numbers("一部分"), (None, None));
		// Not a volume (`曲` follows the unit), so the old reading stays: the bare `2` is a chapter.
		assert_eq!(numbers("三部曲 第2話"), (Some(2.0), None));
		assert_eq!(numbers("卷三"), (None, None));
		assert_eq!(numbers("卷 3"), (None, None));
		assert_eq!(numbers("卷3"), (None, None));
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

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
