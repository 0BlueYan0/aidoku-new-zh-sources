//! Request building and response reading.
//!
//! Building a request only reads defaults: it never sends another request, so entry points
//! cannot hold request slots while waiting for each other. Sessions are handled by
//! `auth::ensure_session` at the top of each entry point.

use crate::{auth, crypto, settings};
use aidoku::{
	alloc::{string::ToString, String},
	helpers::uri::encode_uri_component,
	imports::{
		net::{HttpMethod, Request, Response},
		std::current_date,
	},
	prelude::*,
	FilterValue, Result,
};
use serde::de::DeserializeOwned;

const API_KEY: &str = "C69BAF41DA5ABD1FFEDC6D2FEA56B";
const KEY: &[u8; 63] = br"~d}$Q7$eIni=V)9\RK/P.RM4;9[7|@/CA}b~OW!3?EV`:<>M7pddUBL5n|0/*Cn";

/// API base for a line from the settings. Anything unknown falls back to the default proxy.
pub fn api_base_for(line: &str) -> String {
	String::from(match line {
		"acbbb.com" => "https://picaapi.acbbb.com",
		"official" => "https://picaapi.picacomic.com",
		_ => "https://picaapi.go2778.com",
	})
}

pub fn api_base() -> String {
	api_base_for(&settings::get_api_line())
}

pub fn gen_time() -> String {
	current_date().to_string()
}

pub fn gen_nonce(time: &str) -> String {
	format!("{:x}", md5::compute(time))
}

/// The part of the URL after `<base>/`, which is what the signature covers.
pub fn signature_path<'a>(base: &str, url: &'a str) -> &'a str {
	url.strip_prefix(base)
		.map(|rest| rest.trim_start_matches('/'))
		.unwrap_or(url)
}

pub fn gen_signature(path: &str, time: &str, nonce: &str, method: &str) -> Result<String> {
	let text = format!("{path}{time}{nonce}{method}{API_KEY}").to_ascii_lowercase();
	crypto::encrypt(text.as_bytes(), KEY)
}

/// Rewrite the host of an image server for a proxy line, the way PicaWeb does: `picacomic.com`
/// and `manhuabika.com` become the line, `diwodiwo.xyz` becomes `storage1.<line>`. Only the
/// host changes. The official line keeps the host as the API gave it.
pub fn rewrite_image_host(file_server: &str, line: &str) -> String {
	if line == "official" {
		return String::from(file_server);
	}
	let (scheme, rest) = match file_server.find("://") {
		Some(index) => file_server.split_at(index + 3),
		None => ("", file_server),
	};
	let (host, tail) = match rest.find('/') {
		Some(index) => rest.split_at(index),
		None => (rest, ""),
	};
	let host = if host == "diwodiwo.xyz" {
		format!("storage1.{line}")
	} else if let Some(prefix) = host.strip_suffix(".picacomic.com") {
		format!("{prefix}.{line}")
	} else if host == "picacomic.com" || host == "manhuabika.com" {
		String::from(line)
	} else if let Some(prefix) = host.strip_suffix(".manhuabika.com") {
		format!("{prefix}.{line}")
	} else {
		String::from(host)
	};
	format!("{scheme}{host}{tail}")
}

/// `<fileServer>/static/<path>` with the host rewritten for the selected line.
pub fn image_url(file_server: &str, path: &str, line: &str) -> String {
	let server = file_server.trim_end_matches('/');
	let server = server.strip_suffix("/static").unwrap_or(server);
	format!("{}/static/{path}", rewrite_image_host(server, line))
}

/// The category options of `res/filters.json` after the default "全部" (a test keeps the two
/// lists identical).
const CATEGORIES: [&str; 40] = [
	"大家都在看",
	"大濕推薦",
	"那年今天",
	"官方都在看",
	"嗶咔漢化",
	"全彩",
	"長篇",
	"同人",
	"短篇",
	"圓神領域",
	"碧藍幻想",
	"CG雜圖",
	"英語 ENG",
	"生肉",
	"純愛",
	"百合花園",
	"耽美花園",
	"偽娘哲學",
	"後宮閃光",
	"扶他樂園",
	"單行本",
	"姐姐系",
	"妹妹系",
	"SM",
	"性轉換",
	"足の恋",
	"人妻",
	"NTR",
	"強暴",
	"非人類",
	"艦隊收藏",
	"Love Live",
	"SAO 刀劍神域",
	"Fate",
	"東方",
	"WEBTOON",
	"禁書目錄",
	"歐美",
	"Cosplay",
	"重口地帶",
];

