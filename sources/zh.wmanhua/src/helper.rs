use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		html::{Document, Element},
		net::{Request, Response},
		std::parse_date,
	},
	prelude::*,
	Chapter, ContentRating, Manga, MangaPageResult, MangaStatus, Page, PageContent, Result, Viewer,
};

pub const BASE_URL: &str = "https://www.wmanhua.com";

/// The only listing: `/sort` in its default order (last updated first) is the one paged
/// list the site has. `/hot` boards hold 5 to 10 works and do not page.
pub const UPDATE_LISTING: &str = "update";

/// Kept apart from sending so the home can send its two pages together.
pub fn request(url: &str) -> Result<Request> {
	Ok(Request::get(url)?)
}

/// A non-2xx answer is an error, so an error page is never read as an empty list.
pub fn read_html(response: Response, url: &str) -> Result<Document> {
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[wmanhua] HTTP {status} for {url}");
	}
	Ok(response.get_html()?)
}

/// `/sort` and `/search` pages. The site answers 404 for a `/sort` page past the last one
/// (`/sort?page=999`, 2026-11-03), which is an empty last page, not an error. A first page
/// never does (`/sort?tag=66` with no works answers 200), so a 404 there is an error.
pub fn fetch_list(url: &str, page: i32) -> Result<MangaPageResult> {
	let response = request(url)?.send()?;
	if page > 1 && response.status_code() == 404 {
		return Ok(empty_page());
	}
	Ok(parse_manga_list(&read_html(response, url)?))
}

/// The details page, or `None` when the site has no such work. The site answers 404 with
/// 「当前内容不存在」 (2026-11-03); any other page without a title is an error, so a
/// maintenance page does not mark the library's works as removed.
pub fn fetch_details_page(key: &str) -> Result<Option<Document>> {
	let url = manga_url(key);
	let response = request(&url)?.send()?;
	let status = response.status_code();
	if status == 404 {
		let html = response.get_html()?;
		if is_missing_page(&html) {
			return Ok(None);
		}
		bail!("[wmanhua] HTTP 404 without the missing-work notice for {url}");
	}
	let html = read_html(response, &url)?;
	if html.select_first(".comic-profile h1").is_some() {
		Ok(Some(html))
	} else {
		bail!("[wmanhua] no details on {url}")
	}
}

pub fn empty_page() -> MangaPageResult {
	MangaPageResult {
		entries: Vec::new(),
		has_next_page: false,
	}
}

pub fn is_missing_page(html: &Document) -> bool {
	html.select_first(".comic-profile h1").is_none()
		&& html
			.select_first("main")
			.and_then(|main: Element| main.text())
			.is_some_and(|text: String| text.contains("当前内容不存在"))
}

/// Every chapter of a work: the details page shows only the newest 12 and fetches the
/// rest with `POST /comic/<id>`.
pub fn chapters_request(key: &str) -> Result<Request> {
	Ok(Request::post(format!("{BASE_URL}/comic/{key}"))?)
}

/// `Ok(None)` when the site lists no chapters (`null` or `[]`); see `parse_chapters`.
pub fn fetch_chapters(key: &str) -> Result<Option<Vec<Chapter>>> {
	let response = chapters_request(key)?.send()?;
	let status = response.status_code();
	if !(200..300).contains(&status) {
		bail!("[wmanhua] HTTP {status} for the chapters of {key}");
	}
	parse_chapters(&response.get_string()?, key)
}

pub fn manga_url(key: &str) -> String {
	format!("{BASE_URL}/comic/{key}.html")
}

pub fn chapter_url(manga_key: &str, chapter_id: &str) -> String {
	format!("{BASE_URL}/chapter/{manga_key}-{chapter_id}.html")
}

/// `/sort`. The four filters combine freely (`category=1&tag=15&status=1&order=asc`,
/// 2026-11-03); each takes one value. A filter left at its default is not sent.
pub fn browse_url(
	category: Option<&str>,
	tag: Option<&str>,
	status: Option<&str>,
	order: Option<&str>,
	page: i32,
) -> String {
	let mut url = format!("{BASE_URL}/sort?page={page}");
	for (name, value) in [
		("category", category),
		("tag", tag),
		("status", status),
		("order", order),
	] {
		if let Some(value) = value {
			url.push_str(&format!("&{name}={value}"));
		}
	}
	url
}

/// A listing's address; `None` for unknown ids.
pub fn listing_url(id: &str, page: i32) -> Option<String> {
	(id == UPDATE_LISTING).then(|| browse_url(None, None, None, None, page))
}

