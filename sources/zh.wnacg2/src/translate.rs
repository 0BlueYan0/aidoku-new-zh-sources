//! The site's AI translation of Japanese albums (added 2026-11-01), drawn onto the pages.
//!
//! The site's reader (`themes/weitu/images/mtr.js`) fetches
//! `/index.php?ctl=photos&act=mtr_result&aid=<aid>` and lays the translated text over
//! each image in HTML. The app cannot overlay HTML on a page, so the source paints white
//! boxes and the text into the image itself in `process_page_image`.
//!
//! Only translations someone already ordered are shown. Ordering one (`mtr_request`)
//! spends the reader's site quota, and the user chose not to do that from the app
//! (2026-11-03).

use aidoku::{
	alloc::{String, Vec},
	imports::{
		canvas::{Canvas, Color, Font, FontWeight, ImageRef, Path, Point, Rect},
		defaults::defaults_get,
	},
	prelude::*,
	PageContext,
};

use crate::helper::{base_url, json_objects_in, json_string, json_unescape};

/// `settings.json` keys.
const ENABLED_KEY: &str = "translate";
const LANG_KEY: &str = "translateLang";

/// `PageContext` key carrying one page's boxes from `get_page_list` to the processor.
pub const CONTEXT_KEY: &str = "mtr";

/// Line height and vertical advance of the site's default (`sys`) font, in em
/// (`mtr.js` `METRIC.sys`: `lh` 1.15, `ls` -0.04).
const LINE_HEIGHT: f32 = 1.15;
const VERTICAL_ADVANCE: f32 = 0.96;
/// The site packs into 1 to 12 columns (`bestPack`).
const MAX_COLUMNS: usize = 12;
/// The site prefers the DeepSeek line breaks unless they shrink the text by more than
/// 15% compared with breaking at word boundaries.
const SEGMENT_PREFERENCE: f32 = 0.85;
/// Smallest font size, in pixels of the original image (the site's floor is 6 px).
const MIN_FONT: f32 = 6.0;
/// Largest font size as a share of the image width. The site caps text at 40 CSS px on
/// a page shown about 800 to 1300 px wide; this is the same proportion (my call).
const MAX_FONT_SHARE: f32 = 1.0 / 30.0;
/// Full-height ink above and below the baseline, see `draw_cell`.
const REFERENCE: &str = "田ｇ";
/// Marks that the site keeps off the start of a column (`mtr.js` `TRAIL`).
const TRAIL: &str = "，。、！？；：」』）…～〜—－♥♡★☆♪,.!?;:)";

/// `None` when the setting is off, otherwise whether Simplified Chinese was chosen.
pub fn enabled_language() -> Option<bool> {
	if !defaults_get::<bool>(ENABLED_KEY).unwrap_or(false) {
		return None;
	}
	Some(defaults_get::<String>(LANG_KEY).as_deref() == Some("sc"))
}

pub fn result_url(aid: &str) -> String {
	format!(
		"{}/index.php?ctl=photos&act=mtr_result&aid={aid}",
		base_url()
	)
}

/// One speech box. Coordinates are shares of the image size, as the site sends them.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
	pub x: f32,
	pub y: f32,
	pub w: f32,
	pub h: f32,
	pub vertical: bool,
	/// `#rrggbb` of the original lettering; near-black #111 when absent, as on the site.
	pub color: Option<String>,
	/// The site's own line breaks (`pt`/`ps`), or the whole sentence when it sent none.
	pub segments: Vec<String>,
	/// The sentence broken at word boundaries (`tk`/`tks`).
	pub tokens: Vec<String>,
}