fn is_category(name: &str) -> bool {
	CATEGORIES.contains(&name)
}

pub enum Url {
	Explore {
		category: Option<String>,
		author: Option<String>,
		tag: Option<String>,
		sort: String,
		page: i32,
	},
	Search {
		query: String,
		sort: String,
		page: i32,
	},
	Rank {
		time: &'static str,
	},
	Random,
	Favourite {
		sort: String,
		page: i32,
	},
	Profile,
	Manga {
		id: String,
	},
	ChapterList {
		id: String,
		page: i32,
	},
	PageList {
		manga_id: String,
		chapter_id: String,
		page: i32,
	},
}

impl Url {
	/// The list for a search box entry or a set of filters. What was chosen is what is sent.
	pub fn from_query_or_filters(query: Option<&str>, page: i32, filters: &[FilterValue]) -> Self {
		let mut category = None;
		let mut author = None;
		let mut tag = None;
		let mut keyword = None;
		let mut sort = String::from("dd");

		for filter in filters {
			match filter {
				FilterValue::Text { id, value } => {
					if id == "author" {
						author = Some(value.clone());
					} else {
						keyword = Some(value.clone());
					}
				}
				FilterValue::Select { id, value } => match id.as_str() {
					// "全部" is the default option, which the app does not send.
					"分類" => category = Some(value.clone()),
					// A tapped tag. Category names are not tags on the site (`t=全彩`
					// matches fewer works, `t=短篇` none), so they go to `c=`.
					"genre" if is_category(value) => category = Some(value.clone()),
					"genre" => tag = Some(value.clone()),
					_ => {}
				},
				FilterValue::Sort { id, index, .. } if id == "排序" => {
					if let Some(value) = ["dd", "da", "ld", "vd"].get(*index as usize) {
						sort = String::from(*value);
					}
				}
				_ => {}
			}
		}

		if let Some(query) = query.map(String::from).or(keyword) {
			return Self::Search { query, sort, page };
		}
		Self::Explore {
			category,
			author,
			tag,
			sort,
			page,
		}
	}

	pub fn explore_category(category: &str, page: i32) -> Self {
		Self::Explore {
			category: Some(String::from(category)),
			author: None,
			tag: None,
			sort: String::from("dd"),
			page,
		}
	}

	pub fn latest(page: i32) -> Self {
		Self::Explore {
			category: None,
			author: None,
			tag: None,
			sort: String::from("dd"),
			page,
		}
	}

	pub fn to_url(&self, base: &str) -> String {
		match self {
			Url::Explore {
				category,
				author,
				tag,
				sort,
				page,
			} => {
				let mut url = format!("{base}/comics?page={page}");
				for (name, value) in [("c", category), ("a", author), ("t", tag)] {
					if let Some(value) = value {
						url.push_str(&format!("&{name}={}", encode_uri_component(value)));
					}
				}
				url.push_str(&format!("&s={sort}"));
				url
			}
			Url::Search { page, .. } => format!("{base}/comics/advanced-search?page={page}"),
			Url::Rank { time } => format!("{base}/comics/leaderboard?tt={time}&ct=VC"),
			Url::Random => format!("{base}/comics/random"),
			Url::Favourite { sort, page } => format!("{base}/users/favourite?page={page}&s={sort}"),
			Url::Profile => format!("{base}/users/profile"),
			Url::Manga { id } => format!("{base}/comics/{id}"),
			Url::ChapterList { id, page } => format!("{base}/comics/{id}/eps?page={page}"),
			Url::PageList {
				manga_id,
				chapter_id,
				page,
			} => format!("{base}/comics/{manga_id}/order/{chapter_id}/pages?page={page}"),
		}
	}
}

/// The signed request for a URL, with the stored token when there is one.
pub fn build_request(url: &Url) -> Result<Request> {
	let base = api_base();
	let (method, body) = match url {
		Url::Search { query, sort, .. } => (
			HttpMethod::Post,
			Some(serde_json::json!({ "keyword": query, "sort": sort }).to_string()),
		),
		_ => (HttpMethod::Get, None),
	};
	create_request(url.to_url(&base), &base, method, body, true)
}