/// `/search?query=<keyword>&page=N`. Matches titles and authors, and ignores the `/sort`
/// filters.
pub fn search_url(query: &str, page: i32) -> String {
	format!(
		"{BASE_URL}/search?query={}&page={page}",
		encode_uri_component(query.trim())
	)
}

/// `/comic/5342.html` (relative or absolute) → `5342`.
pub fn comic_id(href: &str) -> Option<String> {
	let (_, rest) = href.split_once("/comic/")?;
	let (id, _) = rest.split_once(".html")?;
	is_id(id).then(|| String::from(id))
}

/// `/chapter/5342-1035448.html` (relative or absolute) → `("5342", "1035448")`.
pub fn chapter_path(href: &str) -> Option<(String, String)> {
	let (_, rest) = href.split_once("/chapter/")?;
	let (path, _) = rest.split_once(".html")?;
	let (work, chapter) = path.split_once('-')?;
	(is_id(work) && is_id(chapter)).then(|| (String::from(work), String::from(chapter)))
}

pub fn is_id(text: &str) -> bool {
	!text.is_empty() && text.chars().all(|c: char| c.is_ascii_digit())
}

fn text_of(el: &Element) -> String {
	el.text()
		.map(|t: String| String::from(t.trim()))
		.unwrap_or_default()
}

/// The address in `background-image:url('…')`, which list cards and the details page use
/// for covers.
pub fn style_url(style: &str) -> Option<String> {
	let (_, rest) = style.split_once("url(")?;
	let (inner, _) = rest.split_once(')')?;
	let url = inner.trim().trim_matches(['\'', '"']);
	url.starts_with("http").then(|| String::from(url))
}

fn background_cover(el: &Element) -> Option<String> {
	el.attr("style").and_then(|style: String| style_url(&style))
}

/// Tags that raise a work's rating. The site has no adult flag, so the tags are the only
/// evidence; a work without one of them is `Safe`. Chosen by the user (2026-11-03), as in
/// zh.wmh1234. The site lists 后宫 and 後宮 as two tags. Each name is in `tags::TAGS`.
pub const SUGGESTIVE_TAGS: [&str; 5] = ["耽美", "百合", "后宫", "後宮", "TL"];

pub fn content_rating<S: AsRef<str>>(tags: &[S]) -> ContentRating {
	if tags
		.iter()
		.any(|t: &S| SUGGESTIVE_TAGS.contains(&t.as_ref()))
	{
		ContentRating::Suggestive
	} else {
		ContentRating::Safe
	}
}

/// By the details page's 类型. Japanese pages are page-sized (650x897), Korean ones long
/// strips (1080x844 slices), 2026-11-03. Hong Kong and Taiwan comics read right to left
/// like Japanese ones; the site has one.
pub fn viewer(category: &str) -> Viewer {
	match category {
		"日本漫画" | "港台漫画" => Viewer::RightToLeft,
		"韩国漫画" | "大陆漫画" => Viewer::Webtoon,
		_ => Viewer::Unknown,
	}
}

/// `第 2 / 6 页` in the list header.
fn has_next_page(html: &Document) -> bool {
	let Some(text) = html
		.select_first(".filter-title span")
		.map(|el: Element| text_of(&el))
	else {
		return false;
	};
	let Some((_, pages)) = text.split_once('第') else {
		return false;
	};
	let Some((pages, _)) = pages.split_once('页') else {
		return false;
	};
	let Some((current, last)) = pages.split_once('/') else {
		return false;
	};
	match (current.trim().parse::<i32>(), last.trim().parse::<i32>()) {
		(Ok(current), Ok(last)) => current < last,
		_ => false,
	}
}

/// A card that links a work and shows its cover as a background (`.thumb`, `.mini`).
/// Cards name no tags or type; the details page sets rating and viewer.
fn card_manga(card: &Element) -> Option<Manga> {
	let key = card
		.attr("href")
		.or_else(|| {
			card.select_first("a[href]")
				.and_then(|a: Element| a.attr("href"))
		})
		.and_then(|href: String| comic_id(&href))?;
	let title = card
		.select_first("h3")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = card
		.select_first(".thumb, .mini")
		.and_then(|el: Element| background_cover(&el));
	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		..Default::default()
	})
}

fn push_unique(entries: &mut Vec<Manga>, manga: Manga) {
	if !entries.iter().any(|m: &Manga| m.key == manga.key) {
		entries.push(manga);
	}
}

