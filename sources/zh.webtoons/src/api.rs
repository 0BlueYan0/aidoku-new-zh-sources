//! App networking and request signatures, as observed in WEBTOON Android 3.9.13.
use aidoku::{
	alloc::{String, Vec},
	helpers::uri::encode_uri_component,
	imports::{defaults::{defaults_get, defaults_get_map, defaults_set, DefaultValue}, net::Request, std::current_date},
	prelude::*,
	Chapter, HashMap, Page, PageContext, Result,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use sha1::{Digest, Sha1};
use serde::de::DeserializeOwned;
use crate::models::*;

const API: &str = "https://global.apis.naver.com/lineWebtoon";
const USER_AGENT: &str = "nApps (Android 13; Pixel 7; linewebtoon; 3.9.13)";
// Public client signing key shipped as string resource apigw_key in Android 3.9.13.
const APP_KEY: &str = "gUtPzJFZch4ZyAGviiyH94P99lQ3pFdRTwpJWDlSGFfwgpr6ses5ALOxWHOIT7R1";
// The V3 endpoint validates pageSize against the App's fixed 30-episode window.
const PAGE_SIZE: usize = 30;
const BATCH_SIZE: usize = 5;

fn sign_url(url: &str, timestamp: i64, key: &str) -> Result<String> {
	let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes())
		.map_err(|_| error!("無法建立 WEBTOON 簽章"))?;
	let prefix: String = url.chars().take(255).collect();
	mac.update(prefix.as_bytes());
	mac.update(format!("{timestamp}").as_bytes());
	let digest = STANDARD.encode(mac.finalize().into_bytes());
	let separator = if url.contains('?') { '&' } else { '?' };
	Ok(format!("{url}{separator}msgpad={timestamp}&md={}", encode_uri_component(digest)))
}

fn anonymous_id() -> String {
	if let Some(id) = defaults_get::<String>("webtoon_anonymous_id") {
		if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) { return id; }
	}
	// A local anonymous identifier, never a real Android ID or an account token.
	let digest = Sha1::digest(format!("aidoku-webtoon-{}", current_date()).as_bytes());
	let id: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
	defaults_set("webtoon_anonymous_id", DefaultValue::String(id.clone()));
	id
}

fn request(path: &str, params: &str) -> Result<Request> {
	let separator = if params.is_empty() { "" } else { "&" };
	let url = format!("{API}/{path}?{params}{separator}platform=APP_ANDROID&language=zh-hant&locale=zh-hant&serviceZone=GLOBAL");
	let signed = sign_url(&url, current_date() * 1000, APP_KEY)?;
	Ok(Request::get(signed)?.timeout(20.0)
		.header("User-Agent", USER_AGENT).header("wtu", anonymous_id().as_str()))
}

fn fetch<T: DeserializeOwned>(path: &str, params: &str) -> Result<T> {
	// Do not log signed URLs, cookie values, or full API bodies.
	let response = request(path, params)?.send()?;
	if response.status_code() != 200 {
		return Err(error!("WEBTOON App HTTP {}", response.status_code()));
	}
	decode(&response.get_string()?)
}

pub fn title_info(title: &str) -> Result<TitleInfo> {
	let title = parse_title(title)?;
	let result: TitleInfoResult = fetch("webtoon/titleInfo.json", &format!("v=1&titleNo={title}"))?;
	Ok(result.title_info)
}

pub fn episode_info(title: &str, episode: i32) -> Result<EpisodeInfo> {
	let title = parse_title(title)?;
	if episode <= 0 { return Err(error!("無效的章節編號")); }
	let result: EpisodeInfoResult = fetch("webtoon/episodeInfo.json",
		&format!("v=4&titleNo={title}&episodeNo={episode}"))?;
	Ok(result.episode_info)
}

fn parse_title(title: &str) -> Result<i32> {
	title.parse::<i32>().ok().filter(|n| *n > 0).ok_or_else(|| error!("無效的作品編號"))
}

pub fn episodes(title: &str, expected: usize) -> Result<Vec<Episode>> {
	let title = parse_title(title)?;
	collect_episode_pages(expected, |offsets| {
		let requests: Result<Vec<Request>> = offsets.iter().map(|offset| {
			request("webtoon/titleHomeEpisodeListV3",
				&format!("v=1&titleNo={title}&offset={offset}&pageSize={PAGE_SIZE}&ordering=LATEST"))
		}).collect();
		Request::send_all(requests?).into_iter().map(|response| {
			let response = response?;
			if response.status_code() != 200 {
				return Err(error!("WEBTOON 章節 HTTP {}", response.status_code()));
			}
			decode(&response.get_string()?)
		}).collect()
	})
}

