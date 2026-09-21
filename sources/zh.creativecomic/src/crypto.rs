//! Page image decryption.
//!
//! CCC serves page images as AES-256-CBC ciphertext. The content key for each page is
//! itself encrypted with a key derived from the viewer's credential, so decrypting a
//! page takes two passes:
//!
//! 1. Derive a key-encryption key from the credential (the access token when signed in,
//!    a fixed public string for guests).
//! 2. Use it to unwrap the per-page key the API hands out, which decrypts to
//!    `"<key hex>:<iv hex>"`.
//! 3. Decrypt the image with that key. The plaintext is a `data:` URI whose base64
//!    payload is the actual JPEG.

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use aidoku::alloc::{String, Vec};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha512};

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// The credential guests decrypt with. The site ships it in its reader bundle.
pub const GUEST_SECRET: &str = "freeforccc2020reading";

/// Mirrors the site's `token2key`: SHA-512 of the credential rendered as lowercase hex,
/// then key = hex[0..64] and iv = hex[30..62].
///
/// Because both offsets are even, those hex windows are whole bytes of the digest, so
/// this slices the digest directly instead of round-tripping through a hex string. The
/// iv offset really is 30 and not 32 — the two windows overlap by one byte.
fn credential_key(secret: &str) -> ([u8; 32], [u8; 16]) {
	let digest = Sha512::digest(secret.as_bytes());
	let mut key = [0u8; 32];
	let mut iv = [0u8; 16];
	key.copy_from_slice(&digest[0..32]);
	iv.copy_from_slice(&digest[15..31]);
	(key, iv)
}

fn hex_nibble(byte: u8) -> Option<u8> {
	match byte {
		b'0'..=b'9' => Some(byte - b'0'),
		b'a'..=b'f' => Some(byte - b'a' + 10),
		b'A'..=b'F' => Some(byte - b'A' + 10),
		_ => None,
	}
}

/// Decode an even-length hex string into exactly `N` bytes.
fn hex_to_array<const N: usize>(hex: &str) -> Option<[u8; N]> {
	let bytes = hex.as_bytes();
	if bytes.len() != N * 2 {
		return None;
	}
	let mut out = [0u8; N];
	for (index, slot) in out.iter_mut().enumerate() {
		let high = hex_nibble(bytes[index * 2])?;
		let low = hex_nibble(bytes[index * 2 + 1])?;
		*slot = (high << 4) | low;
	}
	Some(out)
}

fn aes_cbc_decrypt(ciphertext: &[u8], key: &[u8; 32], iv: &[u8; 16]) -> Option<Vec<u8>> {
	if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
		return None;
	}
	let mut buffer = ciphertext.to_vec();
	let plaintext = Aes256CbcDec::new(key.into(), iv.into())
		.decrypt_padded_mut::<Pkcs7>(&mut buffer)
		.ok()?;
	Some(plaintext.to_vec())
}

/// Unwrap the per-page key returned by `GET /book/chapter/image/{id}`.
///
/// `wrapped` is base64; it decrypts to `"<key hex>:<iv hex>"`.
pub fn unwrap_page_key(wrapped: &str, secret: &str) -> Option<([u8; 32], [u8; 16])> {
	let (kek, kek_iv) = credential_key(secret);
	let blob = STANDARD.decode(wrapped.trim()).ok()?;
	let plaintext = aes_cbc_decrypt(&blob, &kek, &kek_iv)?;
	let text = String::from_utf8(plaintext).ok()?;
	let (key_hex, iv_hex) = text.trim().split_once(':')?;
	Some((hex_to_array::<32>(key_hex)?, hex_to_array::<16>(iv_hex)?))
}

/// Decrypt one page image into raw JPEG bytes.
///
/// The decrypted text is a `data:image/...;base64,...` URI, so the base64 payload after
/// the comma is what actually gets returned.
pub fn decrypt_page(ciphertext: &[u8], key: &[u8; 32], iv: &[u8; 16]) -> Option<Vec<u8>> {
	let plaintext = aes_cbc_decrypt(ciphertext, key, iv)?;
	let text = String::from_utf8(plaintext).ok()?;
	let payload = match text.split_once(',') {
		Some((prefix, rest)) if prefix.starts_with("data:") => rest,
		// Fall back to treating the whole thing as base64 in case the site ever drops
		// the data URI wrapper.
		_ => text.as_str(),
	};
	STANDARD.decode(payload.trim()).ok()
}