/// The `/sort` and `/search` pages: `section.catalog a.book`, 24 a page.
pub fn parse_manga_list(html: &Document) -> MangaPageResult {
	let mut entries: Vec<Manga> = Vec::new();
	for card in html.select("section.catalog a.book").into_iter().flatten() {
		if let Some(manga) = card_manga(&card) {
			push_unique(&mut entries, manga);
		}
	}
	let has_next_page = !entries.is_empty() && has_next_page(html);
	MangaPageResult {
		entries,
		has_next_page,
	}
}

/// The rows of the site's front page, in its order.
pub struct HomeRows {
	/// 今日推荐: big cards with a blurb.
	pub featured: Vec<Manga>,
	/// 热门漫画.
	pub popular: Vec<Manga>,
	/// 本周排行: a numbered list; some entries have no cover, and which ones changes
	/// (1 of 7 and 5 of 7 had one on two loads, 2026-11-03).
	pub ranking: Vec<Manga>,
	/// 最近更新.
	pub latest: Vec<Manga>,
}

/// A front-page card: a cover `<img>` and a title in `h3 a`.
fn home_card(card: &Element) -> Option<Manga> {
	let link = card.select_first("h3 a[href]")?;
	let key = link.attr("href").and_then(|href: String| comic_id(&href))?;
	let title = Some(text_of(&link)).filter(|t: &String| !t.is_empty())?;
	let cover = card
		.select_first("img[src]")
		.and_then(|img: Element| img.attr("src"))
		.filter(|src: &String| src.starts_with("http"));
	Some(Manga {
		url: Some(manga_url(&key)),
		key,
		title,
		cover,
		..Default::default()
	})
}

fn home_cards(html: &Document, selector: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	for card in html.select(selector).into_iter().flatten() {
		if let Some(manga) = home_card(&card) {
			push_unique(&mut entries, manga);
		}
	}
	entries
}

pub fn parse_home(html: &Document) -> HomeRows {
	let mut featured: Vec<Manga> = Vec::new();
	for card in html
		.select(".comic-featured article.comic-feature")
		.into_iter()
		.flatten()
	{
		if let Some(mut manga) = home_card(&card) {
			manga.description = card
				.select_first(".comic-brief")
				.map(|el: Element| text_of(&el))
				.filter(|d: &String| !d.is_empty());
			push_unique(&mut featured, manga);
		}
	}
	HomeRows {
		featured,
		popular: home_cards(html, ".comic-shelf article.comic-book"),
		ranking: home_cards(html, ".comic-ranking ol li"),
		latest: home_cards(html, ".comic-updates article.comic-update"),
	}
}

/// Gives each entry without a cover the cover another row shows for the same work.
/// The front page's 本周排行 leaves most covers out, and `/hot` has them
/// (the 7 works on 2026-11-03 were all on the 人气榜 board, with covers).
pub fn fill_covers(entries: &mut [Manga], sources: &[&[Manga]]) {
	for manga in entries.iter_mut().filter(|m| m.cover.is_none()) {
		manga.cover = sources
			.iter()
			.flat_map(|row| row.iter())
			.find(|other| other.key == manga.key && other.cover.is_some())
			.and_then(|other| other.cover.clone());
	}
}

/// One board of `/hot`: the tab panel `hot` (人气榜), `new` (新作榜) or `fav` (收藏榜).
pub fn parse_hot(html: &Document, panel: &str) -> Vec<Manga> {
	let mut entries: Vec<Manga> = Vec::new();
	let selector = format!(".tab-panel[data-panel=\"{panel}\"] a.book");
	for card in html.select(&selector).into_iter().flatten() {
		if let Some(manga) = card_manga(&card) {
			push_unique(&mut entries, manga);
		}
	}
	entries
}

/// The value after `<b>label：</b>` in `.profile-meta`.
fn profile_meta(html: &Document, label: &str) -> Option<String> {
	for p in html.select(".profile-meta p").into_iter().flatten() {
		let name = p
			.select_first("b")
			.map(|b: Element| text_of(&b))
			.unwrap_or_default();
		if name.trim_end_matches(['：', ':']) == label {
			let text = text_of(&p);
			let value = text.strip_prefix(name.as_str()).unwrap_or(&text).trim();
			return (!value.is_empty()).then(|| String::from(value));
		}
	}
	None
}

