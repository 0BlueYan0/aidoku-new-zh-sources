use aidoku::{
	alloc::{string::ToString, vec, String, Vec},
	imports::html::{Document, Element},
	prelude::*,
	Chapter, Manga, Viewer,
};

pub const BASE_URL: &str = "https://www.bookwalker.com.tw";
const COVER_URL: &str = "https://taiwan-image.bookwalker.com.tw/product";
pub const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

/// Manga keys carry a prefix because the bookcase mixes two kinds of card: a series
/// (`s<series id>`) and a book that has no series card (`p<product id>`).
pub const SERIES_PREFIX: &str = "s";
pub const PRODUCT_PREFIX: &str = "p";

/// One card of the bookcase grid.
pub struct BookCard {
	pub series_id: Option<String>,
	pub product_ids: Vec<String>,
	pub name: String,
	pub authors: Vec<String>,
	pub category: Option<String>,
}

impl BookCard {
	pub fn key(&self) -> Option<String> {
		match &self.series_id {
			Some(id) => Some(format!("{SERIES_PREFIX}{id}")),
			None => self
				.product_ids
				.first()
				.map(|pid| format!("{PRODUCT_PREFIX}{pid}")),
		}
	}

	/// The newest volume a series card lists is the one whose cover the site shows.
	pub fn cover_product(&self) -> Option<&String> {
		self.product_ids.last()
	}

	pub fn to_manga(&self) -> Option<Manga> {
		let key = self.key()?;
		let url = match &self.series_id {
			Some(id) => format!("{BASE_URL}/bookcase/available_book_list/buy?s={id}"),
			None => format!("{BASE_URL}/product/{}", self.product_ids.first()?),
		};
		Some(Manga {
			key,
			title: series_title(&self.name),
			cover: self.cover_product().map(|pid| cover_url(pid)),
			authors: (!self.authors.is_empty()).then(|| self.authors.clone()),
			url: Some(url),
			tags: self.category.clone().map(|c| vec![c]),
			viewer: viewer_for(self.category.as_deref()),
			..Default::default()
		})
	}

	/// A card on the per-series page (`sd=0`) is always a single volume. A volume with
	/// no numeric suffix (`書名 (上)`) would otherwise have nothing to show in the list.
	pub fn to_chapter(&self) -> Option<Chapter> {
		let pid = self.product_ids.first()?;
		let volume_number = volume_number(&self.name);
		Some(Chapter {
			key: pid.clone(),
			title: volume_number.is_none().then(|| self.name.clone()),
			volume_number,
			url: Some(format!("{BASE_URL}/product/{pid}")),
			thumbnail: Some(cover_url(pid)),
			..Default::default()
		})
	}
}

pub fn cover_url(pid: &str) -> String {
	format!("{COVER_URL}/{pid}/{pid}_1.jpg")
}

/// Manga read right to left; everything else keeps the app's default.
fn viewer_for(category: Option<&str>) -> Viewer {
	match category {
		Some("漫畫") => Viewer::RightToLeft,
		_ => Viewer::Unknown,
	}
}

/// A signed-out request is redirected to the login page, which has no book grid.
pub fn is_bookcase(html: &Document) -> bool {
	html.select_first("ul.readerBookList").is_some()
}

pub fn parse_bookcase(html: &Document) -> Vec<BookCard> {
	let Some(items) = html.select("ul.readerBookList > li") else {
		return Vec::new();
	};
	items.filter_map(|item| parse_card(&item)).collect()
}

fn parse_card(item: &Element) -> Option<BookCard> {
	let href = item
		.select_first(".readerBookBorder > a")
		.and_then(|a| a.attr("href"))
		.unwrap_or_default();
	let series_id = query_value(&href, "s");
	let product_ids: Vec<String> = item
		.select_first("input[name=products]")
		.and_then(|input| input.attr("value"))
		.map(|value| {
			value
				.split(',')
				.map(|pid| pid.trim().to_string())
				.filter(|pid| !pid.is_empty())
				.collect()
		})
		.unwrap_or_default();
	if product_ids.is_empty() {
		return None;
	}
	let name = text_of(item, ".readerBookName")?;
	let authors = text_of(item, ".readerBookAuthor")
		.map(|text| {
			text.split('/')
				.map(|author| author.trim().to_string())
				.filter(|author| !author.is_empty())
				.collect()
		})
		.unwrap_or_default();
	Some(BookCard {
		series_id,
		product_ids,
		name,
		authors,
		category: text_of(item, ".readerBookSort"),
	})
}

fn text_of(item: &Element, selector: &str) -> Option<String> {
	item.select_first(selector)
		.and_then(|el| el.text())
		.filter(|text| !text.is_empty())
}

pub fn query_value(url: &str, name: &str) -> Option<String> {
	let query = url.split_once('?')?.1;
	query.split('&').find_map(|pair| {
		let (key, value) = pair.split_once('=')?;
		(key == name && !value.is_empty()).then(|| value.to_string())
	})
}

/// The site names every volume `<series> (N)`; the series card carries the newest
/// volume's name, so the suffix is dropped to get the series title.
pub fn series_title(name: &str) -> String {
	match volume_suffix_start(name) {
		Some(start) => name[..start].trim_end().to_string(),
		None => name.to_string(),
	}
}

