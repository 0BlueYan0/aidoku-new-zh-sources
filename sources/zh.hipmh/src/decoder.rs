//! Decodes the `images` string of `GET /v2/chapter` into image paths.
//!
//! The site ships the algorithm in an obfuscated script,
//! `https://reader.hipmh.top/assets/runtime/chapter-decoder.js`. It is the same scheme the
//! official `zh.godamanga` source decodes (`J7r`/`kD`/`W4s`/`nQ`), with different constants,
//! so the site can and does rotate them. When pages stop loading, re-derive the four
//! constants before touching the logic:
//!
//! 1. Download the script and `eval` it in node with `global.window = global`.
//! 2. The string table decoder is the function taking `(index, key)` that calls the array
//!    function first (`_0x3b94` on 2026-09-24). Replace every `_0x....(0x..., '...')` call
//!    in the source with its evaluated result.
//! 3. The object literal holding the custom alphabet (`_-9876543210abc...`) also holds the
//!    prefix and the second marker; the first marker and the suffix are two-character
//!    literals right after it; the group size is the arithmetic literal after those.
//!
//! The decoded list carries one decoy image, removed by `remove_decoy` (the reader script
//! `_ChapterHidPage.astro_*.js`, function `H`). Measured on chapter 1 of manga 8083
//! (2026-09-24): 151 paths, index 78 removed; that file is 67 bytes, while the preceding
//! path with the same page number `_78` is the real 39 KB page.

use crate::helper::json_top_strings;
use aidoku::{
	alloc::{String, Vec},
	prelude::*,
	Result,
};

const PREFIX: &str = "qM9";
const MARKER1: &str = "Vx";
const MARKER2: &str = "pL0";
const SUFFIX: &str = "Z7";
const GROUP: usize = 7;

const STD_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const CUSTOM_ALPHABET: &[u8] = b"_-9876543210abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The multipliers of the reader's decoy formula, written as the script builds them.
const DECOY_D: u64 = (40503 << 16) | 31153;
const DECOY_R: u64 = (34283 << 16) | 51819;

/// Decode the encoded string into the image paths it lists, decoy included.
pub fn decode(input: &str) -> Result<Vec<String>> {
	let body = input
		.strip_prefix(PREFIX)
		.and_then(|rest: &str| rest.strip_suffix(SUFFIX))
		.ok_or_else(|| error!("chapter images: unknown prefix or suffix"))?;
	if !body.is_ascii() {
		bail!("chapter images: non-ASCII payload");
	}

	let payload_len = body
		.len()
		.checked_sub(MARKER1.len() + MARKER2.len())
		.filter(|&len: &usize| len > 0)
		.ok_or_else(|| error!("chapter images: payload too short"))?;
	let a_len = payload_len / 3;
	let b_len = (payload_len - a_len) / 2;
	let c_len = payload_len - a_len - b_len;

	let marker1_end = b_len + MARKER1.len();
	let part2_end = marker1_end + c_len;
	let marker2_end = part2_end + MARKER2.len();

	let part1 = &body[..b_len];
	let marker1 = &body[b_len..marker1_end];
	let part2 = &body[marker1_end..part2_end];
	let marker2 = &body[part2_end..marker2_end];
	let part3 = &body[marker2_end..];
	if marker1 != MARKER1 || marker2 != MARKER2 || part3.len() != a_len {
		bail!("chapter images: markers not where expected");
	}

	let mut reordered = String::with_capacity(payload_len);
	reordered.push_str(part3);
	reordered.push_str(part1);
	reordered.push_str(part2);

	let mapped = map_alphabet(&unzigzag(reordered.as_bytes()))?;
	let json = base64_url_decode(&mapped)?;
	let json = String::from_utf8(json).map_err(|_| error!("chapter images: invalid UTF-8"))?;
	Ok(json_top_strings(&json))
}

/// Drop the decoy entry, whose index the reader derives from `order_id` and `sid`.
/// An index outside the list means there is nothing to drop, as in the reader.
pub fn remove_decoy(images: &mut Vec<String>, order_id: u64, sid: u64) {
	let len = images.len() as u64;
	if len == 0 {
		return;
	}
	// Both products stay below 2^64: every factor is below 2^32.
	let offset = ((sid.wrapping_mul(DECOY_D)) ^ (len.wrapping_mul(DECOY_R))) % len;
	let index = order_id ^ offset;
	if index < len {
		images.remove(index as usize);
	}
}

/// Reverse every odd group of `GROUP` bytes.
fn unzigzag(bytes: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(bytes.len());
	for (block, chunk) in bytes.chunks(GROUP).enumerate() {
		if block % 2 == 1 {
			out.extend(chunk.iter().rev());
		} else {
			out.extend_from_slice(chunk);
		}
	}
	out
}

fn map_alphabet(bytes: &[u8]) -> Result<Vec<u8>> {
	bytes
		.iter()
		.map(|byte: &u8| {
			CUSTOM_ALPHABET
				.iter()
				.position(|c: &u8| c == byte)
				.map(|i: usize| STD_ALPHABET[i])
				.ok_or_else(|| error!("chapter images: character outside the alphabet"))
		})
		.collect()
}

fn base64_url_decode(bytes: &[u8]) -> Result<Vec<u8>> {
	let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
	let mut buffer = 0u32;
	let mut bits = 0u32;
	for &byte in bytes {
		let value = match byte {
			b'A'..=b'Z' => byte - b'A',
			b'a'..=b'z' => byte - b'a' + 26,
			b'0'..=b'9' => byte - b'0' + 52,
			b'-' => 62,
			b'_' => 63,
			b'=' => break,
			_ => bail!("chapter images: invalid base64"),
		};
		buffer = (buffer << 6) | u32::from(value);
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((buffer >> bits) as u8);
			buffer &= (1 << bits) - 1;
		}
	}
	Ok(out)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	/// `images` of `GET /v2/chapter?hid=Yzo5MjAz-ODA4MzoxLjAw` (manga 8083, chapter 1),
	/// with `order_id` 106 and `sid` 9203, captured 2026-09-24.
	const CHAPTER_1: &str = include_str!("test_chapter_images.txt");
	const FOLDER: &str = "/i/f4VYJA7-l5CeLzKZdirSoieDP9U4fd9hD2LqS_qNWRPFJRolJa7ul6ZbAlz5xzqtdSHHshvkYQ/";

	#[aidoku_test]
	fn decodes_the_same_list_as_the_site_script() {
		let images = decode(CHAPTER_1).expect("decode");
		assert_eq!(images.len(), 151);
		assert_eq!(images[0], format!("{FOLDER}MmE5ZTVhMWVfMV8x_1.lpy8fs.webp"));
		assert_eq!(images[150], format!("{FOLDER}MmE5ZTVhMWVfMV8xNTA_150.dv9owf.webp"));
	}

	#[aidoku_test]
	fn removes_the_decoy_the_reader_removes() {
		let mut images = decode(CHAPTER_1).expect("decode");
		remove_decoy(&mut images, 106, 9203);
		assert_eq!(images.len(), 150);
		// The real page 78 stays, its 67-byte twin is gone, and page 79 follows it.
		assert_eq!(images[77], format!("{FOLDER}MmE5ZTVhMWVfMV83OA_78.oteos5.webp"));
		assert!(!images.iter().any(|path: &String| path.ends_with("_78.m3x7q0.webp")));
		assert!(images[78].contains("_79."));
	}

	#[aidoku_test]
	fn rejects_rotated_constants() {
		assert!(decode("J7rabckDdefW4sghinQ").is_err());
		assert!(decode("").is_err());
	}
}
