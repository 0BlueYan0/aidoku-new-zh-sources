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

/// What a chapter the site keeps from readers shows instead of its text: the catalogue
/// marks it with `❤️`, and the page says it failed review or needs more permission
/// (`不相容的異種族妻子們` 第241話, 2026-10-20).
const LOCKED: [&str; 2] = ["沒有可閱讀的章節", "没有可阅读的章节"];

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

/// Whether the page is the site's notice for a chapter nobody can read.
pub fn is_locked(html: &str) -> bool {
	LOCKED.iter().any(|marker: &&str| html.contains(marker))
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

	// The paragraphs are restored on their own; the images keep their places, marked by
	// `Some` in `layout`.
	let mut paragraphs: Vec<String> = Vec::new();
	let mut layout: Vec<Option<String>> = Vec::new();
	for block in served {
		match block {
			Block::Paragraph(p) => {
				paragraphs.push(p);
				layout.push(None);
			}
			Block::Image(src) => layout.push(Some(src)),
		}
	}
	let mut restored = reorder::restore(paragraphs, chapter_id).into_iter();
	layout
		.into_iter()
		.map(|slot: Option<String>| match slot {
			Some(src) => Block::Image(src),
			None => Block::Paragraph(restored.next().unwrap_or_default()),
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
				if !out
					.iter()
					.any(|s: &Segment| matches!(s, Segment::Image(seen) if *seen == src))
				{
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
			Segment::Image(src) => {
				out.push_str(&format!("![](<{}>)", image(src).replace('>', "%3E")))
			}
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
		if matches!(
			ch,
			'\\' | '`' | '*' | '_' | '[' | ']' | '#' | '<' | '>' | '|' | '~'
		) {
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
				u32::from_str_radix(&name[2..], 16)
					.ok()
					.and_then(char::from_u32)
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
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn inline_html_to_text() {
		assert_eq!(
			inline_markdown("A&amp;B <ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp></ruby>*"),
			"A&B 漢（かん）\\*"
		);
		assert_eq!(inline_markdown("一<br>二"), "一  \n二");
		assert_eq!(
			plain_text("「我的青春」<br />\n<br />\n高中生"),
			"「我的青春」\n高中生"
		);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/content.rs"]
mod fixture_tests;