pub fn parse_details(html: &Document, key: &str) -> Option<Manga> {
	let title = html
		.select_first(".comic-profile h1")
		.map(|el: Element| text_of(&el))
		.filter(|t: &String| !t.is_empty())?;
	let cover = html
		.select_first(".comic-profile .cover")
		.and_then(|el: Element| background_cover(&el));
	let status = match html
		.select_first(".profile-title span")
		.map(|el: Element| text_of(&el))
		.as_deref()
	{
		Some(s) if s.contains("完结") => MangaStatus::Completed,
		Some(s) if s.contains("连载") => MangaStatus::Ongoing,
		_ => MangaStatus::Unknown,
	};
	let authors: Vec<String> = profile_meta(html, "作者")
		.map(|names: String| {
			names
				.split(['&', '/', '、', ',', '，'])
				.map(|n: &str| String::from(n.trim()))
				.filter(|n: &String| !n.is_empty())
				.collect()
		})
		.unwrap_or_default();
	let category = profile_meta(html, "类型").unwrap_or_default();
	let mut tags: Vec<String> = Vec::new();
	for a in html.select(".profile-tags a").into_iter().flatten() {
		let name = text_of(&a);
		if !name.is_empty() && !tags.contains(&name) {
			tags.push(name);
		}
	}
	let description = html
		.select_first(".profile-desc")
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
		viewer: viewer(&category),
		tags: (!tags.is_empty()).then_some(tags),
		status,
		..Default::default()
	})
}

/// `第158话` → 158, `72话 大结局` → 72, `176.拉扯` → 176. `新年番外` gives `None`,
/// and neither does a volume (`第27卷`), which is a `volume_number` instead.
pub fn chapter_number(title: &str) -> Option<f32> {
	leading_number(title)
		.filter(|&(_, unit): &(f32, Option<char>)| unit != Some('卷'))
		.map(|(n, _): (f32, Option<char>)| n)
}

/// `第27卷` → 27. Only a title that is just the volume counts. `01卷後記` and `06卷番外`
/// are appendixes of a volume and get none, as `numbers` does for `第3卷 後記`, because
/// the volume itself is another entry (`第01卷`) and they would share its number.
pub fn volume_number(title: &str) -> Option<f32> {
	if !title.trim().ends_with('卷') {
		return None;
	}
	leading_number(title)
		.filter(|&(_, unit): &(f32, Option<char>)| unit == Some('卷'))
		.map(|(n, _): (f32, Option<char>)| n)
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

/// `第2季 第5话` → (Some(5), Some(2)), `第二季` → (None, Some(2)), `第3卷` → (None, Some(3)),
/// `第158话` → (Some(158), None). A leading volume, season, part or book is the volume, as
/// the official zh.copymanga does. A volume followed by text that is not a chapter
/// (`第3卷 後記`) is an appendix of that volume and gets none, as the official zh.zaimanhua
/// does (`is_extra`): it would share the volume's number with `第3卷` itself.
/// A list where no title has a number is numbered by position in `parse_chapters`.
pub fn numbers(title: &str) -> (Option<f32>, Option<f32>) {
	let title = title.trim();
	// `3卷 第5話` reads like `第3卷 第5話`, as in the official zh.boylove
	// `chapter_list/mod.rs:111` and zh.zaimanhua where the leading `第` is optional.
	if let Some(prefixed) = with_optional_di(title) {
		return numbers(&prefixed);
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
				let chapter = chapter_number(after);
				if unit == '卷' && !after.is_empty() && chapter.is_none() {
					return (None, None);
				}
				return (chapter, Some(volume));
			}
		}
	}
	(chapter_number(title), volume_number(title))
}

/// The number a title opens with and the unit char right after it, if any.
fn leading_number(title: &str) -> Option<(f32, Option<char>)> {
	let title = title.trim();
	let rest = title.strip_prefix('第').unwrap_or(title);
	let end = rest
		.char_indices()
		.find(|&(_, c): &(usize, char)| !(c.is_ascii_digit() || c == '.'))
		.map_or(rest.len(), |(i, _): (usize, char)| i);
	let number = rest[..end].trim_end_matches('.');
	if number.is_empty() {
		// `第十二话`: a Chinese numeral before the unit.
		let unit_at = rest.find(['话', '話', '回', '章', '卷', '集'])?;
		let n = chinese_number(&rest[..unit_at])?;
		return Some((n, rest[unit_at..].chars().next()));
	}
	let next = rest[end..].chars().next();
	let marked = matches!(next, Some('话' | '話' | '回' | '章' | '卷' | '集'));
	// A bare number counts only when it leads the title (`780 大只佬`, `176.拉扯`);
	// `第` without a unit after it (`第2季`) does not.
	if title.starts_with('第') && !marked {
		return None;
	}
	number.parse::<f32>().ok().map(|n: f32| (n, next))
}