/// `authorize` false leaves the Authorization header out, for the sign-in request.
pub fn create_request(
	url: String,
	base: &str,
	method: HttpMethod,
	body: Option<String>,
	authorize: bool,
) -> Result<Request> {
	let method_name = match method {
		HttpMethod::Post => "POST",
		_ => "GET",
	};
	let time = gen_time();
	let nonce = gen_nonce(&time);
	let signature = gen_signature(signature_path(base, &url), &time, &nonce, method_name)?;

	let mut request = Request::new(url, method)?
		.header("api-key", API_KEY)
		.header("app-build-version", "45")
		.header("app-channel", &settings::get_app_channel())
		.header("app-platform", "android")
		.header("app-uuid", "defaultUuid")
		.header("app-version", "2.2.1.3.3.4")
		.header("image-quality", &settings::get_image_quality())
		.header("time", &time)
		.header("nonce", &nonce)
		.header("signature", &signature)
		.header("Accept", "application/vnd.picacomic.com.v1+json")
		.header("Content-Type", "application/json; charset=UTF-8")
		.header("User-Agent", "okhttp/3.8.1");
	if authorize {
		if let Some(token) = auth::token() {
			request = request.header("Authorization", &token);
		}
	}
	if let Some(body) = body {
		request = request.body(body.as_bytes());
	}
	Ok(request)
}

/// Read a response body as JSON. A 401 drops the stored token (the next entry point signs in
/// again) and every non-2xx status is an error.
pub fn read_json<T: DeserializeOwned>(response: &Response) -> Result<T> {
	let status = response.status_code();
	if status == 401 {
		auth::on_unauthorized();
		bail!("[picacomic2] unauthorized (HTTP 401)");
	}
	if !(200..300).contains(&status) {
		bail!("[picacomic2] HTTP {status}");
	}
	let text = response.get_string()?;
	serde_json::from_str(&text).map_err(|_| error!("[picacomic2] unexpected response body"))
}

