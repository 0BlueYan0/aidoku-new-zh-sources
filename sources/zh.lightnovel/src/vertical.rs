//! Chapter text drawn as vertical (top to bottom, right to left) page images.
//!
//! The app's text pages are Markdown and cannot be set vertically, so with the
//! `layout` setting on vertical the source draws every character itself with the
//! canvas. The placement rules below come from the app's own drawing code
//! (AidokuRunner `Canvas.swift`, see docs/aidoku-rs-api.md section 12.0) and were
//! checked on the device against a cell grid on 2026-09-30.

use aidoku::{
	alloc::{string::ToString, String, Vec},
	imports::canvas::{Canvas, Color, Font, FontWeight, Path, Point, Rect},
	Page, PageContent,
};

const WIDTH: f32 = 1125.0;
const HEIGHT: f32 = 2436.0;
const MARGIN_X: f32 = 70.0;
const MARGIN_TOP: f32 = 130.0;
const MARGIN_BOTTOM: f32 = 130.0;
/// Full-height ink above and below the baseline; see `Painter::draw_text`.
const REFERENCE: &str = "田ｇ";
/// Two digits side by side in one cell are drawn at this fraction of the size.
const PAIR_SCALE: f32 = 0.8;

/// Character size for the `verticalSize` setting. Medium gives 11 columns of 42
/// characters a page; small and large differ from it by about a fifth.
pub fn size_for(setting: Option<&str>) -> f32 {
	match setting {
		Some("small") => 42.0,
		Some("large") => 64.0,
		_ => 52.0,
	}
}

enum Block {
	Text(String),
	Image(String),
}

/// Paragraphs and images of the Markdown `content::chapter_markdown` produced.
fn blocks(markdown: &str) -> Vec<Block> {
	let mut out = Vec::new();
	for para in markdown.split("\n\n") {
		let para = para.trim();
		if para.is_empty() {
			continue;
		}
		if let Some(rest) = para.strip_prefix("![](<") {
			out.push(Block::Image(rest.trim_end_matches(">)").to_string()));
			continue;
		}
		let para = para.strip_prefix("### ").unwrap_or(para);
		// Undo the Markdown escaping.
		let mut text = String::with_capacity(para.len());
		let mut chars = para.chars();
		while let Some(ch) = chars.next() {
			if ch == '\\' {
				if let Some(next) = chars.next() {
					text.push(next);
				}
			} else {
				text.push(ch);
			}
		}
		for line in text.split('\n') {
			out.push(Block::Text(line.to_string()));
		}
	}
	out
}

/// Brackets, dashes and ellipses as their vertical presentation forms. Turning the
/// horizontal glyph with `set_transform` put it in the wrong cell on the device, and the
/// forms need no transform. Curly quotes become corner brackets, as vertical Chinese
/// quotes. Commas and full stops stay: Traditional Chinese fonts centre them already.
fn normalize(ch: char) -> char {
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
		'…' => '︙',
		'—' | '―' | '─' => '︱',
		_ => ch,
	}
}

/// Marks that must not start a column: they hang below the last row instead.
fn no_column_start(cell: &str) -> bool {
	let mut chars = cell.chars();
	let (Some(ch), None) = (chars.next(), chars.next()) else {
		return false;
	};
	matches!(ch, '，' | '。' | '、' | '：' | '；' | '！' | '？' | '﹂' | '﹄' | '︶' | '︾' | '﹀' | '︼' | ',' | '.')
}

/// The text of each cell: one character, or a run of one or two ASCII digits set
/// side by side in a single cell. Longer numbers take a cell per digit.
fn cells(text: &str) -> Vec<String> {
	let chars: Vec<char> = text
		.chars()
		.map(normalize)
		.filter(|c: &char| !c.is_whitespace() || *c == '\u{3000}')
		.collect();
	let mut out = Vec::with_capacity(chars.len());
	let mut i = 0;
	while i < chars.len() {
		let run = chars[i..].iter().take_while(|c: &&char| c.is_ascii_digit()).count();
		if run == 2 {
			out.push(chars[i..i + 2].iter().collect());
			i += 2;
		} else if run > 2 {
			// A longer number keeps a cell per digit; its tail must not pair up.
			out.extend(chars[i..i + run].iter().map(|c: &char| c.to_string()));
			i += run;
		} else {
			out.push(chars[i].to_string());
			i += 1;
		}
	}
	out
}

struct Painter {
	pages: Vec<Page>,
	canvas: Option<Canvas>,
	column: usize,
	row: f32,
	font: Font,
	size: f32,
	column_width: f32,
	columns: usize,
	rows: f32,
}

impl Painter {
	fn new(size: f32) -> Self {
		let column_width = size * 1.75;
		Self {
			pages: Vec::new(),
			canvas: None,
			column: 0,
			row: 0.0,
			font: Font::system(FontWeight::Regular),
			size,
			column_width,
			columns: ((WIDTH - 2.0 * MARGIN_X) / column_width) as usize,
			rows: ((HEIGHT - MARGIN_TOP - MARGIN_BOTTOM) / size) as u32 as f32,
		}
	}