// ---------------------------------------------------------------------------
// Hand-written JSON scanning, as in zh.tibiu and zh.komiic2. The chapter list is a flat
// array of objects with string and number fields.
// ---------------------------------------------------------------------------

/// Tracks whether a scan position sits inside a JSON string literal.
#[derive(Default)]
struct StringScanner {
	in_string: bool,
	escaped: bool,
}

impl StringScanner {
	/// Feed the next character; returns true only if it is structural punctuation.
	fn is_structural(&mut self, ch: char) -> bool {
		if self.in_string {
			if self.escaped {
				self.escaped = false;
			} else if ch == '\\' {
				self.escaped = true;
			} else if ch == '"' {
				self.in_string = false;
			}
			return false;
		}
		if ch == '"' {
			self.in_string = true;
			return false;
		}
		true
	}
}

/// `"key":[{...},{...}]` → the `{...}` strings. `None` when the key is missing or its
/// value is not an array (the site sends `"chapters":null` for a work it does not have).
pub fn json_array_objects<'a>(json: &'a str, key: &str) -> Option<Vec<&'a str>> {
	let search = format!("\"{key}\":");
	let pos = json.find(&search)? + search.len();
	let body = json[pos..].trim_start().strip_prefix('[')?;
	let mut results: Vec<&str> = Vec::new();
	let mut scanner = StringScanner::default();
	let mut depth = 0i32;
	let mut obj_start: Option<usize> = None;
	for (i, ch) in body.char_indices() {
		if !scanner.is_structural(ch) {
			continue;
		}
		match ch {
			'{' => {
				if depth == 0 {
					obj_start = Some(i);
				}
				depth += 1;
			}
			'}' => {
				depth -= 1;
				if depth == 0 {
					if let Some(start) = obj_start.take() {
						results.push(&body[start..=i]);
					}
				}
			}
			']' if depth == 0 => return Some(results),
			_ => {}
		}
	}
	// The array never closed: a cut-off answer.
	None
}

/// `"key":"value"` → `value`, unescaped.
pub fn json_string(json: &str, key: &str) -> Option<String> {
	let search = format!("\"{key}\":\"");
	let start = json.find(&search)? + search.len();
	let mut out = String::new();
	let mut chars = json[start..].chars();
	while let Some(ch) = chars.next() {
		match ch {
			'"' => return Some(out),
			'\\' => match chars.next()? {
				'n' => out.push('\n'),
				't' => out.push('\t'),
				'r' => out.push('\r'),
				'u' => {
					let code: String = chars.by_ref().take(4).collect();
					let code = u32::from_str_radix(&code, 16).ok()?;
					out.push(char::from_u32(code)?);
				}
				other => out.push(other),
			},
			ch => out.push(ch),
		}
	}
	None
}

/// `"key":123` → 123. The digits have to start where the value starts.
pub fn json_number(json: &str, key: &str) -> Option<i64> {
	let search = format!("\"{key}\":");
	let pos = json.find(&search)? + search.len();
	let rest = json[pos..].trim_start();
	let end = rest
		.char_indices()
		.find(|&(i, ch): &(usize, char)| !(ch.is_ascii_digit() || (i == 0 && ch == '-')))
		.map_or(rest.len(), |(i, _): (usize, char)| i);
	rest[..end].parse::<i64>().ok()
}

