//! Typed App responses. Business failures must never turn into empty chapter updates.
use aidoku::{
	alloc::{String, Vec},
	prelude::*,
	Chapter, HashMap, Page, PageContent, PageContext, Result, Viewer,
};
use serde::{de::DeserializeOwned, Deserialize};
use crate::helper::{absolute_url, episode_no};

pub const IMAGE_HOST: &str = "https://webtoon-phinf.pstatic.net";
pub const PRODUCT_HOST: &str = "https://prowebtoon-phinf.pstatic.net";

#[derive(Deserialize)]
struct Envelope<T> { message: Message<T> }
#[derive(Deserialize)]
struct Message<T> {
	code: Option<i32>,
	result: Option<T>,
}

pub fn decode<T: DeserializeOwned>(body: &str) -> Result<T> {
	let response: Envelope<T> = serde_json::from_str(body)
		.map_err(|_| error!("無法解析 WEBTOON App 回應"))?;
	if response.message.code.is_some_and(|code| code != 0) {
		return Err(error!("WEBTOON App 請求失敗，代碼 {:?}", response.message.code));
	}
	response.message.result.ok_or_else(|| error!("WEBTOON App 未回傳資料"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeList { pub episode_list: Vec<Episode> }

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
	episodes: Vec<Episode>, legacy: &[Chapter], manga: bool,
) -> Result<Vec<Chapter>> {
	let keys: HashMap<i32, &Chapter> = legacy.iter()
		.filter_map(|chapter| episode_no(&chapter.key).map(|no| (no, chapter))).collect();
	episodes.into_iter().map(|episode| {
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
			key: old.map(|c| c.key.clone()).unwrap_or_else(|| format!("{}", episode.episode_no)),
			url: old.and_then(|c| c.url.clone()),
			title: Some(title),
			chapter_number: (!manga).then_some(episode.episode_no as f32),
			date_uploaded: episode.exposure_ymdt.map(|ms| ms / 1000),
			thumbnail: episode.episode_thumbnail.map(|path| absolute_url(IMAGE_HOST, &path)),
			// This anonymous-only implementation cannot obtain purchase/ad rights.
			locked: requirement.is_some(),
			..Default::default()
		})
	}).collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleInfoResult { pub title_info: TitleInfo }

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
	pub fn is_manga(&self) -> bool { self.viewer == "MANGA" }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeInfoResult { pub episode_info: EpisodeInfo }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeInfo {
	pub page_progression_direction: Option<String>,
	image_info: Vec<ImageInfo>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageInfo { sort_order: i32, url: String }

impl EpisodeInfo {
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
		self.image_info.into_iter().map(|image| {
			let url = absolute_url(host, &image.url);
			if !url.starts_with(&format!("{host}/")) {
				return Err(error!("WEBTOON 圖片主機與預期不符"));
			}
			let mut context = PageContext::new();
			if host == PRODUCT_HOST {
				if let Some(cookie) = cookie { context.insert("Cookie".into(), cookie.into()); }
			}
			Ok(Page {
				content: PageContent::url_context(url, context),
				..Default::default()
			})
		}).collect()
	}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfoResult { pub service_info: ServiceInfo }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo { pub product_image_domain: String }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureTokenResult { pub secure_token: SecureToken }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureToken { cookie_name: String, cookie_value: String }
impl SecureToken {
	pub fn cookie(&self) -> Result<String> {
		if self.cookie_name.is_empty() || self.cookie_value.is_empty()
			|| self.cookie_name.contains(['\r', '\n', ';', '='])
			|| self.cookie_value.contains(['\r', '\n', ';']) {
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
	fn app_list_preserves_legacy_keys_and_manga_titles() {
		let page: EpisodeList = decode(include_str!("fixtures/app-10568-0.json")).unwrap();
		let old_key = "https://www.webtoons.com/zh-hant/action/jjk/trial/viewer?title_no=10568&episode_no=1";
		let legacy = vec![Chapter { key: old_key.into(), chapter_number: Some(1.0), ..Default::default() }];
		let trial: EpisodeList = decode(include_str!("fixtures/app-10568-30.json")).unwrap();
		let chapters = chapters_from_episodes(trial.episode_list, &legacy, true).unwrap();
		assert_eq!(chapters[0].key, old_key);
		assert_eq!(chapters[0].title.as_deref(), Some("試閱"));
		assert!(!chapters[0].locked);
		let paid = chapters_from_episodes(page.episode_list, &legacy, true).unwrap();
		assert_eq!(paid[0].key, "31");
		assert_eq!(paid[0].title.as_deref(), Some("第30集（需官方 App 購買）"));
		assert_eq!(paid[0].chapter_number, None);
		assert!(paid[0].locked);
	}

	#[aidoku_test]
	fn app_only_keys_stay_stable_when_an_episode_later_becomes_free_on_web() {
		let web_url = "https://www.webtoons.com/zh-hant/x/y/chapter/viewer?title_no=3123&episode_no=271";
		let mut chapters = vec![Chapter { key: web_url.into(), url: Some(web_url.into()), ..Default::default() }];
		let saved = HashMap::from([(String::from("271"), String::from("271"))]);
		preserve_keys(&mut chapters, &saved);
		assert_eq!(chapters[0].key, "271");
		assert_eq!(chapters[0].url.as_deref(), Some(web_url));
	}

	#[aidoku_test]
	fn business_errors_and_missing_episode_lists_fail_instead_of_erasing_chapters() {
		assert!(decode::<EpisodeList>(r#"{"message":{"code":2003,"message":"not readable"}}"#).is_err());
		assert!(decode::<EpisodeList>(r#"{"message":{"result":{}}}"#).is_err());
		assert!(decode::<EpisodeList>("not json").is_err());
	}

	#[aidoku_test]
	fn preview_and_reward_episodes_are_listed_but_not_free() {
		let page: EpisodeList = decode(include_str!("fixtures/app-3123-0.json")).unwrap();
		assert_eq!(page.episode_list[0].episode_no, 271);
		let chapters = chapters_from_episodes(page.episode_list, &[], false).unwrap();
		assert!(chapters[0].locked);
		assert_eq!(chapters[0].chapter_number, Some(271.0));
	}

	#[aidoku_test]
	fn viewer_sorts_images_and_obeys_page_direction() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"pageProgressionDirection":"RTL","imageInfo":[{"sortOrder":2,"url":"/b.jpg"},{"sortOrder":1,"url":"/a.jpg"}]}}}}"#).unwrap();
		assert_eq!(info.episode_info.viewer(), aidoku::Viewer::RightToLeft);
		let pages = info.episode_info.into_pages("https://prowebtoon-phinf.pstatic.net", Some("token=value")).unwrap();
		match &pages[0].content {
			aidoku::PageContent::Url(url, context) => {
				assert_eq!(url, "https://prowebtoon-phinf.pstatic.net/a.jpg");
				assert_eq!(context.as_ref().unwrap().get("Cookie").map(|s| s.as_str()), Some("token=value"));
			}
			_ => panic!("expected image"),
		}
	}

	#[aidoku_test]
	fn secure_cookies_never_follow_foreign_image_urls() {
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"imageInfo":[{"sortOrder":1,"url":"https://example.com/a.jpg"}]}}}}"#).unwrap();
		assert!(info.episode_info.into_pages("https://prowebtoon-phinf.pstatic.net", Some("token=value")).is_err());
		let info: EpisodeInfoResult = decode(r#"{"message":{"result":{"episodeInfo":{"imageInfo":[]}}}}"#).unwrap();
		assert!(info.episode_info.into_pages("https://prowebtoon-phinf.pstatic.net", None).is_err());
	}
}