/// Every translated page of an `mtr_result` answer as (page key, boxes). Boxes the site
/// marks `skip` (lettering it leaves in the original) and boxes without text are left out.
pub fn parse_result(json: &str, simplified: bool) -> Vec<(String, Vec<TextBox>)> {
	let (text_key, segment_key, token_key) = if simplified {
		("zs", "ps", "tks")
	} else {
		("zh", "pt", "tk")
	};
	let mut pages = Vec::new();
	for page in json_objects_in(json, "pages") {
		let Some(path) = json_string(page, "path") else {
			continue;
		};
		let mut boxes = Vec::new();
		for object in json_objects_in(page, "boxes") {
			if json_f32(object, "skip").unwrap_or(0.0) != 0.0 {
				continue;
			}
			let Some(text) = json_string(object, text_key).filter(|t| !t.trim().is_empty()) else {
				continue;
			};
			let (Some(x), Some(y), Some(w), Some(h)) = (
				json_f32(object, "x"),
				json_f32(object, "y"),
				json_f32(object, "w"),
				json_f32(object, "h"),
			) else {
				continue;
			};
			// Every box the site sent on 2026-11-03 lies inside the image. One that does not
			// is skipped: an off-image x would make `draw_cell` pad with millions of spaces.
			if !in_image(x, w) || !in_image(y, h) {
				continue;
			}
			let mut segments = json_string_array(object, segment_key);
			if segments.is_empty() {
				segments.push(text.clone());
			}
			let mut tokens = json_string_array(object, token_key);
			if tokens.is_empty() {
				tokens.push(text);
			}
			boxes.push(TextBox {
				x,
				y,
				w,
				h,
				vertical: json_f32(object, "v").unwrap_or(0.0) != 0.0,
				color: json_string(object, "c").filter(|c| parse_color(c).is_some()),
				segments: clean(segments),
				tokens: clean(tokens),
			});
		}
		if !boxes.is_empty() {
			pages.push((path, boxes));
		}
	}
	pages
}

/// `start` and `start + len` within 0 to 1, with a little slack for rounding.
fn in_image(start: f32, len: f32) -> bool {
	start.is_finite() && len.is_finite() && start >= 0.0 && len > 0.0 && start + len <= 1.01
}

/// The site matches a translation to an image by path, not page order (`mtr.js`
/// `pageKey`): what follows `/data/`, without a low-resolution `.w1280.webp` tail.
pub fn page_key(url: &str) -> Option<String> {
	let path = url.split(['?', '#']).next()?;
	let start = path.find("/data/")? + 1;
	let mut key = &path[start..];
	if let Some(stem) = key.strip_suffix(".webp") {
		if let Some(dot) = stem.rfind(".w") {
			let digits = &stem[dot + 2..];
			if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
				key = &stem[..dot];
			}
		}
	}
	Some(key.into())
}

// ---------------------------------------------------------------------------
// Passing boxes through PageContext
// ---------------------------------------------------------------------------

const BOX_SEPARATOR: char = '\n';
const FIELD_SEPARATOR: char = '\t';
const PART_SEPARATOR: char = '\u{1f}';

/// The separators are removed from the text when it is parsed (`clean`), so they cannot
/// appear inside a field.
pub fn encode(boxes: &[TextBox]) -> String {
	let mut out = String::new();
	for (i, b) in boxes.iter().enumerate() {
		if i > 0 {
			out.push(BOX_SEPARATOR);
		}
		out.push_str(&format!(
			"{}\t{}\t{}\t{}\t{}\t{}\t",
			b.x,
			b.y,
			b.w,
			b.h,
			u8::from(b.vertical),
			b.color.as_deref().unwrap_or("")
		));
		push_parts(&mut out, &b.segments);
		out.push(FIELD_SEPARATOR);
		push_parts(&mut out, &b.tokens);
	}
	out
}

fn push_parts(out: &mut String, parts: &[String]) {
	for (i, part) in parts.iter().enumerate() {
		if i > 0 {
			out.push(PART_SEPARATOR);
		}
		out.push_str(part);
	}
}