/// The answer of `POST /comic/<id>`: `data.chapters[]`, oldest first by id. Returned
/// newest first, as the app wants. `Ok(None)` when the site lists none (`null`, sent for
/// a work it does not have, or `[]`). An answer without the field, cut off, or with a
/// chapter lacking its id is an error, so the library keeps its chapters instead of a
/// list with one missing.
pub fn parse_chapters(json: &str, manga_key: &str) -> Result<Option<Vec<Chapter>>> {
	let compact: String = json.chars().filter(|c: &char| !c.is_whitespace()).collect();
	if compact.contains("\"chapters\":null") {
		return Ok(None);
	}
	let Some(objects) = json_array_objects(json, "chapters") else {
		bail!("[wmanhua] unreadable chapter list for {manga_key}");
	};
	if objects.is_empty() {
		return Ok(None);
	}
	let mut chapters: Vec<Chapter> = Vec::with_capacity(objects.len());
	for obj in objects.into_iter().rev() {
		let Some(id) = json_number(obj, "id") else {
			bail!("[wmanhua] a chapter of {manga_key} has no id");
		};
		let key = format!("{id}");
		let title = json_string(obj, "chapterName")
			.map(|t: String| String::from(t.trim()))
			.filter(|t: &String| !t.is_empty());
		// `createdAt` is UTC: the reader shows 2026-09-18 02:28:31 as 10:28 (UTC+8).
		let date_uploaded = json_string(obj, "createdAt")
			.and_then(|date: String| parse_date(date, "yyyy-MM-dd HH:mm:ss"));
		let (chapter_number, volume_number) = title.as_deref().map_or((None, None), numbers);
		chapters.push(Chapter {
			url: Some(chapter_url(manga_key, &key)),
			chapter_number,
			volume_number,
			title,
			date_uploaded,
			key,
			..Default::default()
		});
	}
	// When no title in the list has a number, the position in the list (oldest is 1) is the
	// chapter number, so the tracker still has one to sync. A list that mixes numbered and
	// unnumbered titles keeps `None` for the unnumbered ones, as the official zh.jmcomic does.
	// The list is newest first, so the position is `len - index`.
	if chapters
		.iter()
		.all(|c: &Chapter| c.chapter_number.is_none() && c.volume_number.is_none())
	{
		let len = chapters.len();
		for (index, chapter) in chapters.iter_mut().enumerate() {
			chapter.chapter_number = Some((len - index) as f32);
		}
	}
	Ok(Some(chapters))
}

/// A single-quoted JavaScript string at the start of `text`, unescaped, and what follows it.
fn js_string(text: &str) -> Option<(String, &str)> {
	let rest = text.strip_prefix('\'')?;
	let mut out = String::new();
	let mut chars = rest.char_indices();
	while let Some((i, ch)) = chars.next() {
		match ch {
			'\'' => return Some((out, &rest[i + 1..])),
			'\\' => out.push(chars.next()?.1),
			ch => out.push(ch),
		}
	}
	None
}

/// The reader's images. The page builds them in a script (2026-11-03):
///
/// ```js
/// var totalPages = parseInt('6');
/// var imgBaseUrl = 'https://image4.wmanhua.com/mh/<hash>/' + ('<chapterPath>' || '<name>') + '/';
/// … img.setAttribute('data-src', imgBaseUrl + i + '.webp');   // i = 1..totalPages
/// ```
///
/// A page that does not have both fails the chapter instead of reading as empty.
pub fn parse_pages(page: &str) -> Result<Vec<Page>> {
	let Some(total) = page
		.split_once("var totalPages = parseInt(")
		.and_then(|(_, rest)| js_string(rest.trim_start()))
		.and_then(|(n, _)| n.trim().parse::<usize>().ok())
	else {
		bail!("[wmanhua] no page count in the reader");
	};
	let Some(base) = image_base(page) else {
		bail!("[wmanhua] no image address in the reader");
	};
	if total == 0 {
		bail!("[wmanhua] the reader lists no pages");
	}
	Ok((1..=total)
		.map(|i: usize| Page {
			content: PageContent::url(format!("{base}{i}.webp")),
			..Default::default()
		})
		.collect())
}