/// Reject partial/duplicate refreshes before Aidoku can replace the library list.
fn collect_episode_pages(
	expected: usize,
	mut fetch: impl FnMut(&[usize]) -> Result<Vec<EpisodeList>>,
) -> Result<Vec<Episode>> {
	if expected == 0 || expected > 10_000 {
		return Err(error!("WEBTOON 章節總數不合理"));
	}
	let offsets: Vec<usize> = (0..expected).step_by(PAGE_SIZE).collect();
	let mut episodes = Vec::with_capacity(expected);
	let mut seen = HashMap::new();
	for batch in offsets.chunks(BATCH_SIZE) {
		let pages = fetch(batch)?;
		if pages.len() != batch.len() {
			return Err(error!("WEBTOON 未回傳完整分頁"));
		}
		for (offset, page) in batch.iter().zip(pages) {
			let count = PAGE_SIZE.min(expected - offset);
			if page.episode_list.len() != count {
				return Err(error!("WEBTOON 章節清單不完整，請重新整理"));
			}
			for episode in page.episode_list {
				if episode.episode_no <= 0 || seen.insert(episode.episode_no, ()).is_some() {
					return Err(error!("WEBTOON 分頁重複或章節編號無效"));
				}
				episodes.push(episode);
			}
		}
	}
	Ok(episodes)
}

pub fn manga_pages(title: &str, episode: i32) -> Result<Vec<Page>> {
	// Read permission is checked by episodeInfo before requesting an image token.
	let info = episode_info(title, episode)?;
	let service: ServiceInfoResult = fetch("webtoon/serviceInfo.json", "v=1")?;
	if service.service_info.product_image_domain != PRODUCT_HOST {
		return Err(error!("WEBTOON 圖片主機已變更"));
	}
	// Fetch fresh credentials whenever the page list is opened/reloaded; do not
	// persist expiring credentials in global defaults or attach them to covers.
	let token: SecureTokenResult = fetch("webtoon/getImageSecureToken.json", "v=1")?;
	info.into_pages(PRODUCT_HOST, Some(&token.secure_token.cookie()?))
}

pub fn saved_key(title: &str, episode: i32) -> Option<String> {
	defaults_get_map(&format!("webtoon_chapter_keys_{title}"))?.get(&format!("{episode}")).cloned()
}

pub fn remember_keys(title: &str, chapters: &mut [Chapter]) {
	let storage = format!("webtoon_chapter_keys_{title}");
	let mut keys = defaults_get_map(&storage).unwrap_or_default();
	preserve_keys(chapters, &keys);
	for chapter in chapters {
		if let Some(no) = crate::helper::episode_no(&chapter.key) {
			keys.insert(format!("{no}"), chapter.key.clone());
		}
	}
	defaults_set(&storage, DefaultValue::HashMap(keys));
}

fn image_cookie<'a>(url: &str, context: Option<&'a PageContext>) -> Option<&'a str> {
	if !url.starts_with("https://prowebtoon-phinf.pstatic.net/") { return None; }
	context?.get("Cookie").map(String::as_str)
}

pub fn image_request(url: String, context: Option<&PageContext>) -> Result<Request> {
	let mut request = Request::get(&url)?
		.header("Referer", "https://www.webtoons.com").header("User-Agent", USER_AGENT);
	if let Some(cookie) = image_cookie(&url, context) {
		request = request.header("Cookie", cookie);
	}
	Ok(request)
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn signing_matches_independent_hmac_vector_and_url_prefix_limit() {
		let url = sign_url("https://example.com/a?x=1", 1_700_000_000_000, "key").unwrap();
		assert_eq!(url, "https://example.com/a?x=1&msgpad=1700000000000&md=Ucpvki5iJ0Budc4nbOxdSHrnTyg%3D");
		let long = format!("https://example.com/{}", "a".repeat(260));
		let signed = sign_url(&long, 1_700_000_000_000, "key").unwrap();
		assert!(signed.ends_with("&md=uyAJtFP8XkWWe6FYnFwuC6Hb0hg%3D"));
	}

	#[aidoku_test]
	fn pagination_uses_fixed_thirty_and_keeps_the_last_page() {
		let mut offsets = Vec::new();
		let episodes = collect_episode_pages(31, |batch| {
			offsets.extend_from_slice(batch);
			Ok(batch.iter().map(|offset| decode::<EpisodeList>(if *offset == 0 {
				include_str!("fixtures/app-10568-0.json")
			} else { include_str!("fixtures/app-10568-30.json") }).unwrap()).collect())
		}).unwrap();
		assert_eq!(offsets, [0, 30]);
		assert_eq!(episodes.len(), 31);
		assert_eq!(episodes[0].episode_no, 31);
		assert_eq!(episodes[30].episode_no, 1);
	}

	#[aidoku_test]
	fn incomplete_or_repeated_pages_fail_the_entire_update() {
		assert!(collect_episode_pages(31, |batch| {
			Ok(batch.iter().map(|_| decode::<EpisodeList>(
				include_str!("fixtures/app-10568-0.json")).unwrap()).collect())
		}).is_err());
		assert!(collect_episode_pages(31, |_| Ok(Vec::new())).is_err());
		assert!(collect_episode_pages(31, |_| Err(error!("offline"))).is_err());
	}

	#[aidoku_test]
	fn secure_cookie_is_only_attached_to_product_images() {
		let mut context = PageContext::new();
		context.insert("Cookie".into(), "token=value".into());
		assert_eq!(image_cookie(&format!("{PRODUCT_HOST}/a.jpg"), Some(&context)), Some("token=value"));
		assert_eq!(image_cookie(&format!("{IMAGE_HOST}/a.jpg"), Some(&context)), None);
		assert_eq!(image_cookie("https://prowebtoon-phinf.pstatic.net.evil.test/a.jpg", Some(&context)), None);
	}
}
