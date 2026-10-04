//! A chapter page's `#acontent` to Markdown.
//!
//! A chapter becomes one `PageContent::Text` with its illustrations inline as Markdown
//! images, the way zh.lightnovel renders its novels (smooth scrolling under
//! `Viewer::Vertical` on the device, 2026-09-29). The paragraphs arrive shuffled and
//! `reorder` puts them back before anything is written.
//!
//! The inline conversion (entities, ruby readings, escaping) is zh.lightnovel's
//! `content.rs` without its font map and EPUB heading classes.

use aidoku::{
	alloc::{format, String, Vec},
	imports::html::{Document, Element},
};

use crate::reorder;

/// What the site writes in place of the paragraphs it withheld from a request it took
/// for a bot. The served paragraphs are then a fraction of the page.
const WITHHELD: [&str; 2] = ["內容加載失敗", "内容加载失败"];

/// One child of `#acontent` worth keeping.
enum Block {
	/// Inner HTML of a non-empty `<p>`. Only these take part in the shuffle.
	Paragraph(String),
	Image(String),
}

/// Whether the page lost paragraphs to the site's bot check.
pub fn is_withheld(html: &str) -> bool {
	WITHHELD.iter().any(|marker: &&str| html.contains(marker))
}

/// The address of an `<img>`: lazy-loaded ones keep it in `data-src` behind a spinner.
fn image_source(img: &Element) -> Option<String> {
	["data-src", "src"]
		.iter()
		.filter_map(|attr: &&str| img.attr(attr))
		.map(|src: String| String::from(src.trim()))
		.find(|src: &String| src.starts_with("http"))
}

/// The illustrations of a block, including the ones an illustration chapter keeps in a
/// `display:none` `#hidden-images` box: they are the volume's other colour plates.
fn images_in(el: &Element, out: &mut Vec<Block>) {
	if el.tag_name().as_deref() == Some("img") {
		if let Some(src) = image_source(el) {
			out.push(Block::Image(src));
		}
		return;
	}
	for img in el.select("img").into_iter().flatten() {
		if let Some(src) = image_source(&img) {
			out.push(Block::Image(src));
		}
	}
}

/// The blocks of one page in reading order.
fn blocks(html: &Document, chapter_id: u64) -> Vec<Block> {
	let Some(container) = html.select_first("#acontent") else {
		return Vec::new();
	};
	let mut served: Vec<Block> = Vec::new();
	for child in container.children() {
		match child.tag_name().as_deref() {
			Some("p") => {
				let inner = child.html().unwrap_or_default();
				// The script counts a paragraph when its inner HTML has anything but
				// whitespace, so `<p><br></p>` is shuffled too.
				if inner.chars().any(|c: char| !c.is_whitespace()) {
					served.push(Block::Paragraph(inner));
				}
			}
			// Ads (`div.cgo`, `div.csgo`) have no images and drop out here.
			Some("img" | "div") => images_in(&child, &mut served),
			_ => {}
		}
	}

	let paragraphs: Vec<String> = served
		.iter()
		.filter_map(|block: &Block| match block {
			Block::Paragraph(p) => Some(p.clone()),
			Block::Image(_) => None,
		})
		.collect();
	let mut restored = reorder::restore(paragraphs, chapter_id).into_iter();
	served
		.into_iter()
		.map(|block: Block| match block {
			Block::Paragraph(_) => Block::Paragraph(restored.next().unwrap_or_default()),
			image => image,
		})
		.collect()
}

/// A run of a chapter: Markdown text, or an illustration between two runs of text.
#[derive(Debug, PartialEq)]
pub enum Segment {
	Text(String),
	Image(String),
}

/// Appends one page of a chapter to `out`, joining text with the text before it. An
/// illustration already in the chapter is not added again.
pub fn append_page(html: &Document, chapter_id: u64, out: &mut Vec<Segment>) {
	for block in blocks(html, chapter_id) {
		match block {
			Block::Paragraph(inner) => {
				let text = inline_markdown(&inner);
				if text.is_empty() {
					continue;
				}
				match out.last_mut() {
					Some(Segment::Text(run)) => {
						run.push_str("\n\n");
						run.push_str(&text);
					}
					_ => out.push(Segment::Text(text)),
				}
			}
			Block::Image(src) => {
				if !out.iter().any(|s: &Segment| matches!(s, Segment::Image(seen) if *seen == src)) {
					out.push(Segment::Image(src));
				}
			}
		}
	}
}

