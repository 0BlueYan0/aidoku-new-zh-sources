//! Undo the site's font obfuscation of chapter text.
//!
//! `GetNovelContent` replaces about 3500 common characters with other code points of
//! the same pool and returns a `Font` whose glyph at each fake code point draws the
//! real character. Fonts differ per book and change over time, but every one permutes
//! the same base glyphs and only jitters their coordinates by a few units, so the
//! contour layout (end point indices) identifies the base glyph. `glyph_table.rs`,
//! built by `tools/build_glyph_table.py`, maps each layout to its real character; for
//! the few layouts several characters share, reference points break the tie.
//!
//! The font is fetched as `.ttf` (the same path serves it uncompressed), so no WOFF2
//! decoder is needed. Only `cmap` format 4, `head`, `loca` and `glyf` are read.

use aidoku::{
	alloc::{format, String, Vec},
	imports::{
		defaults::{defaults_get, defaults_set, DefaultValue},
		net::Request,
	},
	prelude::*,
	Result,
};

use crate::glyph_table::{POOL, REFS, SIGS};

const CACHE_IDS_KEY: &str = "font_cache_ids";
const CACHE_MAP_PREFIX: &str = "font_cache_map_";
/// Fonts whose maps are kept; each map is about 7000 characters.
const CACHE_FONTS: usize = 3;
const REF_POINTS: usize = 16;

/// Fake code point -> real character, sorted by the fake one.
pub struct FontMap(Vec<(u16, u16)>);

impl FontMap {
	pub fn decode(&self, text: &str) -> String {
		text.chars()
			.map(|ch: char| {
				let cp = ch as u32;
				if cp > 0xFFFF {
					return ch;
				}
				match self.0.binary_search_by_key(&(cp as u16), |(fake, _)| *fake) {
					Ok(i) => char::from_u32(u32::from(self.0[i].1)).unwrap_or(ch),
					Err(_) => ch,
				}
			})
			.collect()
	}

	fn to_stored(&self) -> String {
		let mut out = String::with_capacity(self.0.len() * 2);
		for (fake, real) in &self.0 {
			out.push(char::from_u32(u32::from(*fake)).unwrap_or('\u{fffd}'));
			out.push(char::from_u32(u32::from(*real)).unwrap_or('\u{fffd}'));
		}
		out
	}

	fn from_stored(stored: &str) -> Option<Self> {
		let chars: Vec<char> = stored.chars().collect();
		if chars.is_empty() || !chars.len().is_multiple_of(2) {
			return None;
		}
		let pairs = chars.chunks(2).map(|p: &[char]| (p[0] as u16, p[1] as u16)).collect();
		Some(Self(pairs))
	}
}

/// The map for `font_path`, the chapter's `Font` field: `/font/<hash>.woff2`, or a full
/// URL. The site gives each book its own font (it also changed for one book between
/// sessions on 2026-09-29), so the maps of the last few fonts stay in defaults and the
/// chapters of a book download the font once.
pub fn map_for(font_path: &str) -> Result<FontMap> {
	let file = font_path.split(['?', '#']).next().unwrap_or("");
	let id = file.rsplit('/').next().unwrap_or("").split('.').next().unwrap_or("");
	if id.is_empty() || !id.bytes().all(|b: u8| b.is_ascii_alphanumeric()) {
		bail!("[lightnovel] unexpected font path {font_path}");
	}
	let mut ids: Vec<String> = defaults_get::<String>(CACHE_IDS_KEY)
		.unwrap_or_default()
		.split(',')
		.filter(|s: &&str| !s.is_empty())
		.map(String::from)
		.collect();
	if ids.iter().any(|cached: &String| cached == id) {
		if let Some(map) = defaults_get::<String>(&format!("{CACHE_MAP_PREFIX}{id}")).and_then(|s: String| FontMap::from_stored(&s)) {
			return Ok(map);
		}
	}
	// The same hash is served uncompressed as .ttf, which spares a WOFF2 decoder.
	let data = Request::get(format!("{}/font/{id}.ttf", crate::hub::api_base()))?.timeout(60.0).data()?;
	let map = build_map(&data)?;
	ids.retain(|cached: &String| cached != id);
	while ids.len() >= CACHE_FONTS {
		let evicted = ids.remove(0);
		defaults_set(&format!("{CACHE_MAP_PREFIX}{evicted}"), DefaultValue::Null);
	}
	defaults_set(&format!("{CACHE_MAP_PREFIX}{id}"), DefaultValue::String(map.to_stored()));
	ids.push(String::from(id));
	defaults_set(CACHE_IDS_KEY, DefaultValue::String(ids.join(",")));
	Ok(map)
}

