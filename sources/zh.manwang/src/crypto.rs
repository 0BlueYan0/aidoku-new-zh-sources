//! The reader page's image list.
//!
//! The page carries `var tpl_path = '...', params = '<base64>';`. The site's
//! `template/pc/33/js/pic-v2.js` (jsjiami-obfuscated) decrypts it with CryptoJS:
//! base64-decode, the first 16 bytes are the IV, the rest is AES-128-CBC ciphertext
//! under a fixed key, Pkcs7 padding. The plaintext is JSON:
//! `{"host":"www.manwang.net","source_id":"15","comic_id":"...","chapter_id":"...",
//! "images":["https:\/\/dmw.546457.xyz\/..\/..\/<md5>.webp",...],"lazy":false}`.
//!
//! When images stop loading, fetch `pic-v2.js` again and run it under node with
//! `CryptoJS.AES.decrypt` wrapped to log its key: the key is the only constant here.

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use aidoku::{
	alloc::{String, Vec},
	prelude::*,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

const KEY: &[u8; 16] = b"9S8$vJnU2ANeSRoF";

/// The reader's decrypted image list: the site's `source_id` and the image addresses.
pub struct ImageList {
	pub source_id: String,
	pub images: Vec<String>,
}

/// The `params` string from the reader page's inline script.
pub fn params_of(html: &str) -> Option<&str> {
	let (_, rest) = html.split_once("params = '")?;
	let (params, _) = rest.split_once('\'')?;
	Some(params)
}

pub fn decrypt_params(params: &str) -> Option<String> {
	let blob = STANDARD.decode(params.trim()).ok()?;
	if blob.len() <= 16 || !(blob.len() - 16).is_multiple_of(16) {
		return None;
	}
	let (iv, ciphertext) = blob.split_at(16);
	let mut buffer = ciphertext.to_vec();
	let iv: &[u8; 16] = iv.try_into().ok()?;
	let length = Aes128CbcDec::new(KEY.into(), iv.into())
		.decrypt_padded_mut::<Pkcs7>(&mut buffer)
		.ok()?
		.len();
	buffer.truncate(length);
	String::from_utf8(buffer).ok()
}

/// A JSON string's contents with `\/`, `\"`, `\\` and `\uXXXX` resolved.
fn unescape(raw: &str) -> String {
	let mut out = String::new();
	let mut chars = raw.chars();
	while let Some(c) = chars.next() {
		if c != '\\' {
			out.push(c);
			continue;
		}
		match chars.next() {
			Some('u') => {
				let hex: String = chars.by_ref().take(4).collect();
				if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
					out.push(c);
				}
			}
			Some('n') => out.push('\n'),
			Some('t') => out.push('\t'),
			Some(other) => out.push(other),
			None => {}
		}
	}
	out
}

/// The JSON strings inside `[...]` starting at `text`'s first `[`.
fn string_array(text: &str) -> Option<Vec<String>> {
	let start = text.find('[')?;
	let mut items = Vec::new();
	let mut rest = &text[start + 1..];
	loop {
		rest = rest.trim_start_matches([' ', '\n', '\r', '\t', ',']);
		if rest.starts_with(']') {
			return Some(items);
		}
		rest = rest.strip_prefix('"')?;
		let mut end = None;
		let mut escaped = false;
		for (index, c) in rest.char_indices() {
			match c {
				_ if escaped => escaped = false,
				'\\' => escaped = true,
				'"' => {
					end = Some(index);
					break;
				}
				_ => {}
			}
		}
		let end = end?;
		items.push(unescape(&rest[..end]));
		rest = &rest[end + 1..];
	}
}

/// A top-level string or number field: `"source_id":"15"` or `"source_id":15`.
fn scalar_field(json: &str, name: &str) -> Option<String> {
	let (_, rest) = json.split_once(&format!("\"{name}\""))?;
	let rest = rest.trim_start().strip_prefix(':')?.trim_start();
	if let Some(rest) = rest.strip_prefix('"') {
		let (value, _) = rest.split_once('"')?;
		Some(unescape(value))
	} else {
		let value: String = rest.chars().take_while(|c: &char| c.is_ascii_digit()).collect();
		(!value.is_empty()).then_some(value)
	}
}

pub fn parse_image_list(json: &str) -> Option<ImageList> {
	let (_, rest) = json.split_once("\"images\"")?;
	let images = string_array(rest.trim_start().strip_prefix(':')?)?;
	Some(ImageList {
		source_id: scalar_field(json, "source_id").unwrap_or_default(),
		images,
	})
}

/// The image list of a reader page, or `None` when the page has no decryptable list.
pub fn image_list(html: &str) -> Option<ImageList> {
	parse_image_list(&decrypt_params(params_of(html)?)?)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn decrypts_the_reader_page() {
		let list = image_list(include_str!("fixtures/reader.html")).expect("image list");
		assert_eq!(list.source_id, "15");
		assert_eq!(list.images.len(), 15);
		assert_eq!(
			list.images[0],
			"https://dmw.546457.xyz/6d/7e/6d7e1fa50aea92b2511b67a688bfbc58.webp"
		);
		assert_eq!(
			list.images[14],
			"https://dmw.546457.xyz/e3/85/e3850e88d8240a93e3d12d402bdd2839.webp"
		);
	}

	#[aidoku_test]
	fn reads_the_json() {
		let list = parse_image_list(r#"{"host":"x","source_id":12,"images":["\/a\/b.jpg", "cA"],"lazy":false}"#)
			.expect("list");
		assert_eq!(list.source_id, "12");
		assert_eq!(list.images, ["/a/b.jpg", "cA"]);
		assert_eq!(parse_image_list(r#"{"images":[]}"#).map(|l| l.images.len()), Some(0));
		assert!(decrypt_params("not base64!").is_none());
		assert!(params_of("<script>var x = 1;</script>").is_none());
	}
}