/// The chapter as one Markdown text, each illustration written by `image`.
pub fn markdown(segments: &[Segment], mut image: impl FnMut(&str) -> String) -> String {
	let mut out = String::new();
	for segment in segments {
		if !out.is_empty() {
			out.push_str("\n\n");
		}
		match segment {
			Segment::Text(text) => out.push_str(text),
			Segment::Image(src) => out.push_str(&format!("![](<{}>)", image(src).replace('>', "%3E"))),
		}
	}
	out
}

/// Plain text for the details page's summary: `<br>` become line breaks.
pub fn plain_text(html: &str) -> String {
	render(html, false)
}

fn inline_markdown(html: &str) -> String {
	render(html, true)
}

/// Characters Markdown would treat as syntax at the start of or inside a paragraph.
fn escape(text: &str, out: &mut String) {
	for ch in text.chars() {
		if matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '#' | '<' | '>' | '|' | '~') {
			out.push('\\');
		}
		out.push(ch);
	}
}

fn decode_entities(text: &str) -> String {
	if !text.contains('&') {
		return String::from(text);
	}
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(pos) = rest.find('&') {
		out.push_str(&rest[..pos]);
		rest = &rest[pos..];
		// Entity names are short ASCII; look for the `;` within the next 12 bytes by
		// character, since a byte index could land inside a multibyte character.
		let end = rest
			.char_indices()
			.take_while(|(i, _): &(usize, char)| *i < 12)
			.find(|(_, c): &(usize, char)| *c == ';')
			.map(|(i, _): (usize, char)| i);
		let Some(end) = end else {
			out.push('&');
			rest = &rest[1..];
			continue;
		};
		let name = &rest[1..end];
		let decoded = match name {
			"amp" => Some('&'),
			"lt" => Some('<'),
			"gt" => Some('>'),
			"quot" => Some('"'),
			"apos" | "#039" => Some('\''),
			"nbsp" => Some(' '),
			_ if name.starts_with("#x") || name.starts_with("#X") => {
				u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32)
			}
			_ if name.starts_with('#') => name[1..].parse::<u32>().ok().and_then(char::from_u32),
			_ => None,
		};
		match decoded {
			Some(ch) => {
				out.push(ch);
				rest = &rest[end + 1..];
			}
			None => {
				out.push('&');
				rest = &rest[1..];
			}
		}
	}
	out.push_str(rest);
	out
}

/// Newlines inside HTML text are layout, not content. Between two CJK characters a
/// space would be wrong, so a break only becomes a space between ASCII words.
fn push_text(raw: &str, markdown: bool, out: &mut String) {
	let text = decode_entities(raw);
	let mut cleaned = String::with_capacity(text.len());
	let chars: Vec<char> = text.chars().collect();
	for (i, &ch) in chars.iter().enumerate() {
		if ch == '\n' || ch == '\r' || ch == '\t' {
			let prev = cleaned.chars().last();
			let next = chars[i + 1..].iter().find(|c: &&char| !c.is_whitespace());
			if prev.is_some_and(|c: char| c.is_ascii_alphanumeric())
				&& next.is_some_and(|c: &char| c.is_ascii_alphanumeric())
				&& prev != Some(' ')
			{
				cleaned.push(' ');
			}
		} else {
			cleaned.push(ch);
		}
	}
	if markdown {
		escape(&cleaned, out);
	} else {
		out.push_str(&cleaned);
	}
}

