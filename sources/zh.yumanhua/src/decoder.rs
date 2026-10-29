//! Decodes the reader page's image list.
//!
//! The reader page has no `<img>` tags for the pages. The site's obfuscated
//! `/static/js/all2.js` (jsjiami v6, 2026-10-13) builds them from two values on the page:
//!
//! 1. An inline `eval(function(p,a,c,k,e,d){...}('<p>',<a>,<c>,'<k>'.split('|'),0,{}))`
//!    (Dean Edwards' packer) that evaluates to `var __c0rst96="<base64>"`.
//! 2. `<div class="readerContainer" data-id="N">`, which picks one of ten keys.
//!
//! `all2.js` then base64-decodes `__c0rst96`, XORs it with the key (repeated), and
//! base64-decodes the result into a JSON array of image addresses.
//!
//! When pages stop loading, re-derive the keys before touching the logic: download
//! `all2.js`, `eval` it in node with a stub `$` whose `append` records its argument, and
//! replace `][_0x4a0135];var _0x3af5f6` with `];global.KEYS=_0x3d1d18.slice();...` to read
//! the key table (each entry is the base64 of the key below). The array name changes with
//! every re-obfuscation; look for the ten-element array indexed by `data-id`. Also check
//! whether the packed payload gained escapes such as `\n`: `quoted` keeps only the escaped
//! character, which would join it to the neighbouring word.
//! Checked on 12 chapters of 6 works (2026-10-13): `data-id` 0, 1, 2, 3, 6 and 8, 4 to
//! 120 images each.

use crate::helper::json_top_strings;
use aidoku::alloc::{String, Vec};
use base64::{
	alphabet,
	engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
	Engine,
};

/// The keys `all2.js` picks from by `data-id`, decoded from its base64 table.
pub const KEYS: [&str; 10] = [
	"smkhy258", "smkd95fv", "md496952", "cdcsdwq", "vbfsa256", "cawf151c", "cd56cvda", "8kihnt9",
	"dso15tlo", "5ko6plhy",
];

/// The variable the packed script assigns.
const VARIABLE: &str = "__c0rst96=\"";

/// `atob` ignores missing padding, so this does too.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
	GeneralPurpose::new(
		&alphabet::STANDARD,
		GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
	)
	.decode(text.trim())
	.ok()
}

/// The value of one packer digit: `0-9`, `a-z`, then `A-Z`, as the packer's `e(c)`
/// writes `c % a` (`c > 35` becomes `chr(c + 29)`).
fn digit_value(b: u8) -> Option<usize> {
	match b {
		b'0'..=b'9' => Some((b - b'0') as usize),
		b'a'..=b'z' => Some((b - b'a') as usize + 10),
		b'A'..=b'Z' => Some((b - b'A') as usize + 36),
		_ => None,
	}
}

/// The index a packed word stands for, or `None` if it is not one the packer wrote. The
/// packer never writes a leading zero, so `01` stays a literal.
fn word_index(word: &str, radix: usize) -> Option<usize> {
	let bytes = word.as_bytes();
	if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
		return None;
	}
	let mut value = 0usize;
	for &b in bytes {
		let digit = digit_value(b).filter(|d: &usize| *d < radix)?;
		value = value.checked_mul(radix)?.checked_add(digit)?;
	}
	Some(value)
}

/// A single-quoted JS string starting just after its opening quote: its contents and the
/// rest of the text after the closing quote.
fn quoted(text: &str) -> Option<(String, &str)> {
	let mut out = String::new();
	let mut chars = text.char_indices();
	while let Some((i, ch)) = chars.next() {
		match ch {
			'\\' => out.push(chars.next()?.1),
			'\'' => return Some((out, &text[i + 1..])),
			_ => out.push(ch),
		}
	}
	None
}

/// Run a packed script: replace every word of `p` that encodes an index into `k` with that
/// entry. An empty entry leaves the word as it is, as the packer's `k[c] || e(c)` does.
pub fn unpack(script: &str) -> Option<String> {
	let start = script.find("}('")? + 3;
	let (payload, rest) = quoted(&script[start..])?;
	let mut numbers = rest.strip_prefix(',')?.splitn(3, ',');
	let radix = numbers.next()?.trim().parse::<usize>().ok()?;
	let _count = numbers.next()?.trim().parse::<usize>().ok()?;
	let (words, _) = quoted(numbers.next()?.trim_start().strip_prefix('\'')?)?;
	let words: Vec<&str> = words.split('|').collect();
	if !(2..=62).contains(&radix) {
		return None;
	}

	let mut out = String::with_capacity(payload.len() * 2);
	let mut word = String::new();
	let flush = |word: &mut String, out: &mut String| {
		match word_index(word, radix).and_then(|i: usize| words.get(i)) {
			Some(entry) if !entry.is_empty() => out.push_str(entry),
			_ => out.push_str(word),
		}
		word.clear();
	};
	for ch in payload.chars() {
		if ch.is_ascii_alphanumeric() || ch == '_' {
			word.push(ch);
		} else {
			flush(&mut word, &mut out);
			out.push(ch);
		}
	}
	flush(&mut word, &mut out);
	Some(out)
}

/// The image addresses from the unpacked script's `__c0rst96` and the page's `data-id`.
pub fn decode_images(unpacked: &str, key_index: usize) -> Option<Vec<String>> {
	let start = unpacked.find(VARIABLE)? + VARIABLE.len();
	let value = &unpacked[start..start + unpacked[start..].find('"')?];
	let key = KEYS.get(key_index)?.as_bytes();
	let mixed: Vec<u8> = base64_decode(value)?
		.iter()
		.enumerate()
		.map(|(i, b): (usize, &u8)| b ^ key[i % key.len()])
		.collect();
	let json = String::from_utf8(base64_decode(core::str::from_utf8(&mixed).ok()?)?).ok()?;
	let images = json_top_strings(&json);
	(!images.is_empty()).then_some(images)
}

/// The image addresses of a reader page, given the text of every inline script and the
/// `data-id` of `.readerContainer`.
pub fn decode_pages<S: AsRef<str>>(scripts: &[S], key_index: usize) -> Option<Vec<String>> {
	scripts
		.iter()
		.map(|s: &S| s.as_ref())
		.filter(|s: &&str| s.contains("eval(function(p,a,c,k,e"))
		.filter_map(unpack)
		.find_map(|unpacked: String| decode_images(&unpacked, key_index))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn unpacks_words_by_index() {
		let script =
			"eval(function(p,a,c,k,e,d){}('0 1=\"2+3\"',62,4,'var|x||Ab'.split('|'),0,{}))";
		assert_eq!(unpack(script).as_deref(), Some("var x=\"2+Ab\""));
		// Base 62: `10` is 62, `A` is 36; a leading zero is never a packed word.
		assert_eq!(word_index("10", 62), Some(62));
		assert_eq!(word_index("A", 62), Some(36));
		assert_eq!(word_index("01", 62), None);
		assert_eq!(word_index("a", 10), None);
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[path = "fixture_tests/decoder.rs"]
mod fixture_tests;
