//! Chapter HTML to the Markdown of one text page.
//!
//! A chapter becomes a single `PageContent::Text` with its illustrations inline as
//! Markdown images. On the device (2026-09-29) that rendered the images and scrolled
//! cleanly under `Viewer::Vertical`; splitting text and images into separate pages
//! made the text jump and overlap while scrolling.
//!
//! The HTML ranges from `<p>` paragraphs to Word exports (`<p class=MsoNormal>` with
//! nested `<span>`s and `<o:p>`). Only structure matters here: block ends become
//! paragraph breaks, `<img>` becomes an image, `<rt>` becomes a parenthesised reading,
//! and everything else is dropped. Text goes through the font map before escaping.

use aidoku::alloc::{format, String};

use crate::font::FontMap;

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
		let Some(end) = rest[..rest.len().min(12)].find(';') else {
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
			"apos" => Some('\''),
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

/// Value of `attr` in a tag's inner text (`img src="..." alt=...`).
fn attribute(tag: &str, attr: &str) -> Option<String> {
	let lower = tag.to_ascii_lowercase();
	let mut from = 0;
	while let Some(found) = lower[from..].find(attr) {
		let at = from + found;
		from = at + attr.len();
		let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
		let after = lower[from..].trim_start();
		if !before_ok || !after.starts_with('=') {
			continue;
		}
		let value_start = tag.len() - after.len() + 1;
		let value = tag[value_start..].trim_start();
		let (quoted, body) = match value.chars().next() {
			Some(q @ ('"' | '\'')) => (Some(q), &value[1..]),
			_ => (None, value),
		};
		let end = match quoted {
			Some(q) => body.find(q).unwrap_or(body.len()),
			None => body.find(|c: char| c.is_ascii_whitespace() || c == '>').unwrap_or(body.len()),
		};
		return Some(decode_entities(&body[..end]));
	}
	None
}

/// Newlines inside HTML text are layout, not content. Between two CJK characters a
/// space would be wrong, so a break only becomes a space between ASCII words.
fn push_text(raw: &str, map: Option<&FontMap>, markdown: bool, out: &mut String) {
	let text = decode_entities(raw);
	let text = match map {
		Some(map) => map.decode(&text),
		None => text,
	};
	let mut cleaned = String::with_capacity(text.len());
	let chars: aidoku::alloc::Vec<char> = text.chars().collect();
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

fn paragraph_break(out: &mut String) {
	let trimmed = out.trim_end_matches([' ', '\u{3000}']).len();
	out.truncate(trimmed);
	if !out.is_empty() && !out.ends_with("\n\n") {
		if out.ends_with('\n') {
			out.push('\n');
		} else {
			out.push_str("\n\n");
		}
	}
}

/// Markdown for one chapter. `map` undoes the font obfuscation; `None` leaves the
/// text as it came.
pub fn chapter_markdown(html: &str, map: Option<&FontMap>) -> String {
	render(html, map, true)
}

/// `markdown` false gives plain text: no escaping, no heading marks.
fn render(html: &str, map: Option<&FontMap>, markdown: bool) -> String {
	let mut out = String::with_capacity(html.len() / 2);
	let mut rest = html;
	// Element being skipped and how deep inside it the scan is.
	let mut skipping: Option<(String, usize)> = None;
	let mut footnote_link = false;
	while !rest.is_empty() {
		let Some(lt) = rest.find('<') else {
			if skipping.is_none() {
				push_text(rest, map, markdown, &mut out);
			}
			break;
		};
		if lt > 0 && skipping.is_none() {
			push_text(&rest[..lt], map, markdown, &mut out);
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
		let self_closing = tag.ends_with('/');
		let name_part = tag.trim_start_matches('/');
		let name_end = name_part
			.find(|c: char| c.is_ascii_whitespace() || c == '/')
			.unwrap_or(name_part.len());
		let name = name_part[..name_end].to_ascii_lowercase();
		let void = self_closing || matches!(name.as_str(), "img" | "image" | "br" | "hr" | "meta" | "link" | "input");

		if let Some((skip_name, depth)) = skipping.as_mut() {
			if name == *skip_name && !void {
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
		if !closing && is_hidden(&name, tag) {
			if !void {
				skipping = Some((name, 1));
			}
			continue;
		}
		let class = if closing { None } else { attribute(tag, "class") };
		match (name.as_str(), closing) {
			("a", false) => footnote_link = class.as_deref().is_some_and(|c: &str| c.contains("duokan-footnote")),
			("a", true) => footnote_link = false,
			("img" | "image", false) => {
				let src = attribute(tag, "src").or_else(|| attribute(tag, "xlink:href"));
				let Some(src) = src.filter(|s: &String| s.starts_with("http") && markdown) else {
					continue;
				};
				// Footnote markers are a small note.png inside the reference link.
				if footnote_link || src.split(['?', '#']).next().is_some_and(|p: &str| p.ends_with("/note.png")) {
					out.push_str("〔註〕");
					continue;
				}
				paragraph_break(&mut out);
				out.push_str(&format!("![](<{}>)", image_url(&src).replace('>', "%3E")));
				out.push_str("\n\n");
			}
			("rt", false) => out.push('（'),
			("rt", true) => out.push('）'),
			("br", _) => {
				let trimmed = out.trim_end_matches(' ').len();
				out.truncate(trimmed);
				if !out.is_empty() && !out.ends_with('\n') {
					out.push('\n');
				}
			}
			("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
				paragraph_break(&mut out);
				if markdown {
					out.push_str("### ");
				}
			}
			("p" | "div", false) if class.as_deref().is_some_and(is_heading_class) => {
				paragraph_break(&mut out);
				if markdown {
					out.push_str("### ");
				}
			}
			("p" | "div" | "li" | "blockquote" | "tr" | "hr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
			| "section" | "article" | "aside", _) => paragraph_break(&mut out),
			_ => {}
		}
	}
	// A heading opened on an empty block would be left dangling.
	let result = out.replace("### \n", "\n");
	String::from(result.trim())
}

/// `<style>`, `<script>`, ruby fallbacks and anything the site hides.
fn is_hidden(name: &str, tag: &str) -> bool {
	if matches!(name, "style" | "script" | "rp" | "head" | "title") {
		return true;
	}
	if attribute(tag, "hidden").is_some() || tag.split_ascii_whitespace().any(|w: &str| w == "hidden") {
		return true;
	}
	attribute(tag, "style").is_some_and(|style: String| {
		let compact: String = style.chars().filter(|c: &char| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
		compact.contains("display:none") || compact.contains("visibility:hidden")
	})
}

/// Heading classes of the site's EPUB imports: `pius1`, `pius2`, `ph4`.
fn is_heading_class(class: &str) -> bool {
	class.split_ascii_whitespace().any(|c: &str| matches!(c, "pius1" | "pius2" | "ph4"))
}

/// Old covers and images carry an unencoded BlurHash in `placeholder`, which may
/// contain `#`. Anything after it would be taken as a fragment and the `t` signature
/// never sent, so the image host refuses the request.
pub fn image_url(url: &str) -> String {
	url.replace('#', "%23")
}

/// Plain text for a book's `introduction` HTML.
pub fn plain_text(html: &str) -> String {
	render(html, None, false)
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn converts_word_export_and_images() {
		let html = concat!(
			"<div class=\"main\"> <div class=\"illus duokan-image-single\">",
			"<img alt=\"p001\" src=\"https://img.lightnovel.life/images/p001.jpg?placeholder=J%5b&amp;size=1x2&amp;t=ab\">",
			"</div> </div>",
			"<p class=\"MsoNormal\" style=\"text-indent:24.0pt;\nline-height:125%\"><span style=\"font-size:12.0pt\">",
			"第一行*強調*<span lang=\"EN-US\"><o:p></o:p></span></span></p>",
			"<p><br></p><p>漢<ruby>字<rp>(</rp><rt>じ</rt><rp>)</rp></ruby>&nbsp;Hello\nworld",
			"<a class=\"duokan-footnote\" href=\"#n1\"><img src=\"https://img.lightnovel.life/note.png\"></a></p>",
			"<div style=\"display: none\"><div>藏起來</div>還是藏</div><p class=\"pius1\">第一章</p>",
			"<img src=\"https://img.lightnovel.life/a.jpg?placeholder=J#x&t=cd\">",
		);
		let md = chapter_markdown(html, None);
		assert_eq!(
			md,
			concat!(
				"![](<https://img.lightnovel.life/images/p001.jpg?placeholder=J%5b&size=1x2&t=ab>)\n\n",
				"第一行\\*強調\\*\n\n漢字（じ） Hello world〔註〕\n\n### 第一章\n\n",
				"![](<https://img.lightnovel.life/a.jpg?placeholder=J%23x&t=cd>)",
			)
		);
	}

	#[aidoku_test]
	fn introduction_to_plain_text() {
		let html = "<p>关于她们的命运与抗争的故事。</p><p>——“我并不是英雄”</p>";
		assert_eq!(plain_text(html), "关于她们的命运与抗争的故事。\n\n——“我并不是英雄”");
		// Text the author wrote keeps its backslashes and asterisks.
		let html = "<p class=\"pius1\">C:\\書*名*</p><img src=\"https://img.lightnovel.life/a.jpg\">";
		assert_eq!(plain_text(html), "C:\\書*名*");
	}
}