pub fn build_map(ttf: &[u8]) -> Result<FontMap> {
	let font = Font::parse(ttf).ok_or_else(|| error!("[lightnovel] unreadable font"))?;
	let mut pairs = Vec::with_capacity(POOL.len());
	let mut unknown = 0;
	for &fake in POOL.iter() {
		let real = font
			.glyph_id(fake)
			.and_then(|gid: u16| font.outline(gid, 0))
			.and_then(|outline: Outline| lookup(&outline));
		match real {
			Some(real) => pairs.push((fake, real)),
			None => unknown += 1,
		}
	}
	if unknown > 0 {
		// The site changed its base font or its pool: rerun tools/build_glyph_table.py.
		println!("[lightnovel] {unknown} obfuscated glyphs not in the table");
	}
	if pairs.is_empty() {
		bail!("[lightnovel] font matches no known glyph");
	}
	Ok(FontMap(pairs))
}

struct Outline {
	ends: Vec<u16>,
	points: Vec<(f32, f32)>,
}

/// Same hash as `sig_hash` in tools/build_glyph_table.py: FNV-1a over the contour count
/// and end indices, each as two little-endian bytes.
fn sig_hash(ends: &[u16]) -> u32 {
	let mut h: u32 = 0x811C_9DC5;
	let count = ends.len() as u16;
	for v in core::iter::once(&count).chain(ends.iter()) {
		for b in [(v & 0xFF) as u8, (v >> 8) as u8] {
			h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
		}
	}
	h
}

/// Must match `ref_indices` in tools/build_glyph_table.py.
fn ref_indices(n: usize) -> [usize; REF_POINTS] {
	let mut idx = [0; REF_POINTS];
	for (i, slot) in idx.iter_mut().enumerate() {
		*slot = i * n.saturating_sub(1) / (REF_POINTS - 1);
	}
	idx
}

fn lookup(outline: &Outline) -> Option<u16> {
	if outline.ends.is_empty() {
		return None;
	}
	let h = sig_hash(&outline.ends);
	let start = SIGS.partition_point(|row: &(u32, u16, u16)| row.0 < h);
	let rows = &SIGS[start..SIGS[start..].iter().position(|r: &(u32, u16, u16)| r.0 != h).map_or(SIGS.len(), |p| start + p)];
	match rows {
		[] => None,
		[only] => Some(only.1),
		_ => {
			let idx = ref_indices(outline.points.len());
			rows.iter()
				.filter(|row: &&(u32, u16, u16)| usize::from(row.2) < REFS.len())
				.map(|row: &(u32, u16, u16)| {
					let reference = &REFS[usize::from(row.2)];
					let distance: f32 = idx
						.iter()
						.zip(reference.iter())
						.map(|(&i, &(rx, ry))| {
							let (x, y) = outline.points.get(i).copied().unwrap_or((0.0, 0.0));
							(x - f32::from(rx)).abs() + (y - f32::from(ry)).abs()
						})
						.sum();
					(distance, row.1)
				})
				.min_by(|a: &(f32, u16), b: &(f32, u16)| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal))
				.map(|(_, real)| real)
		}
	}
}

// ---------------------------------------------------------------------------
// TrueType parsing
// ---------------------------------------------------------------------------

fn u16_at(data: &[u8], off: usize) -> Option<u16> {
	Some(u16::from_be_bytes([*data.get(off)?, *data.get(off + 1)?]))
}

fn i16_at(data: &[u8], off: usize) -> Option<i16> {
	u16_at(data, off).map(|v: u16| v as i16)
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
	Some(u32::from_be_bytes([*data.get(off)?, *data.get(off + 1)?, *data.get(off + 2)?, *data.get(off + 3)?]))
}

struct Font<'a> {
	data: &'a [u8],
	cmap4: usize,
	loca: usize,
	glyf: usize,
	long_loca: bool,
}

