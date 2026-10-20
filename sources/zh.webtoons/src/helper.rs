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

// --- Mobile web API: used only to preserve already-published chapter keys. ---

#[derive(serde::Deserialize)]
struct MobileEnvelope {
	success: bool,
	result: MobileResult,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileResult {
	episode_list: Vec<MobileEpisode>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileEpisode {
	episode_no: i32,
	episode_title: Option<String>,
	viewer_link: Option<String>,
	thumbnail: Option<String>,
	exposure_date_millis: Option<i64>,
}

pub fn parse_episodes_json(body: &str) -> aidoku::Result<Vec<Chapter>> {
	let envelope: MobileEnvelope =
		serde_json::from_str(body).map_err(|_| aidoku::error!("無法解析網頁章節清單"))?;
	if !envelope.success {
		return Err(aidoku::error!("網頁章節清單請求失敗"));
	}
	let mut chapters: Vec<Chapter> = envelope
		.result
		.episode_list
		.into_iter()
		.map(|episode| {
			let url = episode
				.viewer_link
				.map(|link| absolute_url("https://www.webtoons.com", &link));
			Chapter {
				key: url
					.clone()
					.unwrap_or_else(|| aidoku::alloc::format!("{}", episode.episode_no)),
				title: episode.episode_title,
				chapter_number: Some(episode.episode_no as f32),
				date_uploaded: episode.exposure_date_millis.map(|ms| ms / 1000),
				url,
				thumbnail: episode
					.thumbnail
					.map(|path| absolute_url("https://webtoon-phinf.pstatic.net", &path)),
				..Default::default()
			}
		})
		.collect();
	chapters.reverse();
	Ok(chapters)
}

pub fn absolute_url(base: &str, path: &str) -> String {
	if path.starts_with("https://") || path.starts_with("http://") {
		String::from(path)
	} else {
		aidoku::alloc::format!(
			"{}/{}",
			base.trim_end_matches('/'),
			path.trim_start_matches('/')
		)
	}
}

/// Existing chapter keys can be viewer URLs or bare episode IDs.
pub fn episode_no(key: &str) -> Option<i32> {
	let raw = key
		.split_once("episode_no=")
		.map(|(_, tail)| tail.split(['&', '#']).next().unwrap_or_default())
		.unwrap_or(key);
	raw.parse::<i32>().ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn mobile_titles_preserve_quotes_and_braces() {
		let chapters = parse_episodes_json(r#"{"result":{"episodeList":[{"episodeNo":1,"episodeTitle":"試閱 \"{咒術}\"","viewerLink":"/zh-hant/action/jjk/trial/viewer?title_no=10568&episode_no=1"}]},"success":true}"#).unwrap();
		assert_eq!(chapters[0].title.as_deref(), Some("試閱 \"{咒術}\""));
	}

	#[aidoku_test]
	fn mobile_absolute_links_keep_the_existing_chapter_key() {
		let chapters = parse_episodes_json(r#"{"result":{"episodeList":[{"episodeNo":1,"viewerLink":"https://www.webtoons.com/zh-hant/viewer?title_no=10568&episode_no=1"}]},"success":true}"#).unwrap();
		assert_eq!(
			chapters[0].key,
			"https://www.webtoons.com/zh-hant/viewer?title_no=10568&episode_no=1"
		);
	}
}