/// Render a key/iv pair as hex so it can ride along in a `PageContext`.
pub fn to_hex(bytes: &[u8]) -> String {
	const DIGITS: &[u8; 16] = b"0123456789abcdef";
	let mut out = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		out.push(DIGITS[(byte >> 4) as usize] as char);
		out.push(DIGITS[(byte & 0x0f) as usize] as char);
	}
	out
}

pub fn key_from_hex(hex: &str) -> Option<[u8; 32]> {
	hex_to_array::<32>(hex)
}

pub fn iv_from_hex(hex: &str) -> Option<[u8; 16]> {
	hex_to_array::<16>(hex)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	// A synthetic key pair and the matching fixtures, produced offline with openssl so
	// the tests never depend on live site data (which would rot when keys rotate).
	const TEST_KEY_HEX: &str =
		"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
	const TEST_IV_HEX: &str = "fedcba9876543210fedcba9876543210";
	const WRAPPED_KEY: &str = "VWRsoDe26w2k/hJKuCOJ6fv75y38gXRENC7XB/sWaiZZvGowQCWhKCJAjmcAGwGletfjFcG4SwDbvsVgC6v5uaIpe5YrL278Qogn1WshBK/VeD+gBWcvXiWd4YQU1x1vZKGZWIwpMkRaqmclpUCxEQ==";
	const PAGE_CIPHERTEXT: &str = "x0W4dbklvjwJh2RBLHzrKOQQkUFwbLlDNKo3ET75U+eyDdChtWE9W4glCQBR0CDn";

	/// SHA-512 of a fixed string never changes, so these constants pin the derivation
	/// permanently. The iv starting mid-byte-15 is the easiest part to get wrong.
	#[aidoku_test]
	fn derives_the_guest_credential_key() {
		let (key, iv) = credential_key(GUEST_SECRET);
		assert_eq!(
			to_hex(&key),
			"8134f84a8dbde288125cf50029c1992cb7e197b42290404a1efe7ab0dfe16aee"
		);
		assert_eq!(to_hex(&iv), "2cb7e197b42290404a1efe7ab0dfe16a");
	}

	#[aidoku_test]
	fn unwraps_a_page_key() {
		let (key, iv) = unwrap_page_key(WRAPPED_KEY, GUEST_SECRET).expect("should unwrap");
		assert_eq!(to_hex(&key), TEST_KEY_HEX);
		assert_eq!(to_hex(&iv), TEST_IV_HEX);
	}

	#[aidoku_test]
	fn unwrapping_with_the_wrong_credential_fails() {
		assert!(unwrap_page_key(WRAPPED_KEY, "not-the-secret").is_none());
	}

	#[aidoku_test]
	fn decrypts_a_page_and_strips_the_data_uri() {
		let key = key_from_hex(TEST_KEY_HEX).unwrap();
		let iv = iv_from_hex(TEST_IV_HEX).unwrap();
		let ciphertext = STANDARD.decode(PAGE_CIPHERTEXT).unwrap();
		let image = decrypt_page(&ciphertext, &key, &iv).expect("should decrypt");
		assert_eq!(image, b"CCC-TEST-PAYLOAD");
	}

	#[aidoku_test]
	fn rejects_ciphertext_that_is_not_a_block_multiple() {
		let key = key_from_hex(TEST_KEY_HEX).unwrap();
		let iv = iv_from_hex(TEST_IV_HEX).unwrap();
		assert!(decrypt_page(&[1, 2, 3], &key, &iv).is_none());
	}

	#[aidoku_test]
	fn hex_round_trips() {
		assert_eq!(to_hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
		assert!(hex_to_array::<16>("zz").is_none());
	}
}
