//! Typed App responses. Business failures must never turn into empty chapter updates.
use crate::helper::{absolute_url, episode_no};
use aidoku::{
	alloc::{String, Vec},
	prelude::*,
	Chapter, HashMap, Page, PageContent, PageContext, Result, Viewer,
};
use serde::{de::DeserializeOwned, Deserialize};

pub const IMAGE_HOST: &str = "https://webtoon-phinf.pstatic.net";
pub const PRODUCT_HOST: &str = "https://prowebtoon-phinf.pstatic.net";
// Actual absolute image host returned for purchased WEBTOON episodes on device.
pub const STATIC_HOST: &str = "https://webtoons-static.pstatic.net";

#[derive(Deserialize)]
struct Envelope<T> {
	message: Message<T>,
}
#[derive(Deserialize)]
struct Message<T> {
	code: Option<i32>,
	result: Option<T>,
}

pub fn decode<T: DeserializeOwned>(body: &str) -> Result<T> {
	let response: Envelope<T> =
		serde_json::from_str(body).map_err(|_| error!("無法解析 WEBTOON App 回應"))?;
	if response.message.code.is_some_and(|code| code != 0) {
		return Err(error!(
			"WEBTOON App 請求失敗，代碼 {:?}",
			response.message.code
		));
	}
	response
		.message
		.result
		.ok_or_else(|| error!("WEBTOON App 未回傳資料"))
}

pub fn response_code(body: &str) -> Option<i64> {
	let value: serde_json::Value = serde_json::from_str(body).ok()?;
	value.get("message")?.get("code")?.as_i64()
}

