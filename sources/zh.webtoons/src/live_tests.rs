//! Opt-in live checks: cargo test --features live-tests
use super::*;
use aidoku_test::aidoku_test;

#[aidoku_test]
fn live_app_only_manga_list_trial_and_image() {
	let source = WebtoonSource::new();
	let manga = source.get_manga_update(Manga {
		key: "10568".into(),
		url: Some("https://www.webtoons.com/zh-hant/action/jujutsukaisen/list?title_no=10568".into()),
		..Default::default()
	}, true, true).unwrap();
	assert_eq!(manga.viewer, Viewer::RightToLeft);
	let chapters = manga.chapters.as_ref().unwrap();
	assert!(chapters.len() >= 31);
	let trial = chapters.iter().find(|chapter| episode_no(&chapter.key) == Some(1)).unwrap().clone();
	assert!(trial.key.starts_with("https://www.webtoons.com/"));
	assert_eq!(api::saved_key("10568", 1).as_deref(), Some(trial.key.as_str()));
	assert!(!trial.locked);
	let paid = chapters.iter().find(|chapter| episode_no(&chapter.key) == Some(2)).unwrap().clone();
	assert!(!paid.locked);
	assert!(source.get_page_list(manga.clone(), paid).is_err());
	let pages = source.get_page_list(manga, trial).unwrap();
	assert!(pages.len() >= 19);
	let PageContent::Url(url, context) = &pages[0].content else { panic!("missing image") };
	let response = source.get_image_request(url.clone(), context.clone()).unwrap().send().unwrap();
	assert_eq!(response.status_code(), 200);
	let bytes = response.get_data().unwrap();
	assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]));
}

#[aidoku_test]
fn live_latest_api_includes_preview_episodes_missing_from_old_api() {
	let source = WebtoonSource::new();
	let expected = api::title_info("3123").unwrap().total_episode_count;
	let manga = source.get_manga_update(Manga { key: "3123".into(), ..Default::default() }, false, true).unwrap();
	let chapters = manga.chapters.as_ref().unwrap();
	assert_eq!(chapters.len(), expected);
	assert!(chapters.len() >= 271);
	assert!(chapters.iter().any(|chapter| chapter.title.as_deref().is_some_and(|title| title.contains("需官方 App"))));
	assert!(chapters.iter().any(|chapter| !chapter.locked));
	let free = chapters.iter().find(|chapter| !chapter.title.as_deref().is_some_and(|title| title.contains("需官方 App"))).unwrap().clone();
	let pages = source.get_page_list(manga, free).unwrap();
	assert!(!pages.is_empty());
}