pub fn decode(text: &str) -> Vec<TextBox> {
	text.split(BOX_SEPARATOR)
		.filter_map(|line| {
			let mut fields = line.split(FIELD_SEPARATOR);
			let x = fields.next()?.parse().ok()?;
			let y = fields.next()?.parse().ok()?;
			let w = fields.next()?.parse().ok()?;
			let h = fields.next()?.parse().ok()?;
			let vertical = fields.next()? == "1";
			let color = Some(fields.next()?)
				.filter(|c| !c.is_empty())
				.map(String::from);
			let parts = |field: &str| -> Vec<String> {
				field
					.split(PART_SEPARATOR)
					.filter(|p| !p.is_empty())
					.map(String::from)
					.collect()
			};
			let segments = parts(fields.next()?);
			let tokens = parts(fields.next()?);
			(!segments.is_empty() && !tokens.is_empty()).then_some(TextBox {
				x,
				y,
				w,
				h,
				vertical,
				color,
				segments,
				tokens,
			})
		})
		.collect()
}

pub fn context_for(boxes: &[TextBox]) -> PageContext {
	let mut context = PageContext::new();
	context.insert(CONTEXT_KEY.into(), encode(boxes));
	context
}

// ---------------------------------------------------------------------------
// Layout, ported from mtr.js `pack` / `bestPack` / `layout`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Packed {
	pub font_size: f32,
	pub lines: Vec<String>,
}

fn char_len(text: &str) -> usize {
	text.chars().count()
}

/// Join `parts` into lines of at most `limit` characters. With `strict`, a part longer
/// than a line makes the packing fail; otherwise it is cut, never before a trailing mark.
fn pack(parts: &[String], limit: usize, strict: bool) -> Option<Vec<String>> {
	let mut lines: Vec<String> = Vec::new();
	let mut current = String::new();
	for part in parts {
		let mut part: Vec<char> = part.chars().collect();
		if part.len() > limit {
			if strict {
				return None;
			}
			if !current.is_empty() {
				lines.push(core::mem::take(&mut current));
			}
			while part.len() > limit {
				let mut cut = limit;
				while cut > 1 && TRAIL.contains(part[cut]) {
					cut -= 1;
				}
				lines.push(part[..cut].iter().collect());
				part.drain(..cut);
			}
		}
		if !current.is_empty() && char_len(&current) + part.len() > limit {
			lines.push(core::mem::take(&mut current));
		}
		current.extend(part);
	}
	if !current.is_empty() {
		lines.push(current);
	}
	Some(lines)
}

/// Try 1 to 12 lines; for each, the shortest line limit that fits, then keep the largest
/// font (a candidate must beat the previous best by 4%, so fewer lines win ties).
fn best_pack(
	parts: &[String],
	total: usize,
	along: f32,
	across: f32,
	advance: f32,
	strict: bool,
) -> Option<Packed> {
	let mut best: Option<Packed> = None;
	for n in 1..=MAX_COLUMNS.min(total.max(1)) {
		let (mut lo, mut hi) = (total.div_ceil(n).max(1), total.max(1));
		let mut found = None;
		while lo <= hi {
			let mid = (lo + hi) / 2;
			match pack(parts, mid, strict) {
				Some(lines) if lines.len() <= n => {
					found = Some(lines);
					if mid == 1 {
						break;
					}
					hi = mid - 1;
				}
				_ => lo = mid + 1,
			}
		}
		let Some(lines) = found else {
			continue;
		};
		let longest = lines.iter().map(|l| char_len(l)).max().unwrap_or(1).max(1);
		let size =
			(along / (longest as f32 * advance)).min(across / (lines.len() as f32 * LINE_HEIGHT));
		if best.as_ref().is_none_or(|b| size > b.font_size * 1.04) {
			best = Some(Packed {
				font_size: size,
				lines,
			});
		}
	}
	best
}