/// One request in, one result out. Nothing else is sent while this runs.
pub fn request_json<T: DeserializeOwned>(url: Url) -> Result<T> {
	let response = build_request(&url)?.send()?;
	read_json(&response)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	const BASE: &str = "https://picaapi.go2778.com";

	#[aidoku_test]
	fn api_base_per_line() {
		assert_eq!(api_base_for("go2778.com"), "https://picaapi.go2778.com");
		assert_eq!(api_base_for("acbbb.com"), "https://picaapi.acbbb.com");
		assert_eq!(api_base_for("official"), "https://picaapi.picacomic.com");
		assert_eq!(api_base_for("other"), "https://picaapi.go2778.com");
	}

	#[aidoku_test]
	fn signature_covers_only_the_path() {
		assert_eq!(
			signature_path(BASE, "https://picaapi.go2778.com/comics?page=1&s=dd"),
			"comics?page=1&s=dd"
		);
		assert_eq!(
			signature_path(
				"https://picaapi.picacomic.com",
				"https://picaapi.picacomic.com/auth/sign-in"
			),
			"auth/sign-in"
		);
		// A URL on another host is not stripped.
		assert_eq!(
			signature_path(BASE, "https://example.com/comics"),
			"https://example.com/comics"
		);
		let signature = gen_signature("comics?page=1", "1700000000", "n", "GET").unwrap();
		assert_eq!(signature.len(), 64);
		assert!(signature
			.bytes()
			.all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
		// The signed text is lowercased, so the case of the input does not matter.
		assert_eq!(
			signature,
			gen_signature("COMICS?page=1", "1700000000", "N", "get").unwrap()
		);
	}

	#[aidoku_test]
	fn nonce_is_md5_of_the_time() {
		assert_eq!(gen_nonce(""), "d41d8cd98f00b204e9800998ecf8427e");
		assert_eq!(gen_nonce("1700000000").len(), 32);
	}

	#[aidoku_test]
	fn image_hosts_per_line() {
		assert_eq!(
			rewrite_image_host("https://storage-b.picacomic.com", "go2778.com"),
			"https://storage-b.go2778.com"
		);
		assert_eq!(
			rewrite_image_host("https://storage1.picacomic.com", "acbbb.com"),
			"https://storage1.acbbb.com"
		);
		assert_eq!(
			rewrite_image_host("https://diwodiwo.xyz", "go2778.com"),
			"https://storage1.go2778.com"
		);
		assert_eq!(
			rewrite_image_host("https://picacomic.com", "go2778.com"),
			"https://go2778.com"
		);
		assert_eq!(
			rewrite_image_host("https://manhuabika.com", "go2778.com"),
			"https://go2778.com"
		);
		assert_eq!(
			rewrite_image_host("https://storage-b.picacomic.com", "official"),
			"https://storage-b.picacomic.com"
		);
		// Another host is left alone, and the path is never touched.
		assert_eq!(
			rewrite_image_host("https://img.example.com/picacomic.com", "go2778.com"),
			"https://img.example.com/picacomic.com"
		);
		assert_eq!(
			image_url(
				"https://storage-b.picacomic.com",
				"tobs/a.jpg",
				"go2778.com"
			),
			"https://storage-b.go2778.com/static/tobs/a.jpg"
		);
		assert_eq!(
			image_url("https://diwodiwo.xyz/static", "help.jpg", "go2778.com"),
			"https://storage1.go2778.com/static/help.jpg"
		);
	}

	#[aidoku_test]
	fn list_urls() {
		assert_eq!(
			Url::latest(2).to_url(BASE),
			"https://picaapi.go2778.com/comics?page=2&s=dd"
		);
		assert_eq!(
			Url::explore_category("大家都在看", 1).to_url(BASE),
			"https://picaapi.go2778.com/comics?page=1&c=%E5%A4%A7%E5%AE%B6%E9%83%BD%E5%9C%A8%E7%9C%8B&s=dd"
		);
		assert_eq!(
			Url::Rank { time: "D7" }.to_url(BASE),
			"https://picaapi.go2778.com/comics/leaderboard?tt=D7&ct=VC"
		);
		assert_eq!(
			Url::PageList {
				manga_id: String::from("m"),
				chapter_id: String::from("2"),
				page: 3
			}
			.to_url(BASE),
			"https://picaapi.go2778.com/comics/m/order/2/pages?page=3"
		);
	}

	/// The code's category list is the option list of `res/filters.json`.
	#[aidoku_test]
	fn categories_match_filters_json() {
		let filters: serde_json::Value =
			serde_json::from_str(include_str!("../res/filters.json")).unwrap();
		let options = filters
			.as_array()
			.unwrap()
			.iter()
			.find(|filter| filter["id"] == "分類")
			.unwrap()["options"]
			.as_array()
			.unwrap();
		assert_eq!(options[0], "全部");
		assert_eq!(options.len() - 1, CATEGORIES.len());
		for (option, category) in options[1..].iter().zip(CATEGORIES) {
			assert_eq!(option, category);
		}
	}

	/// A tapped category name goes to `c=`, any other tag to `t=`.
	#[aidoku_test]
	fn tapped_tags_split_into_category_and_tag() {
		let tapped = |name: &str| {
			Url::from_query_or_filters(
				None,
				1,
				&[FilterValue::Select {
					id: String::from("genre"),
					value: String::from(name),
				}],
			)
			.to_url(BASE)
		};
		for name in ["全彩", "CG雜圖", "短篇", "WEBTOON"] {
			let url = tapped(name);
			assert!(url.contains("&c="), "{url}");
			assert!(!url.contains("&t="), "{url}");
		}
		let url = tapped("巨乳");
		assert!(url.contains("&t="), "{url}");
		assert!(!url.contains("&c="), "{url}");
	}

	/// What was chosen is what is sent. An author or tag with `&` or `+` stays one value.
	#[aidoku_test]
	fn filters_become_query_parts() {
		let author = Url::from_query_or_filters(
			None,
			1,
			&[FilterValue::Text {
				id: String::from("author"),
				value: String::from("A&B+C"),
			}],
		);
		assert_eq!(
			author.to_url(BASE),
			"https://picaapi.go2778.com/comics?page=1&a=A%26B%2BC&s=dd"
		);
		let tag = Url::from_query_or_filters(
			None,
			2,
			&[FilterValue::Select {
				id: String::from("genre"),
				value: String::from("巨乳"),
			}],
		);
		assert!(tag.to_url(BASE).contains("&t=%E5%B7%A8%E4%B9%B3&s=dd"));
		let both = Url::from_query_or_filters(
			None,
			1,
			&[
				FilterValue::Select {
					id: String::from("分類"),
					value: String::from("全彩"),
				},
				FilterValue::Sort {
					id: String::from("排序"),
					index: 2,
					ascending: false,
				},
			],
		);
		assert!(both.to_url(BASE).ends_with("&c=%E5%85%A8%E5%BD%A9&s=ld"));
		assert!(matches!(
			Url::from_query_or_filters(Some("abc"), 1, &[]),
			Url::Search { .. }
		));
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/net.rs"]
mod fixture_tests;