	fn canvas(&mut self) -> &mut Canvas {
		self.canvas.get_or_insert_with(|| {
			let mut canvas = Canvas::new(WIDTH, HEIGHT);
			canvas.fill(&Path::rect(&Rect::new(0.0, 0.0, WIDTH, HEIGHT)), &Color::white());
			canvas
		})
	}

	fn flush(&mut self) {
		if let Some(canvas) = self.canvas.take() {
			self.pages.push(Page {
				content: PageContent::image(canvas.get_image()),
				..Default::default()
			});
		}
		self.column = 0;
		self.row = 0.0;
	}

	fn next_column(&mut self) {
		self.column += 1;
		self.row = 0.0;
		if self.column >= self.columns {
			self.flush();
		}
	}

	/// Draw `cell` with the top left of its first glyph at (x, y).
	///
	/// The app puts the baseline at y plus the ink height of the whole string
	/// (`CTLineGetImageBounds`), so a flat glyph such as 一 or ， rode up into the cell
	/// above. A full-height reference after the text fixes the ink height, and
	/// ideographic spaces push it off the right edge. The text comes first so its
	/// position does not depend on the spaces' width: they measured about 0.95 em,
	/// which shifted the right columns 72 px when the reference came first.
	fn draw_text(&mut self, cell: &str, size: f32, x: f32, y: f32) {
		let pad = ((WIDTH - x) / size) as usize + 4;
		let mut text = String::with_capacity(cell.len() + pad * 3 + 8);
		text.push_str(cell);
		for _ in 0..pad {
			text.push('\u{3000}');
		}
		text.push_str(REFERENCE);
		self.canvas();
		if let Some(canvas) = self.canvas.as_mut() {
			canvas.draw_text(&text, size, &Point::new(x, y), &self.font, &Color::black());
		}
	}

	fn draw(&mut self, cell: &str, hang: bool) {
		if !hang && self.row + 1.0 > self.rows {
			self.next_column();
		}
		let s = self.size;
		// Top left of the cell.
		let cell_x = WIDTH - MARGIN_X - (self.column as f32 + 1.0) * self.column_width + (self.column_width - s) / 2.0;
		let cell_y = MARGIN_TOP + self.row * s;
		let count = cell.chars().count();
		if count == 2 {
			// Two digits at PAIR_SCALE are about 0.9 em wide together.
			let small = s * PAIR_SCALE;
			let offset = (s - small) / 2.0;
			self.draw_text(cell, small, cell_x + s * 0.05, cell_y + offset);
		} else if cell.is_ascii() {
			self.draw_text(cell, s, cell_x + s * 0.25, cell_y);
		} else {
			self.draw_text(cell, s, cell_x, cell_y);
		}
		self.row += 1.0;
	}

	fn paragraph(&mut self, text: &str) {
		let cells = cells(text);
		if cells.is_empty() {
			return;
		}
		if self.row > 0.0 {
			self.next_column();
		}
		for (i, cell) in cells.iter().enumerate() {
			// Closing marks that would start a new column stay on this one, below the last row.
			let hang = self.row >= self.rows && no_column_start(cell) && i > 0;
			self.draw(cell, hang);
		}
	}

	fn image(&mut self, url: String) {
		if self.canvas.is_some() {
			self.flush();
		}
		self.pages.push(Page {
			content: PageContent::url(url),
			..Default::default()
		});
	}
}

/// Page images for a chapter's Markdown, illustrations as pages of their own.
pub fn pages(markdown: &str, size: f32) -> Vec<Page> {
	let mut painter = Painter::new(size);
	for block in blocks(markdown) {
		match block {
			Block::Text(text) => painter.paragraph(&text),
			Block::Image(url) => painter.image(url),
		}
	}
	painter.flush();
	painter.pages
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn pairs_short_numbers_only() {
		assert_eq!(cells("12月"), ["12", "月"]);
		assert_eq!(cells("第5章"), ["第", "5", "章"]);
		assert_eq!(cells("5451年"), ["5", "4", "5", "1", "年"]);
		assert_eq!(cells("「好」"), ["﹁", "好", "﹂"]);
	}

	#[aidoku_test]
	fn splits_markdown_into_blocks() {
		let md = "### 序章\n\n![](<https://img.lightnovel.life/a.jpg?t=1>)\n\n第一行\\*\n第二行";
		let blocks = blocks(md);
		assert_eq!(blocks.len(), 4);
		assert!(matches!(&blocks[1], Block::Image(url) if url == "https://img.lightnovel.life/a.jpg?t=1"));
		assert!(matches!(&blocks[2], Block::Text(t) if t == "第一行*"));
	}
}