/// Lines and font size for a box `width` × `height` pixels.
pub fn layout(b: &TextBox, width: f32, height: f32, max_font: f32) -> Option<Packed> {
	let total = b.segments.iter().map(|s| char_len(s)).sum::<usize>();
	let (along, across, advance) = if b.vertical {
		(height, width, VERTICAL_ADVANCE)
	} else {
		(width, height, 1.0)
	};
	let segmented = best_pack(&b.segments, total, along, across, advance, true);
	let tokenized = best_pack(&b.tokens, total, along, across, advance, false)?;
	let mut best = match segmented {
		Some(s) if s.font_size >= tokenized.font_size * SEGMENT_PREFERENCE => s,
		_ => tokenized,
	};
	best.font_size = best.font_size.min(max_font).max(MIN_FONT);
	Some(best)
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub fn parse_color(text: &str) -> Option<Color> {
	let hex = text.strip_prefix('#')?;
	if hex.len() != 6 || !hex.is_ascii() {
		return None;
	}
	let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
	// The app takes colour channels as 0 to 1 (docs/aidoku-rs-api.md 12.0).
	Some(Color::new(
		f32::from(channel(0)?) / 255.0,
		f32::from(channel(2)?) / 255.0,
		f32::from(channel(4)?) / 255.0,
		1.0,
	))
}

/// Brackets, dashes and ellipses as vertical presentation forms, as in zh.lightnovel
/// `vertical.rs`: the site rotates these in CSS, the app's canvas cannot.
fn vertical_form(ch: char) -> char {
	match ch {
		'「' | '“' => '﹁',
		'」' | '”' => '﹂',
		'『' | '‘' => '﹃',
		'』' | '’' => '﹄',
		'（' | '(' => '︵',
		'）' | ')' => '︶',
		'《' => '︽',
		'》' => '︾',
		'〈' => '︿',
		'〉' => '﹀',
		'【' => '︻',
		'】' => '︼',
		'…' | '‥' => '︙',
		'—' | '―' | '─' | '－' | '-' | '～' | '〜' | '~' => '︱',
		_ => ch,
	}
}

/// Width of a horizontal line in em: ASCII is about half a full-width character.
fn horizontal_width(text: &str) -> f32 {
	text.chars()
		.map(|c| if c.is_ascii() { 0.55 } else { 1.0 })
		.sum()
}

struct Painter {
	canvas: Canvas,
	font: Font,
	width: f32,
}

impl Painter {
	/// Draw `text` with the top left of its first glyph at (x, y).
	///
	/// The app puts the baseline at y plus the ink height of the whole string, so a flat
	/// glyph such as 一 or ， would ride up. A full-height reference after the text fixes
	/// the ink height and ideographic spaces push it past the right edge of the image
	/// (the same fix as zh.lightnovel `Painter::draw_text`, docs/aidoku-rs-api.md 12.0).
	fn draw_cell(&mut self, text: &str, size: f32, x: f32, y: f32, color: &Color) {
		let pad = ((self.width - x).clamp(0.0, self.width) / size) as usize + 4;
		let mut line = String::with_capacity(text.len() + pad * 3 + 8);
		line.push_str(text);
		for _ in 0..pad {
			line.push('\u{3000}');
		}
		line.push_str(REFERENCE);
		self.canvas
			.draw_text(&line, size, &Point::new(x, y), &self.font, color);
	}

	fn draw_box(&mut self, b: &TextBox, image_w: f32, image_h: f32) {
		let (x, y, w, h) = (b.x * image_w, b.y * image_h, b.w * image_w, b.h * image_h);
		if w <= 0.0 || h <= 0.0 {
			return;
		}
		let inset = (image_w * 0.002).max(1.0);
		let Some(packed) = layout(
			b,
			w - 2.0 * inset,
			h - 2.0 * inset,
			image_w * MAX_FONT_SHARE,
		) else {
			// Leave the original lettering rather than a blank box.
			return;
		};
		self.canvas
			.fill(&Path::rect(&Rect::new(x, y, w, h)), &Color::white());
		let color = b
			.color
			.as_deref()
			.and_then(parse_color)
			.unwrap_or(Color::new(0.067, 0.067, 0.067, 1.0));
		let size = packed.font_size;
		let (cx, cy) = (x + w / 2.0, y + h / 2.0);
		let line_gap = size * LINE_HEIGHT;
		if b.vertical {
			// Columns right to left, each centred along its length, the block centred.
			let right = cx + packed.lines.len() as f32 * line_gap / 2.0;
			let advance = size * VERTICAL_ADVANCE;
			for (i, column) in packed.lines.iter().enumerate() {
				let centre = right - (i as f32 + 0.5) * line_gap;
				let top = cy - char_len(column) as f32 * advance / 2.0;
				for (j, ch) in column.chars().map(vertical_form).enumerate() {
					let offset = if ch.is_ascii() { size * 0.22 } else { 0.0 };
					let mut cell = [0u8; 4];
					self.draw_cell(
						ch.encode_utf8(&mut cell),
						size,
						centre - size / 2.0 + offset,
						top + j as f32 * advance,
						&color,
					);
				}
			}
		} else {
			let top = cy - packed.lines.len() as f32 * line_gap / 2.0 + (line_gap - size) / 2.0;
			for (i, line) in packed.lines.iter().enumerate() {
				let left = cx - horizontal_width(line) * size / 2.0;
				self.draw_cell(line, size, left, top + i as f32 * line_gap, &color);
			}
		}
	}
}

/// The page with its translated boxes painted over, or `None` when the context carries
/// none, so the caller hands back the original image.
pub fn paint(image: &ImageRef, context: Option<&PageContext>) -> Option<ImageRef> {
	let boxes = decode(context?.get(CONTEXT_KEY)?);
	if boxes.is_empty() {
		return None;
	}
	let (w, h) = (image.width(), image.height());
	if w <= 0.0 || h <= 0.0 {
		return None;
	}
	let mut painter = Painter {
		canvas: Canvas::new(w, h),
		font: Font::system(FontWeight::Semibold),
		width: w,
	};
	painter.canvas.draw_image(image, Rect::new(0.0, 0.0, w, h));
	for b in &boxes {
		painter.draw_box(b, w, h);
	}
	// The canvas calls report no errors, so an empty result is the only failure visible.
	Some(painter.canvas.get_image()).filter(|painted| painted.width() > 0.0)
}

// ---------------------------------------------------------------------------
// JSON helpers for the box objects
// ---------------------------------------------------------------------------

/// `"key":0.3888` → 0.3888. The key must not also appear in an earlier nested object.
fn json_f32(json: &str, key: &str) -> Option<f32> {
	let marker = format!("\"{key}\":");
	let rest = json[json.find(&marker)? + marker.len()..].trim_start();
	let end = rest
		.char_indices()
		.find(|&(_, ch)| !(ch.is_ascii_digit() || matches!(ch, '-' | '.' | 'e' | 'E' | '+')))
		.map_or(rest.len(), |(i, _)| i);
	rest[..end].parse().ok()
}

/// `"key":["a","b"]` → the unescaped strings.
fn json_string_array(json: &str, key: &str) -> Vec<String> {
	let mut out = Vec::new();
	let marker = format!("\"{key}\":[");
	let Some(at) = json.find(&marker) else {
		return out;
	};
	let body = &json[at + marker.len()..];
	let (mut start, mut in_string, mut escaped) = (None, false, false);
	for (i, ch) in body.char_indices() {
		if in_string {
			if escaped {
				escaped = false;
			} else if ch == '\\' {
				escaped = true;
			} else if ch == '"' {
				in_string = false;
				if let Some(s) = start.take() {
					out.push(json_unescape(&body[s..i]));
				}
			}
			continue;
		}
		match ch {
			'"' => {
				in_string = true;
				start = Some(i + 1);
			}
			']' => break,
			_ => {}
		}
	}
	out
}

/// Drop the `encode` separators and empty parts.
fn clean(parts: Vec<String>) -> Vec<String> {
	parts
		.into_iter()
		.map(|p| {
			p.chars()
				.filter(|&c| {
					c != BOX_SEPARATOR && c != FIELD_SEPARATOR && c != PART_SEPARATOR && c != '\r'
				})
				.collect::<String>()
		})
		.filter(|p| !p.trim().is_empty())
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku::alloc::vec;
	use aidoku_test::aidoku_test;

	fn strings(parts: &[&str]) -> Vec<String> {
		parts.iter().map(|&p| String::from(p)).collect()
	}

	#[aidoku_test]
	fn page_key_matches_the_site_rule() {
		assert_eq!(
			page_key("https://img5.qy0.ru/data/4011/11/0049.webp?verify=1793664000-abc").as_deref(),
			Some("data/4011/11/0049.webp")
		);
		assert_eq!(
			page_key("https://img5.qy0.ru/data/4011/11/0049.webp.w1280.webp").as_deref(),
			Some("data/4011/11/0049.webp")
		);
		assert_eq!(page_key("https://img5.qy0.ru/themes/x.png"), None);
	}

	#[aidoku_test]
	fn context_round_trips() {
		let boxes = vec![
			TextBox {
				x: 0.3888,
				y: 0.2175,
				w: 0.0744,
				h: 0.141,
				vertical: true,
				color: None,
				segments: strings(&["雖然是", "對胸部"]),
				tokens: strings(&["雖然", "是", "對", "胸部"]),
			},
			TextBox {
				x: 0.7,
				y: 0.88,
				w: 0.27,
				h: 0.08,
				vertical: false,
				color: Some(String::from("#fe1c7e")),
				segments: strings(&["(新繪21P)學姊"]),
				tokens: strings(&["(新繪", "21P)", "學姊"]),
			},
		];
		assert_eq!(decode(&encode(&boxes)), boxes);
	}

	#[aidoku_test]
	fn strict_pack_refuses_a_long_segment() {
		assert_eq!(pack(&strings(&["一二三四"]), 3, true), None);
		assert_eq!(
			pack(&strings(&["一二三四"]), 3, false),
			Some(strings(&["一二三", "四"]))
		);
	}

	#[aidoku_test]
	fn a_cut_never_starts_a_line_with_a_trailing_mark() {
		assert_eq!(
			pack(&strings(&["一二三。"]), 3, false),
			Some(strings(&["一二", "三。"]))
		);
	}

	#[aidoku_test]
	fn layout_fits_the_box() {
		let b = TextBox {
			x: 0.0,
			y: 0.0,
			w: 0.0,
			h: 0.0,
			vertical: true,
			color: None,
			segments: strings(&["虽然是", "对胸部", "很没抵抗力的", "雷特同学…"]),
			tokens: strings(&[
				"虽然",
				"是",
				"对",
				"胸部",
				"很",
				"没",
				"抵抗力",
				"的",
				"雷特",
				"同学…",
			]),
		};
		let (w, h) = (190.0, 510.0);
		let packed = layout(&b, w, h, 1000.0).expect("layout");
		let longest = packed.lines.iter().map(|l| char_len(l)).max().unwrap();
		assert!(longest as f32 * packed.font_size * VERTICAL_ADVANCE <= h + 0.01);
		assert!(packed.lines.len() as f32 * packed.font_size * LINE_HEIGHT <= w + 0.01);
		assert_eq!(packed.lines.concat(), "虽然是对胸部很没抵抗力的雷特同学…");
	}

	#[aidoku_test]
	fn colours_are_zero_to_one() {
		let c = parse_color("#ff0080").expect("colour");
		assert_eq!((c.red, c.green, c.alpha), (1.0, 0.0, 1.0));
		assert!(parse_color("red").is_none());
	}

	#[aidoku_test]
	fn boxes_outside_the_image_are_skipped() {
		let json = concat!(
			r#"{"pages":[{"path":"data/1.webp","boxes":["#,
			r#"{"x":-50,"y":0.1,"w":0.1,"h":0.1,"v":1,"zh":"甲"},"#,
			r#"{"x":0.1,"y":0.1,"w":1e999,"h":0.1,"v":1,"zh":"乙"},"#,
			r#"{"x":0.95,"y":0.1,"w":0.1,"h":0.1,"v":1,"zh":"丙"},"#,
			r#"{"x":0.1,"y":0.1,"w":0.1,"h":0.1,"v":1,"zh":"丁"}]}]}"#
		);
		let pages = parse_result(json, false);
		assert_eq!(pages.len(), 1);
		assert_eq!(pages[0].1.len(), 1);
		assert_eq!(pages[0].1[0].segments, ["丁"]);
	}

	#[aidoku_test]
	fn result_without_pages_is_empty() {
		assert!(parse_result("{\"aid\":1,\"pages\":[]}", false).is_empty());
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/translate.rs"]
mod fixture_tests;