impl<'a> Font<'a> {
	fn parse(data: &'a [u8]) -> Option<Self> {
		let tables = usize::from(u16_at(data, 4)?);
		let (mut cmap, mut loca, mut glyf, mut head) = (None, None, None, None);
		for i in 0..tables {
			let rec = 12 + i * 16;
			let tag = data.get(rec..rec + 4)?;
			let offset = u32_at(data, rec + 8)? as usize;
			match tag {
				b"cmap" => cmap = Some(offset),
				b"loca" => loca = Some(offset),
				b"glyf" => glyf = Some(offset),
				b"head" => head = Some(offset),
				_ => {}
			}
		}
		let (cmap, loca, glyf, head) = (cmap?, loca?, glyf?, head?);
		let long_loca = i16_at(data, head + 50)? == 1;

		// Pick the Windows Unicode BMP (3, 1) format 4 subtable.
		let count = usize::from(u16_at(data, cmap + 2)?);
		let mut cmap4 = None;
		for i in 0..count {
			let rec = cmap + 4 + i * 8;
			let (platform, encoding) = (u16_at(data, rec)?, u16_at(data, rec + 2)?);
			let sub = cmap + u32_at(data, rec + 4)? as usize;
			if (platform, encoding) == (3, 1) && u16_at(data, sub)? == 4 {
				cmap4 = Some(sub);
			}
		}
		Some(Self {
			data,
			cmap4: cmap4?,
			loca,
			glyf,
			long_loca,
		})
	}

	fn glyph_id(&self, cp: u16) -> Option<u16> {
		let d = self.data;
		let t = self.cmap4;
		let seg_x2 = usize::from(u16_at(d, t + 6)?);
		let ends = t + 14;
		let starts = ends + seg_x2 + 2;
		let deltas = starts + seg_x2;
		let ranges = deltas + seg_x2;
		// Binary search the segment whose end code is >= cp.
		let (mut lo, mut hi) = (0, seg_x2 / 2);
		while lo < hi {
			let mid = (lo + hi) / 2;
			if u16_at(d, ends + mid * 2)? < cp {
				lo = mid + 1;
			} else {
				hi = mid;
			}
		}
		let seg = lo;
		if seg >= seg_x2 / 2 || u16_at(d, starts + seg * 2)? > cp {
			return None;
		}
		let delta = u16_at(d, deltas + seg * 2)?;
		let range_off = usize::from(u16_at(d, ranges + seg * 2)?);
		let gid = if range_off == 0 {
			cp.wrapping_add(delta)
		} else {
			let start = u16_at(d, starts + seg * 2)?;
			let addr = ranges + seg * 2 + range_off + usize::from(cp - start) * 2;
			let raw = u16_at(d, addr)?;
			if raw == 0 {
				return None;
			}
			raw.wrapping_add(delta)
		};
		(gid != 0).then_some(gid)
	}

	fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
		let g = usize::from(gid);
		let (a, b) = if self.long_loca {
			(u32_at(self.data, self.loca + g * 4)? as usize, u32_at(self.data, self.loca + g * 4 + 4)? as usize)
		} else {
			(
				usize::from(u16_at(self.data, self.loca + g * 2)?) * 2,
				usize::from(u16_at(self.data, self.loca + g * 2 + 2)?) * 2,
			)
		};
		Some((self.glyf + a, self.glyf + b))
	}

	/// Contour ends and points, with composite glyphs flattened in component order like
	/// fontTools' `getCoordinates`.
	fn outline(&self, gid: u16, depth: u8) -> Option<Outline> {
		let (start, end) = self.glyph_range(gid)?;
		if start == end {
			return Some(Outline { ends: Vec::new(), points: Vec::new() });
		}
		let d = self.data;
		let contours = i16_at(d, start)?;
		if contours >= 0 {
			return self.simple(start, contours as usize);
		}
		if depth > 4 {
			return None;
		}
		let mut out = Outline { ends: Vec::new(), points: Vec::new() };
		let mut p = start + 10;
		loop {
			let flags = u16_at(d, p)?;
			let child = u16_at(d, p + 2)?;
			p += 4;
			let (dx, dy) = if flags & 0x0001 != 0 {
				let v = (f32::from(i16_at(d, p)?), f32::from(i16_at(d, p + 2)?));
				p += 4;
				v
			} else {
				let v = (f32::from(*d.get(p)? as i8), f32::from(*d.get(p + 1)? as i8));
				p += 2;
				v
			};
			let f2dot14 = |off: usize| -> Option<f32> { Some(f32::from(i16_at(d, off)?) / 16384.0) };
			let (mut a, mut b, mut c, mut e) = (1.0, 0.0, 0.0, 1.0);
			if flags & 0x0008 != 0 {
				a = f2dot14(p)?;
				e = a;
				p += 2;
			} else if flags & 0x0040 != 0 {
				a = f2dot14(p)?;
				e = f2dot14(p + 2)?;
				p += 4;
			} else if flags & 0x0080 != 0 {
				a = f2dot14(p)?;
				b = f2dot14(p + 2)?;
				c = f2dot14(p + 4)?;
				e = f2dot14(p + 6)?;
				p += 8;
			}
			let part = self.outline(child, depth + 1)?;
			let base = out.points.len() as u16;
			out.ends.extend(part.ends.iter().map(|end: &u16| end + base));
			out.points
				.extend(part.points.iter().map(|&(x, y)| (x * a + y * c + dx, x * b + y * e + dy)));
			if flags & 0x0020 == 0 {
				break;
			}
		}
		Some(out)
	}

	fn simple(&self, start: usize, contours: usize) -> Option<Outline> {
		let d = self.data;
		let mut ends = Vec::with_capacity(contours);
		for i in 0..contours {
			ends.push(u16_at(d, start + 10 + i * 2)?);
		}
		let count = ends.last().map_or(0, |e: &u16| usize::from(*e) + 1);
		let instr_len = usize::from(u16_at(d, start + 10 + contours * 2)?);
		let mut p = start + 12 + contours * 2 + instr_len;

		let mut flags = Vec::with_capacity(count);
		while flags.len() < count {
			let flag = *d.get(p)?;
			p += 1;
			flags.push(flag);
			if flag & 0x08 != 0 {
				let repeat = *d.get(p)?;
				p += 1;
				for _ in 0..repeat {
					flags.push(flag);
				}
			}
		}
		flags.truncate(count);

		let mut read_axis = |short: u8, same: u8| -> Option<Vec<f32>> {
			let mut values = Vec::with_capacity(count);
			let mut v: i32 = 0;
			for &flag in &flags {
				if flag & short != 0 {
					let delta = i32::from(*d.get(p)?);
					p += 1;
					v += if flag & same != 0 { delta } else { -delta };
				} else if flag & same == 0 {
					v += i32::from(i16_at(d, p)?);
					p += 2;
				}
				values.push(v as f32);
			}
			Some(values)
		};
		let xs = read_axis(0x02, 0x10)?;
		let ys = read_axis(0x04, 0x20)?;
		Some(Outline {
			ends,
			points: xs.into_iter().zip(ys).collect(),
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn hash_matches_generator() {
		// tools/build_glyph_table.py sig_hash((5, 9, 13, 42, 56)), the layout of 的.
		let ends = [5u16, 9, 13, 42, 56];
		let h = sig_hash(&ends);
		assert!(SIGS.iter().any(|row: &(u32, u16, u16)| row.0 == h && row.1 == '的' as u16));
	}

	#[aidoku_test]
	fn ref_indices_span_the_glyph() {
		let idx = ref_indices(57);
		assert_eq!(idx[0], 0);
		assert_eq!(idx[REF_POINTS - 1], 56);
	}

	#[aidoku_test]
	fn decodes_sample_font() {
		// Subset of a font served on 2026-09-29 that tools/build_glyph_table.py never
		// saw, with the obfuscated opening of that chapter and its decoded form.
		let font = include_bytes!("../tests/font_2ada196a.ttf");
		let sample = include_str!("../tests/font_2ada196a_sample.txt");
		let expected = include_str!("../tests/font_2ada196a_expected.txt");
		let map = build_map(font).expect("map");
		assert_eq!(map.decode(sample), expected);
		let stored = FontMap::from_stored(&map.to_stored()).expect("stored");
		assert_eq!(stored.decode(sample), expected);
	}
}
