use aidoku::{
	alloc::Vec, prelude::*, Home, HomeComponent, HomeComponentValue, HomeLayout, Listing, Result,
};

use crate::block::BlockState;
use crate::helpers::{api_post, fetch_json, get_adult_mode, get_img_base};
use crate::models::{visible, HomeResp};
use crate::{auth, NoyAcg};

impl Home for NoyAcg {
	fn get_home(&self) -> Result<HomeLayout> {
		auth::ensure_session();
		auth::try_daily_signin();
		let home: HomeResp =
			fetch_json(|| api_post("/api/home", "v=3&stream_all=1", get_adult_mode()))?;
		home.check()?;

		let img_base = get_img_base();
		let block = BlockState::load();
		let mut components = Vec::new();
		// Whether the site sent any work at all, before the block list.
		let mut any_work = false;
		if !home.tags.is_empty() {
			components.push(HomeComponent {
				title: Some("標籤推薦".into()),
				value: HomeComponentValue::Filters(home.tags.into_iter().map(Into::into).collect()),
				..Default::default()
			});
		}
		for (title, entries, listing_id) in [
			("今日閱讀榜", home.read_day, Some("read:day")),
			("今日收藏榜", home.fav_day, Some("fav:day")),
			("高質榜", home.proportion, Some("proportion")),
			("收藏推薦", home.fs, None),
		] {
			any_work |= !entries.is_empty();
			let entries: Vec<_> = visible(entries, &block)
				.map(|m| m.into_basic_manga(&img_base).into())
				.collect();
			if entries.is_empty() {
				continue;
			}
			components.push(HomeComponent {
				title: Some(title.into()),
				value: HomeComponentValue::Scroller {
					entries,
					listing: listing_id.map(|id| Listing {
						id: id.into(),
						name: title.into(),
						..Default::default()
					}),
				},
				..Default::default()
			});
		}

		// Every row hidden by the block list is not a server problem: show what is left.
		if components.is_empty() && !any_work {
			bail!("無法取得資料，請嘗試切換分流伺服器");
		}
		Ok(HomeLayout { components })
	}
}