/// `imgBaseUrl`: the work's folder, then the chapter's folder (its `chapterPath`, or its
/// name when the path is empty, as the page's `||` does), then `/`.
fn image_base(page: &str) -> Option<String> {
	let (_, rest) = page.split_once("var imgBaseUrl = ")?;
	let (prefix, rest) = js_string(rest)?;
	let rest = rest.trim_start().strip_prefix('+')?.trim_start();
	let rest = rest.strip_prefix('(')?.trim_start();
	let (path, rest) = js_string(rest)?;
	let rest = rest.trim_start().strip_prefix("||")?.trim_start();
	let (name, rest) = js_string(rest)?;
	let rest = rest.trim_start().strip_prefix(')')?.trim_start();
	let rest = rest.strip_prefix('+')?.trim_start();
	let (suffix, _) = js_string(rest)?;
	if !prefix.starts_with("http") {
		return None;
	}
	let folder = if path.is_empty() {
		encode_uri_component(name)
	} else {
		path
	};
	if folder.is_empty() {
		return None;
	}
	Some(format!("{prefix}{folder}{suffix}"))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn builds_addresses() {
		assert_eq!(
			browse_url(None, None, None, None, 1),
			"https://www.wmanhua.com/sort?page=1"
		);
		assert_eq!(
			browse_url(Some("1"), Some("15"), Some("1"), Some("asc"), 2),
			"https://www.wmanhua.com/sort?page=2&category=1&tag=15&status=1&order=asc"
		);
		assert_eq!(
			listing_url("update", 3).as_deref(),
			Some("https://www.wmanhua.com/sort?page=3")
		);
		assert_eq!(listing_url("hot", 1), None);
		assert_eq!(
			search_url(" 一人 ", 2),
			"https://www.wmanhua.com/search?query=%E4%B8%80%E4%BA%BA&page=2"
		);
		assert_eq!(manga_url("5342"), "https://www.wmanhua.com/comic/5342.html");
		assert_eq!(
			chapter_url("5342", "1035448"),
			"https://www.wmanhua.com/chapter/5342-1035448.html"
		);
	}

	#[aidoku_test]
	fn reads_ids() {
		assert_eq!(comic_id("/comic/5342.html").as_deref(), Some("5342"));
		assert_eq!(
			comic_id("https://www.wmanhua.com/comic/5342.html?x=1").as_deref(),
			Some("5342")
		);
		assert_eq!(comic_id("/comic/12a.html"), None);
		assert_eq!(comic_id("/sort?tag=15"), None);
		assert_eq!(
			chapter_path("https://www.wmanhua.com/chapter/5342-1035448.html"),
			Some((String::from("5342"), String::from("1035448")))
		);
		assert_eq!(chapter_path("/chapter/5342.html"), None);
		assert_eq!(chapter_path("/comic/5342.html"), None);
	}

	#[aidoku_test]
	fn reads_cover_styles() {
		assert_eq!(
			style_url("background-image:url('https://image4.wmanhua.com/mh/a/cover.webp')")
				.as_deref(),
			Some("https://image4.wmanhua.com/mh/a/cover.webp")
		);
		assert_eq!(style_url("color:red"), None);
		assert_eq!(style_url("background-image:url('/x.png')"), None);
	}

	#[aidoku_test]
	fn reads_chapter_numbers() {
		assert_eq!(chapter_number("第158话"), Some(158.0));
		assert_eq!(chapter_number("第8.2話"), Some(8.2));
		assert_eq!(chapter_number("72话 大结局：婚礼"), Some(72.0));
		assert_eq!(chapter_number("780 大只佬"), Some(780.0));
		assert_eq!(chapter_number("176.拉扯"), Some(176.0));
		assert_eq!(chapter_number("第27卷"), None);
		assert_eq!(volume_number("第27卷"), Some(27.0));
		assert_eq!(volume_number("第158话"), None);
		assert_eq!(numbers("第2季 第5话"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("第二季"), (None, Some(2.0)));
		assert_eq!(numbers("第27卷"), (None, Some(27.0)));
		assert_eq!(numbers("第3卷 第5话"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("第3卷 后记"), (None, None));
		assert_eq!(chapter_number("第十二话"), Some(12.0));
		assert_eq!(numbers("第十二话"), (Some(12.0), None));
		assert_eq!(numbers("第158话"), (Some(158.0), None));
		assert_eq!(volume_number("01卷後記"), None);
		assert_eq!(chapter_number("01卷後記"), None);
		assert_eq!(numbers("06卷番外"), (None, None));
		assert_eq!(numbers("3卷"), (None, Some(3.0)));
		assert_eq!(numbers("三卷"), (None, Some(3.0)));
		// The leading `第` of a volume may be left out.
		assert_eq!(numbers("3卷 第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("3卷第5話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("三卷 第五話"), (Some(5.0), Some(3.0)));
		assert_eq!(numbers("2季 第5話"), (Some(5.0), Some(2.0)));
		assert_eq!(numbers("3卷 後記"), (None, None));
		assert_eq!(numbers("3卷卷首"), (None, None));
		assert_eq!(numbers("一部分"), (None, None));
		assert_eq!(numbers("三部曲 第2話"), (None, None));
		assert_eq!(numbers("卷三"), (None, None));
		assert_eq!(numbers("卷 3"), (None, None));
		assert_eq!(volume_number("新年番外"), None);
		assert_eq!(chapter_number("新年番外"), None);
		assert_eq!(chapter_number("第二季"), None);
		assert_eq!(chapter_number("第2季"), None);
	}

	#[aidoku_test]
	fn rates_by_tags_and_reads_by_type() {
		for tag in SUGGESTIVE_TAGS {
			assert!(crate::tags::tag_id(tag).is_some(), "{tag}");
		}
		assert_eq!(content_rating::<&str>(&[]), ContentRating::Safe);
		assert_eq!(content_rating(&["搞笑", "冒险"]), ContentRating::Safe);
		assert_eq!(content_rating(&["耽美", "宫廷"]), ContentRating::Suggestive);
		assert_eq!(content_rating(&["後宮"]), ContentRating::Suggestive);
		assert_eq!(viewer("日本漫画"), Viewer::RightToLeft);
		assert_eq!(viewer("港台漫画"), Viewer::RightToLeft);
		assert_eq!(viewer("韩国漫画"), Viewer::Webtoon);
		assert_eq!(viewer("大陆漫画"), Viewer::Webtoon);
		assert_eq!(viewer(""), Viewer::Unknown);
	}

	#[aidoku_test]
	fn reads_chapter_json() {
		let json = r#"{"code":0,"message":"","data":{"chapters":[{"id":1,"contentId":5,"chapterName":"第01话","chapterNum":12,"chapterPath":"a","createdAt":"2026-05-05 07:02:57","updatedAt":"2026-09-18 02:28:30"},{"id":2,"contentId":5,"chapterName":"新年\"番外\"","chapterNum":9,"chapterPath":"b","createdAt":"2026-05-06 07:02:57","updatedAt":"x"}]}}"#;
		let chapters = parse_chapters(json, "5").expect("list").expect("chapters");
		assert_eq!(chapters.len(), 2);
		assert_eq!(chapters[0].key, "2");
		assert_eq!(chapters[0].title.as_deref(), Some("新年\"番外\""));
		// No number in the title, and the other title has one, so it stays None.
		assert_eq!(chapters[0].chapter_number, None);
		assert_eq!(chapters[1].key, "1");
		assert_eq!(chapters[1].chapter_number, Some(1.0));
		assert_eq!(chapters[1].date_uploaded, Some(1777964577));
		assert_eq!(
			chapters[1].url.as_deref(),
			Some("https://www.wmanhua.com/chapter/5-1.html")
		);
		assert!(matches!(
			parse_chapters(r#"{"code":0,"data":{"chapters":null}}"#, "5"),
			Ok(None)
		));
		assert!(matches!(
			parse_chapters(r#"{"code":0,"data":{"chapters": []}}"#, "5"),
			Ok(None)
		));
		// A chapter without an id rejects the whole list.
		assert!(parse_chapters(
			r#"{"data":{"chapters":[{"id":1,"chapterName":"a"},{"chapterName":"b"}]}}"#,
			"5"
		)
		.is_err());
		// Cut off before the array closes, or not a chapter list at all.
		assert!(parse_chapters(r#"{"data":{"chapters":[{"id":1,"chapterName":"a"}"#, "5").is_err());
		assert!(parse_chapters("<html>502</html>", "5").is_err());
	}

	#[aidoku_test]
	fn builds_page_addresses() {
		let page = "var totalPages = parseInt('3');\nvar imgBaseUrl = 'https://image4.wmanhua.com/mh/h/' + ('p-1' || '第1话') + '/';";
		let pages = parse_pages(page).expect("pages");
		assert_eq!(pages.len(), 3);
		assert_eq!(
			pages[2].content,
			PageContent::url("https://image4.wmanhua.com/mh/h/p-1/3.webp")
		);
		// An empty chapter path falls back to the chapter's name, as the page's `||` does.
		let page = "var totalPages = parseInt('1');\nvar imgBaseUrl = 'https://image4.wmanhua.com/mh/h/' + ('' || '第1话') + '/';";
		assert_eq!(
			parse_pages(page).expect("pages")[0].content,
			PageContent::url("https://image4.wmanhua.com/mh/h/%E7%AC%AC1%E8%AF%9D/1.webp")
		);
		assert!(parse_pages(
			"var totalPages = parseInt('0');\nvar imgBaseUrl = 'https://x/' + ('p' || 'n') + '/';"
		)
		.is_err());
		assert!(parse_pages("var totalPages = parseInt('2');").is_err());
		assert!(parse_pages("<html></html>").is_err());
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/helper.rs"]
mod fixture_tests;