#[derive(Deserialize)]
pub struct MemberInfo {
	pub id: String,
	pub nickname: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RightList {
	pub right_list: Vec<ProductRight>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductRight {
	episode_no: i32,
	product_id: String,
	has_right: bool,
	#[serde(default)]
	infinite: bool,
	#[serde(default)]
	expire_time: i64,
}

pub fn apply_rights(chapters: &mut [Chapter], title: &str, rights: &[ProductRight], now_ms: i64) {
	for chapter in chapters {
		let Some(episode) = episode_no(&chapter.key) else {
			continue;
		};
		let expected = format!("linewebtoon-WEBTOON-{title}-{episode}");
		if !rights.iter().any(|right| {
			right.episode_no == episode
				&& right.product_id == expected
				&& right.has_right
				&& (right.infinite || right.expire_time > now_ms)
		}) {
			continue;
		}
		if let Some(name) = chapter.title.as_ref() {
			let base = [
				"（需官方 App 購買）",
				"（需官方 App 解鎖）",
				"（需官方 App 限時解鎖）",
			]
			.into_iter()
			.find_map(|suffix| name.strip_suffix(suffix))
			.unwrap_or(name);
			chapter.title = Some(String::from(base));
		}
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeList {
	pub episode_list: Vec<Episode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
	pub episode_no: i32,
	pub episode_title: String,
	episode_thumbnail: Option<String>,
	exposure_ymdt: Option<i64>,
	product_id: Option<String>,
	basic_product_sale_unit: Option<serde_json::Value>,
	#[serde(default)]
	reward_ad_episode: bool,
	#[serde(default)]
	time_deal_episode: bool,
}

pub fn chapters_from_episodes(
	episodes: Vec<Episode>,
	legacy: &[Chapter],
	manga: bool,
) -> Result<Vec<Chapter>> {
	let mut keys = HashMap::new();
	let mut urls = HashMap::new();
	// The caller supplies the current web list before stored chapters. Keep
	// stored keys for progress, but prefer the first available (current) URL.
	for chapter in legacy {
		let Some(no) = episode_no(&chapter.key) else {
			continue;
		};
		keys.insert(no, chapter);
		if let Some(url) = &chapter.url {
			urls.entry(no).or_insert_with(|| url.clone());
		}
	}
	episodes
		.into_iter()
		.map(|episode| {
			if episode.episode_no <= 0 {
				return Err(error!("WEBTOON 回傳無效章節編號"));
			}
			let old = keys.get(&episode.episode_no);
			let requirement = if episode.time_deal_episode {
				Some("需官方 App 限時解鎖")
			} else if episode.reward_ad_episode {
				Some("需官方 App 解鎖")
			} else if episode.product_id.is_some() || episode.basic_product_sale_unit.is_some() {
				Some("需官方 App 購買")
			} else {
				None
			};
			let title = match requirement {
				Some(reason) => format!("{}（{reason}）", episode.episode_title),
				None => episode.episode_title,
			};
			Ok(Chapter {
				// Existing public keys remain byte-for-byte unchanged. New App-only keys
				// use the episode ID; unlike a viewer slug, it does not change with the title.
				key: old
					.map(|c| c.key.clone())
					.unwrap_or_else(|| format!("{}", episode.episode_no)),
				url: urls.get(&episode.episode_no).cloned(),
				title: Some(title),
				chapter_number: (!manga).then_some(episode.episode_no as f32),
				date_uploaded: episode.exposure_ymdt.map(|ms| ms / 1000),
				thumbnail: episode
					.episode_thumbnail
					.map(|path| absolute_url(IMAGE_HOST, &path)),
				// Purchases can change after a refresh. Always recheck at reader entry.
				locked: false,
				..Default::default()
			})
		})
		.collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleInfoResult {
	pub title_info: TitleInfo,
}

/// App-only numeric keys must survive a later public web release or title-slug change.
pub fn preserve_keys(chapters: &mut [Chapter], saved: &HashMap<String, String>) {
	for chapter in chapters {
		if let Some(key) = episode_no(&chapter.key).and_then(|no| saved.get(&format!("{no}"))) {
			chapter.key.clone_from(key);
		}
	}
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleInfo {
	pub viewer: String,
	pub first_episode_no: i32,
	pub total_episode_count: usize,
}
impl TitleInfo {
	pub fn is_manga(&self) -> bool {
		self.viewer == "MANGA"
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeInfoResult {
	pub episode_info: EpisodeInfo,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeInfo {
	pub page_progression_direction: Option<String>,
	#[serde(default)]
	product: bool,
	image_info: Vec<ImageInfo>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageInfo {
	sort_order: i32,
	url: String,
}

pub fn trusted_image_url(url: &str) -> bool {
	[IMAGE_HOST, PRODUCT_HOST, STATIC_HOST].iter().any(|host| {
		url.strip_prefix(host)
			.is_some_and(|path| path.starts_with('/'))
	})
}

impl EpisodeInfo {
	pub fn requires_secure_token(&self, manga: bool) -> bool {
		self.product || manga
	}

	pub fn viewer(&self) -> Viewer {
		match self.page_progression_direction.as_deref() {
			Some("RTL") => Viewer::RightToLeft,
			Some("LTR") => Viewer::LeftToRight,
			_ => Viewer::Webtoon,
		}
	}

	pub fn into_pages(mut self, host: &str, cookie: Option<&str>) -> Result<Vec<Page>> {
		if !matches!(host, IMAGE_HOST | PRODUCT_HOST) {
			return Err(error!("無效的 WEBTOON 圖片主機"));
		}
		if self.image_info.is_empty() {
			return Err(error!("WEBTOON 未回傳可閱讀的圖片"));
		}
		self.image_info.sort_by_key(|image| image.sort_order);
		self.image_info
			.into_iter()
			.map(|image| {
				// Android preserves absolute URLs, using productDomain only for relative
				// paths. Paid WEBTOON images may already point at the regular image CDN.
				let url = if let Some(rest) = image.url.strip_prefix("http://") {
					format!("https://{rest}")
				} else if image.url.starts_with("//") {
					format!("https:{}", image.url)
				} else {
					absolute_url(host, &image.url)
				};
				if !trusted_image_url(&url) {
					return Err(error!("WEBTOON 圖片主機與預期不符"));
				}
				let mut context = PageContext::new();
				if let Some(cookie) = cookie {
					context.insert("Cookie".into(), cookie.into());
				}
				Ok(Page {
					content: PageContent::url_context(url, context),
					..Default::default()
				})
			})
			.collect()
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfoResult {
	pub service_info: ServiceInfo,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
	pub product_image_domain: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureTokenResult {
	pub secure_token: SecureToken,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureToken {
	cookie_name: String,
	cookie_value: String,
}
impl SecureToken {
	pub fn cookie(&self) -> Result<String> {
		if self.cookie_name.is_empty()
			|| self.cookie_value.is_empty()
			|| self.cookie_name.contains(['\r', '\n', ';', '='])
			|| self.cookie_value.contains(['\r', '\n', ';'])
		{
			return Err(error!("無效的 WEBTOON 圖片 token"));
		}
		Ok(format!("{}={}", self.cookie_name, self.cookie_value))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku::{alloc::vec, Chapter};
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn app_only_keys_stay_stable_when_an_episode_later_becomes_free_on_web() {
		let web_url =
			"https://www.webtoons.com/zh-hant/x/y/chapter/viewer?title_no=3123&episode_no=271";
		let mut chapters = vec![Chapter {
			key: web_url.into(),
			url: Some(web_url.into()),
			..Default::default()
		}];
		let saved = HashMap::from([(String::from("271"), String::from("271"))]);
		preserve_keys(&mut chapters, &saved);
		assert_eq!(chapters[0].key, "271");
		assert_eq!(chapters[0].url.as_deref(), Some(web_url));
	}

	#[aidoku_test]
	fn refreshed_public_url_survives_stored_numeric_key_and_reads_without_app_api() {
		let web_url = "https://www.webtoons.com/zh-hant/x/y/viewer?title_no=3123&episode_no=271";
		for previous_url in [
			None,
			Some("https://www.webtoons.com/zh-hant/old/slug/viewer?title_no=3123&episode_no=271"),
		] {
			let mut legacy = crate::helper::parse_episodes_json(r#"{"success":true,"result":{"episodeList":[{"episodeNo":271,"viewerLink":"/zh-hant/x/y/viewer?title_no=3123&episode_no=271"}]}}"#).unwrap();
			legacy.push(Chapter {
				key: "271".into(),
				url: previous_url.map(String::from),
				..Default::default()
			});
			let page: EpisodeList = decode(
				r#"{"message":{"result":{"episodeList":[{"episodeNo":271,"episodeTitle":"Free"}]}}}"#,
			)
			.unwrap();
			let mut chapters = chapters_from_episodes(page.episode_list, &legacy, false).unwrap();
			preserve_keys(
				&mut chapters,
				&HashMap::from([("271".into(), "271".into())]),
			);
			assert_eq!(chapters[0].key, "271");
			assert_eq!(chapters[0].url.as_deref(), Some(web_url));
			let manga = aidoku::Manga {
				viewer: Viewer::Webtoon,
				..Default::default()
			};
			let pages = crate::read_chapter(
				&manga,
				&chapters[0],
				false,
				|| panic!("public free chapter must not call titleInfo"),
				|_, _| panic!("public free chapter must not use App reader"),
				|url| {
					assert_eq!(url, web_url);
					Ok(vec![Page::default()])
				},
			)
			.unwrap();
			assert_eq!(pages.len(), 1);
		}
	}

	#[aidoku_test]
	fn stored_url_is_retained_when_no_new_public_url_exists() {
		let old_url =
			"https://www.webtoons.com/zh-hant/old/slug/viewer?title_no=3123&episode_no=271";
		for previous_url in [None, Some(old_url)] {
			let legacy = vec![Chapter {
				key: "271".into(),
				url: previous_url.map(String::from),
				..Default::default()
			}];
			let page: EpisodeList = decode(
				r#"{"message":{"result":{"episodeList":[{"episodeNo":271,"episodeTitle":"Episode"}]}}}"#,
			)
			.unwrap();
			let chapters = chapters_from_episodes(page.episode_list, &legacy, false).unwrap();
			assert_eq!(chapters[0].key, "271");
			assert_eq!(chapters[0].url.as_deref(), previous_url);
		}
	}

	#[aidoku_test]
	fn paid_webtoon_images_require_secure_tokens_too() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"product":true,"imageInfo":[{"sortOrder":1,"url":"/a.jpg"}]}}}}"#).unwrap();
		assert!(info.episode_info.requires_secure_token(false));
	}

	#[aidoku_test]
	fn business_errors_and_missing_episode_lists_fail_instead_of_erasing_chapters() {
		assert!(
			decode::<EpisodeList>(r#"{"message":{"code":2003,"message":"not readable"}}"#).is_err()
		);
		assert!(decode::<EpisodeList>(r#"{"message":{"result":{}}}"#).is_err());
		assert!(decode::<EpisodeList>("not json").is_err());
	}

	#[aidoku_test]
	fn viewer_sorts_images_and_obeys_page_direction() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"pageProgressionDirection":"RTL","imageInfo":[{"sortOrder":2,"url":"/b.jpg"},{"sortOrder":1,"url":"/a.jpg"}]}}}}"#).unwrap();
		assert_eq!(info.episode_info.viewer(), aidoku::Viewer::RightToLeft);
		let pages = info
			.episode_info
			.into_pages("https://prowebtoon-phinf.pstatic.net", Some("token=value"))
			.unwrap();
		match &pages[0].content {
			aidoku::PageContent::Url(url, context) => {
				assert_eq!(url, "https://prowebtoon-phinf.pstatic.net/a.jpg");
				assert_eq!(
					context.as_ref().unwrap().get("Cookie").map(|s| s.as_str()),
					Some("token=value")
				);
			}
			_ => panic!("expected image"),
		}
	}

	#[aidoku_test]
	fn paid_static_image_url_observed_on_device_keeps_url_and_secure_cookie() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"product":true,"imageInfo":[{"sortOrder":1,"url":"https://webtoons-static.pstatic.net/paid/page.jpg?type=q90"}]}}}}"#).unwrap();
		let pages = info
			.episode_info
			.into_pages(PRODUCT_HOST, Some("token=value"))
			.unwrap();
		let PageContent::Url(url, Some(context)) = &pages[0].content else {
			panic!("expected secure image")
		};
		assert_eq!(
			url,
			"https://webtoons-static.pstatic.net/paid/page.jpg?type=q90"
		);
		assert_eq!(
			context.get("Cookie").map(String::as_str),
			Some("token=value")
		);
		assert!(!trusted_image_url(
			"https://webtoons-static.pstatic.net.evil.test/paid/page.jpg"
		));
		assert!(!trusted_image_url(
			"https://webtoons-static.pstatic.net@evil.test/paid/page.jpg"
		));
	}

	#[aidoku_test]
	fn paid_images_preserve_absolute_official_urls_and_their_secure_cookie() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"product":true,"imageInfo":[{"sortOrder":1,"url":"https://webtoon-phinf.pstatic.net/paid/page.jpg?type=q90"}]}}}}"#).unwrap();
		let pages = info
			.episode_info
			.into_pages(PRODUCT_HOST, Some("token=value"))
			.unwrap();
		let PageContent::Url(url, Some(context)) = &pages[0].content else {
			panic!("expected secure image")
		};
		assert_eq!(
			url,
			"https://webtoon-phinf.pstatic.net/paid/page.jpg?type=q90"
		);
		assert_eq!(
			context.get("Cookie").map(String::as_str),
			Some("token=value")
		);
	}

	#[aidoku_test]
	fn official_http_and_protocol_relative_image_urls_use_https() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"product":true,"imageInfo":[{"sortOrder":1,"url":"http://webtoon-phinf.pstatic.net/a.jpg"},{"sortOrder":2,"url":"//prowebtoon-phinf.pstatic.net/b.jpg"}]}}}}"#).unwrap();
		let pages = info
			.episode_info
			.into_pages(PRODUCT_HOST, Some("token=value"))
			.unwrap();
		let PageContent::Url(first, _) = &pages[0].content else {
			panic!("expected image")
		};
		let PageContent::Url(second, _) = &pages[1].content else {
			panic!("expected image")
		};
		assert_eq!(first, "https://webtoon-phinf.pstatic.net/a.jpg");
		assert_eq!(second, "https://prowebtoon-phinf.pstatic.net/b.jpg");
	}

	#[aidoku_test]
	fn secure_cookies_never_follow_foreign_image_urls() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"imageInfo":[{"sortOrder":1,"url":"https://example.com/a.jpg"}]}}}}"#).unwrap();
		assert!(info
			.episode_info
			.into_pages("https://prowebtoon-phinf.pstatic.net", Some("token=value"))
			.is_err());
		let info: EpisodeInfoResult =
			decode(r#"{"message":{"result":{"episodeInfo":{"imageInfo":[]}}}}"#).unwrap();
		assert!(info
			.episode_info
			.into_pages("https://prowebtoon-phinf.pstatic.net", None)
			.is_err());
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[path = "fixture_tests/models.rs"]
mod fixture_tests;
