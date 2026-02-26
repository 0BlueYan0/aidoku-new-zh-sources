use aidoku::{
	alloc::{String, Vec},
	imports::html::Element,
	Chapter, Manga, Viewer,
};

/// Map genre name (Chinese) to the Webtoons URL slug.
pub fn genre_name_to_slug(name: &str) -> &'static str {
	match name {
		"愛情" => "romance",
		"歐式宮廷" => "western_palace",
		"影視化" => "adaptation",
		"校園" => "school",
		"台灣原創作品" => "local",
		"奇幻冒險" => "fantasy",
		"驚悚" => "thriller",
		"恐怖" => "horror",
		"武俠" => "martial_arts",
		"LGBTQ+" => "bl_gl",
		"大人系" => "romance_m",
		"劇情" => "drama",
		"動作" => "action",
		"生活/日常" => "slice_of_life",
		"搞笑" => "comedy",
		"穿越/轉生" => "time_slip",
		"現代/職場" => "city_office",
		"懸疑推理" => "mystery",
		"療癒/萌系" => "heartwarming",
		"少年" => "shonen",
		"古代宮廷" => "eastern_palace",
		"小說" => "web_novel",
		_ => "romance",
	}
}

/// Extract `title_no` from a Webtoons URL.
pub fn extract_title_no(url: &str) -> Option<String> {
	let pos = url.find("title_no=")?;
	let start = pos + 9;
	let rest = &url[start..];
	let end = rest.find('&').unwrap_or(rest.len());
	Some(String::from(&rest[..end]))
}

/// Parse a manga item from listing/genre/search pages.
///
/// Expected HTML structure:
/// ```html
/// <a href="...?title_no=2089" class="link" data-title-no="2089">
///   <div class="image_wrap"><img src="..." /></div>
///   <div class="info_text">
///     <div class="genre">奇幻冒險</div>
///     <strong class="title">全知讀者視角</strong>
///     <div class="author">Author Name</div>
///   </div>
/// </a>
/// ```
pub fn parse_manga_item(item: &Element) -> Option<Manga> {
	let href = item.attr("href")?;
	let title_no = item
		.attr("data-title-no")
		.or_else(|| extract_title_no(&href))?;

	// Title: strong.title
	let title = item
		.select_first("strong.title")
		.and_then(|el: Element| el.text())
		.unwrap_or_default();

	if title.is_empty() {
		return None;
	}

	// Cover image: div.image_wrap img
	let cover = item
		.select_first(".image_wrap img")
		.and_then(|el: Element| el.attr("src"));

	// Author: div.author (may not be present on originals pages)
	let mut manga = Manga {
		key: title_no,
		title,
		cover,
		url: Some(href),
		viewer: Viewer::Webtoon,
		..Default::default()
	};

	if let Some(author_el) = item.select_first(".author") {
		if let Some(author_text) = author_el.text() {
			let authors: Vec<String> = author_text
				.split('/')
				.map(|s: &str| String::from(s.trim()))
				.filter(|s: &String| !s.is_empty())
				.collect();
			if !authors.is_empty() {
				manga.authors = Some(authors);
			}
		}
	}

	// Genre tag (shown on originals pages where author spot has genre)
	if let Some(genre_el) = item.select_first(".genre") {
		if let Some(genre_text) = genre_el.text() {
			manga.tags = Some(aidoku::alloc::vec![genre_text]);
		}
	}

	Some(manga)
}



// --- Mobile API JSON parsing ---

const BASE_URL_HELPER: &str = "https://www.webtoons.com";
const THUMB_CDN_HELPER: &str = "https://webtoon-phinf.pstatic.net";

/// Extract a JSON string value for a given key from a JSON object substring.
/// Looks for `"key":"value"` and returns the value.
fn json_str_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
	let search = aidoku::alloc::format!("\"{}\":\"", key);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];
	let end = rest.find('"')?;
	Some(&rest[..end])
}

/// Extract a JSON number value for a given key from a JSON object substring.
fn json_num_value(json: &str, key: &str) -> Option<i64> {
	let search = aidoku::alloc::format!("\"{}\":", key);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];

	let mut num_str = String::new();
	for ch in rest.chars() {
		if ch.is_ascii_digit() || ch == '-' {
			num_str.push(ch);
		} else if !num_str.is_empty() {
			break;
		}
	}

	num_str.parse::<i64>().ok()
}

/// Parse the Webtoons mobile API JSON response into a list of Chapter objects.
/// The JSON format is:
/// ```json
/// {"result":{"episodeList":[{"episodeNo":1,"episodeTitle":"...","viewerLink":"...","thumbnail":"...","exposureDateMillis":123456},...]},"success":true}
/// ```
pub fn parse_episodes_json(body: &str) -> Vec<Chapter> {
	let mut chapters: Vec<Chapter> = Vec::new();

	// Find the episodeList array
	let list_start = match body.find("\"episodeList\":[") {
		Some(pos) => pos + 14, // skip past "episodeList":[
		None => return chapters,
	};

	let body_from_list = &body[list_start..];

	// Split by each episode object: find each {...} block
	let mut depth = 0;
	let mut obj_start: Option<usize> = None;

	for (i, ch) in body_from_list.char_indices() {
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
					if let Some(start) = obj_start {
						let obj_str = &body_from_list[start..=i];
						if let Some(chapter) = parse_single_episode(obj_str) {
							chapters.push(chapter);
						}
					}
					obj_start = None;
				}
			}
			']' if depth == 0 => break,
			_ => {}
		}
	}

	// API returns oldest first; reverse to show newest first
	chapters.reverse();
	chapters
}

fn parse_single_episode(obj: &str) -> Option<Chapter> {
	let episode_no = json_num_value(obj, "episodeNo")? as i32;
	let title = json_str_value(obj, "episodeTitle")
		.map(|s: &str| String::from(s));
	let viewer_link = json_str_value(obj, "viewerLink");
	let thumb_path = json_str_value(obj, "thumbnail");
	let date_millis = json_num_value(obj, "exposureDateMillis");

	// Unescape URL-encoded paths (the JSON has already-encoded URLs)
	let viewer_url = viewer_link.map(|link: &str| {
		aidoku::alloc::format!("{BASE_URL_HELPER}{link}")
	});

	let thumbnail = thumb_path.map(|path: &str| {
		aidoku::alloc::format!("{THUMB_CDN_HELPER}{path}")
	});

	let date_uploaded = date_millis.map(|ms: i64| ms / 1000);

	let key = viewer_url
		.clone()
		.unwrap_or_else(|| aidoku::alloc::format!("{episode_no}"));

	Some(Chapter {
		key,
		title,
		chapter_number: Some(episode_no as f32),
		date_uploaded,
		url: viewer_url,
		thumbnail,
		..Default::default()
	})
}