/// Inline HTML to text: `<br>` becomes a line break, `<rt>` a parenthesised reading,
/// `<script>`/`<style>`/`<rp>` are dropped and other tags are ignored.
fn render(html: &str, markdown: bool) -> String {
	let mut out = String::with_capacity(html.len());
	let mut rest = html;
	let mut skipping: Option<(String, usize)> = None;
	while !rest.is_empty() {
		let Some(lt) = rest.find('<') else {
			if skipping.is_none() {
				push_text(rest, markdown, &mut out);
			}
			break;
		};
		if lt > 0 && skipping.is_none() {
			push_text(&rest[..lt], markdown, &mut out);
		}
		rest = &rest[lt..];
		if rest.starts_with("<!--") {
			rest = rest.find("-->").map_or("", |end: usize| &rest[end + 3..]);
			continue;
		}
		let Some(gt) = rest.find('>') else {
			break;
		};
		let tag = &rest[1..gt];
		rest = &rest[gt + 1..];
		let closing = tag.starts_with('/');
		let name_part = tag.trim_start_matches('/');
		let name_end = name_part
			.find(|c: char| c.is_ascii_whitespace() || c == '/')
			.unwrap_or(name_part.len());
		let name = name_part[..name_end].to_ascii_lowercase();

		if let Some((skip_name, depth)) = skipping.as_mut() {
			if name == *skip_name {
				if closing {
					*depth -= 1;
					if *depth == 0 {
						skipping = None;
					}
				} else {
					*depth += 1;
				}
			}
			continue;
		}
		match (name.as_str(), closing) {
			("script" | "style" | "rp", false) if !tag.ends_with('/') => skipping = Some((name, 1)),
			("rt", false) => out.push('（'),
			("rt", true) => out.push('）'),
			("br", _) => {
				let trimmed = out.trim_end_matches(' ').len();
				out.truncate(trimmed);
				if !out.is_empty() && !out.ends_with('\n') {
					// A Markdown hard line break inside the paragraph.
					out.push_str(if markdown { "  \n" } else { "\n" });
				}
			}
			_ => {}
		}
	}
	String::from(out.trim())
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::html::Html;
	use aidoku_test::aidoku_test;

	fn doc(html: &str) -> Document {
		Html::parse_with_url(html, "https://tw.linovelib.com").expect("parse")
	}

	fn page(fixture: &str, chapter_id: u64) -> String {
		let mut segments = Vec::new();
		append_page(&doc(fixture), chapter_id, &mut segments);
		markdown(&segments, |src: &str| String::from(src))
	}

	fn paragraphs(markdown: &str) -> Vec<&str> {
		markdown.split("\n\n").filter(|p: &&str| !p.is_empty()).collect()
	}

	#[aidoku_test]
	fn restores_the_order_the_browser_shows() {
		// The browser's #acontent after its script ran (Playwright, 2026-10-04): 122
		// paragraphs, these at positions 20, 21, 50 and last.
		let out = page(include_str!("fixtures/chapter_403.html"), 403);
		let p = paragraphs(&out);
		assert_eq!(p.len(), 122);
		assert_eq!(p[0], "「你喔，連對烹飪課都有創傷嗎？」");
		assert_eq!(p[20], "老師打從心底感到無奈，嘴巴叼起一根香煙。");
		assert_eq!(p[21], "「話說回來，你會做菜喔？」");
		assert_eq!(p[50], "「……嗯，的確。」");
		assert_eq!(p[121], "我看起來溝通能力有那麼差嗎……");
	}

	#[aidoku_test]
	fn keeps_inline_images_in_place() {
		let out = page(include_str!("fixtures/chapter_403_2.html"), 403);
		let p = paragraphs(&out);
		// 118 paragraphs and the one illustration the browser shows.
		assert_eq!(p.len(), 119);
		assert_eq!(p[20], "「不過啊，會說『女人味』這種話，更代表你是個蕩婦。」");
		assert_eq!(p[50], "反觀雪之下，她的頭腦靈活又伶牙俐齒，胸前則像一塊洗衣板。");
		assert_eq!(p[118], "「就、就是說嘛，很奇怪呢～～」");
		assert!(p.contains(&"![](<https://img3.readpai.com/0/2/109088/199061.jpg>)"));
	}

	#[aidoku_test]
	fn simplified_site_uses_the_same_order() {
		let out = page(include_str!("fixtures/chapter_bili_403.html"), 403);
		let p = paragraphs(&out);
		assert_eq!(p.len(), 122);
		assert_eq!(p[20], "老师打从心底感到无奈，嘴巴叼起一根香烟。");
		assert_eq!(p[121], "我看起来沟通能力有那么差吗……");
	}

	#[aidoku_test]
	fn collects_every_illustration() {
		let mut segments = Vec::new();
		let html = doc(include_str!("fixtures/chapter_109088.html"));
		append_page(&html, 109088, &mut segments);
		// A second pass over the same page adds nothing: each illustration once.
		append_page(&html, 109088, &mut segments);
		let images: Vec<&String> = segments
			.iter()
			.filter_map(|s: &Segment| match s {
				Segment::Image(src) => Some(src),
				Segment::Text(_) => None,
			})
			.collect();
		// The six on the page and the eighteen hidden ones.
		assert_eq!(images.len(), 24);
		assert!(images.iter().all(|src: &&String| src.starts_with("https://img3.readpai.com/")));
	}

	#[aidoku_test]
	fn notices_withheld_paragraphs() {
		assert!(is_withheld(include_str!("fixtures/chapter_truncated.html")));
		assert!(!is_withheld(include_str!("fixtures/chapter_403.html")));
		assert!(!is_withheld(include_str!("fixtures/chapter_bili_403.html")));
	}

	#[aidoku_test]
	fn inline_html_to_text() {
		assert_eq!(inline_markdown("A&amp;B <ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp></ruby>*"), "A&B 漢（かん）\\*");
		assert_eq!(inline_markdown("一<br>二"), "一  \n二");
		assert_eq!(plain_text("「我的青春」<br />\n<br />\n高中生"), "「我的青春」\n高中生");
	}
}