pub fn volume_number(name: &str) -> Option<f32> {
	let start = volume_suffix_start(name)?;
	name[start..]
		.trim_start_matches('(')
		.trim_end_matches(')')
		.parse::<f32>()
		.ok()
}

fn volume_suffix_start(name: &str) -> Option<usize> {
	let trimmed = name.trim_end();
	if !trimmed.ends_with(')') {
		return None;
	}
	let start = trimmed.rfind('(')?;
	let inner = &trimmed[start + 1..trimmed.len() - 1];
	let is_number = !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '.');
	is_number.then_some(start)
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn strips_volume_suffix() {
		assert_eq!(series_title("ATRI -My Dear Moments- (4)"), "ATRI -My Dear Moments-");
		assert_eq!(series_title("靠死亡遊戲混飯吃。 (12)"), "靠死亡遊戲混飯吃。");
		assert_eq!(series_title("沒有集數的書"), "沒有集數的書");
		assert_eq!(series_title("書名 (上)"), "書名 (上)");
		assert_eq!(volume_number("敗北女角太多了！@comic (3)"), Some(3.0));
		assert_eq!(volume_number("沒有集數的書"), None);
	}

	#[aidoku_test]
	fn reads_query_values() {
		assert_eq!(
			query_value("https://www.bookwalker.com.tw/bookcase/available_book_list?s=29604", "s"),
			Some("29604".to_string())
		);
		assert_eq!(query_value("https://www.bookwalker.com.tw/browserViewer/266673/read", "s"), None);
	}

	/// Structure copied from the live bookcase (`buy?sd=1`, 2026-09-23), names replaced.
	const BOOKCASE: &str = r#"<html><body>
<ul class="readerBookList  picType ">
	<li>
		<div class="readerBooks">
			<div class="readerBookBorder seriesBorder ">
				<a href="https://www.bookwalker.com.tw/bookcase/available_book_list?s=29604" target="_blank">
					<img src="https://taiwan-image.bookwalker.com.tw/product/300/300_1.jpg" class="readerBookPic">
					<span class="readerBookNum">4</span>
				</a>
				<label class="inputCheckStyle readerBookCheck hidden">
					<input type="checkbox" name="products" value="100,200,250,300" class="inputCheck"><span></span>
				</label>
			</div>
		</div>
		<div class="readerBookListType">
			<div class="readerBookName">系列甲 (4)</div>
			<div class="readerBookAuthor">作者一 / 作者二</div>
			<div class="readerBookSort">漫畫</div>
		</div>
	</li>
	<li>
		<div class="readerBooks">
			<div class="readerBookBorder ">
				<a href="https://www.bookwalker.com.tw/browserViewer/266673/read" target="_blank">
					<img src="https://taiwan-image.bookwalker.com.tw/product/266673/266673_1.jpg" class="readerBookPic">
					<span class="readerBookNum">5</span>
				</a>
				<label class="inputCheckStyle readerBookCheck hidden">
					<input type="checkbox" name="products" value="266673" class="inputCheck"><span></span>
				</label>
			</div>
		</div>
		<div class="readerBookListType">
			<div class="readerBookName">單本乙 (5)</div>
			<div class="readerBookAuthor">作者三</div>
			<div class="readerBookSort">輕小說</div>
		</div>
	</li>
</ul>
<a href="https://cp.bookwalker.com.tw/event/"><img alt="banner"></a>
</body></html>"#;

	#[aidoku_test]
	fn parses_bookcase_cards() {
		let html = aidoku::imports::html::Html::parse(BOOKCASE).unwrap();
		assert!(is_bookcase(&html));
		let cards = parse_bookcase(&html);
		assert_eq!(cards.len(), 2);

		let series = &cards[0];
		assert_eq!(series.key(), Some("s29604".to_string()));
		assert_eq!(series.product_ids, ["100", "200", "250", "300"]);
		assert_eq!(series.authors, ["作者一", "作者二"]);
		let manga = series.to_manga().unwrap();
		assert_eq!(manga.title, "系列甲");
		assert_eq!(
			manga.cover.as_deref(),
			Some("https://taiwan-image.bookwalker.com.tw/product/300/300_1.jpg")
		);
		assert_eq!(manga.viewer, Viewer::RightToLeft);

		let single = &cards[1];
		assert_eq!(single.key(), Some("p266673".to_string()));
		assert_eq!(single.category.as_deref(), Some("輕小說"));
		let chapter = single.to_chapter().unwrap();
		assert_eq!(chapter.volume_number, Some(5.0));
		assert_eq!(chapter.title, None);
	}

	#[aidoku_test]
	fn unnumbered_volume_keeps_its_name_as_title() {
		let card = BookCard {
			series_id: None,
			product_ids: vec!["1".to_string()],
			name: "書名 (上)".to_string(),
			authors: Vec::new(),
			category: None,
		};
		let chapter = card.to_chapter().unwrap();
		assert_eq!(chapter.volume_number, None);
		assert_eq!(chapter.title.as_deref(), Some("書名 (上)"));
	}

	#[aidoku_test]
	fn login_page_is_not_a_bookcase() {
		let html = aidoku::imports::html::Html::parse("<html><body><form id=\"login\"></form></body></html>").unwrap();
		assert!(!is_bookcase(&html));
		assert!(parse_bookcase(&html).is_empty());
	}
}
