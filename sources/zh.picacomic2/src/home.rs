use crate::{
	auth,
	json::{ExploreResponse, RankResponse},
	net::{self, Url},
	punch, settings, Picacomic2,
};
use aidoku::{
	alloc::{String, Vec},
	imports::{net::Request, std::send_partial_result},
	AidokuError, Home, HomeComponent, HomeComponentValue, HomeLayout, HomePartialResult, Link,
	Listing, MangaPageResult, Result,
};

/// Requests per `send_all`: the app runs at most 5 at once, and more in one call can hang.
const HOME_BATCH: usize = 5;

#[derive(Clone, Copy)]
enum Shape {
	Big,
	Ranked,
	Scroller,
}

#[derive(Clone, Copy)]
enum Feed {
	Rank(&'static str),
	Category(&'static str),
	Latest,
}

/// One home row. `id` is the listing its 「更多」 opens, as in `res/source.json`.
#[derive(Clone, Copy)]
struct Row {
	id: &'static str,
	title: &'static str,
	listing_name: &'static str,
	shape: Shape,
	feed: Feed,
}

const ROWS: [Row; 8] = [
	Row {
		id: "dayup",
		title: "日榜",
		listing_name: "日榜",
		shape: Shape::Big,
		feed: Feed::Rank("H24"),
	},
	Row {
		id: "weekup",
		title: "週榜",
		listing_name: "週榜",
		shape: Shape::Ranked,
		feed: Feed::Rank("D7"),
	},
	Row {
		id: "monthup",
		title: "月榜",
		listing_name: "月榜",
		shape: Shape::Ranked,
		feed: Feed::Rank("D30"),
	},
	Row {
		id: "djkz",
		title: "大家都在看",
		listing_name: "大家都在看",
		shape: Shape::Scroller,
		feed: Feed::Category("大家都在看"),
	},
	Row {
		id: "gfdjkz",
		title: "官方都在看",
		listing_name: "官方都在看",
		shape: Shape::Scroller,
		feed: Feed::Category("官方都在看"),
	},
	Row {
		id: "dswj",
		title: "大濕推薦",
		listing_name: "大濕推薦",
		shape: Shape::Scroller,
		feed: Feed::Category("大濕推薦"),
	},
	Row {
		id: "nndtn",
		title: "那年今天",
		listing_name: "那年今天",
		shape: Shape::Scroller,
		feed: Feed::Category("那年今天"),
	},
	Row {
		id: "update",
		title: "最近更新",
		listing_name: "更新",
		shape: Shape::Scroller,
		feed: Feed::Latest,
	},
];

impl Row {
	fn url(&self) -> Url {
		match self.feed {
			Feed::Rank(time) => Url::Rank { time },
			Feed::Category(category) => Url::explore_category(category, 1),
			Feed::Latest => Url::latest(1),
		}
	}

	fn skeleton(&self) -> HomeComponent {
		HomeComponent {
			title: Some(String::from(self.title)),
			subtitle: None,
			value: match self.shape {
				Shape::Big => HomeComponentValue::empty_big_scroller(),
				Shape::Ranked => HomeComponentValue::empty_manga_list(),
				Shape::Scroller => HomeComponentValue::empty_scroller(),
			},
		}
	}

	fn listing(&self) -> Option<Listing> {
		Some(Listing {
			id: String::from(self.id),
			name: String::from(self.listing_name),
			..Default::default()
		})
	}

	fn component(&self, page: MangaPageResult) -> HomeComponent {
		let value = match self.shape {
			Shape::Big => HomeComponentValue::BigScroller {
				entries: page.entries,
				auto_scroll_interval: Some(8.0),
			},
			Shape::Ranked => HomeComponentValue::MangaList {
				ranking: true,
				page_size: Some(3),
				entries: page.entries.into_iter().map(Link::from).collect(),
				listing: self.listing(),
			},
			Shape::Scroller => HomeComponentValue::Scroller {
				entries: page.entries.into_iter().map(Link::from).collect(),
				listing: self.listing(),
			},
		};
		HomeComponent {
			title: Some(String::from(self.title)),
			subtitle: None,
			value,
		}
	}
}

impl Home for Picacomic2 {
	fn get_home(&self) -> Result<HomeLayout> {
		send_partial_result(&HomePartialResult::Layout(HomeLayout {
			components: ROWS.iter().map(Row::skeleton).collect(),
		}));

		// The only requests this call can make before the rows are the sign-in and the daily
		// check-in, and both are gated.
		auth::ensure_session();
		punch::punch_if_due();

		// Rows go out together so the covers, which queue behind this call, start sooner.
		// A row whose request cannot be built is skipped.
		let mut rows = Vec::new();
		let mut requests: Vec<Request> = Vec::new();
		for row in ROWS {
			if let Ok(request) = net::build_request(&row.url()) {
				rows.push(row);
				requests.push(request);
			}
		}
		let mut responses = Vec::new();
		let mut requests = requests.into_iter().peekable();
		while requests.peek().is_some() {
			let batch: Vec<Request> = requests.by_ref().take(HOME_BATCH).collect();
			responses.extend(Request::send_all(batch));
		}

		let line = settings::get_api_line();
		let blocklist = settings::get_blocklist();
		let mut responses = responses.into_iter();
		let mut shown = false;
		let mut first_error: Option<AidokuError> = None;
		for row in rows {
			let Some(response) = responses.next() else {
				break;
			};
			let page = response
				.map_err(AidokuError::from)
				.and_then(|response| match row.feed {
					Feed::Rank(_) => net::read_json::<RankResponse>(&response)
						.map(|body| body.data.into_page(&line, &blocklist)),
					_ => net::read_json::<ExploreResponse>(&response)
						.map(|body| body.data.into_page(&line, &blocklist)),
				});
			match page {
				Ok(page) => {
					shown = true;
					if !page.entries.is_empty() {
						send_partial_result(&HomePartialResult::Component(row.component(page)));
					}
				}
				Err(error) => {
					first_error.get_or_insert(error);
				}
			}
		}

		// A home with no row at all (offline, signed out, a line that is down) is an error the
		// reader can see, not an empty page.
		match first_error {
			Some(error) if !shown => Err(error),
			_ => Ok(HomeLayout::default()),
		}
	}
}
