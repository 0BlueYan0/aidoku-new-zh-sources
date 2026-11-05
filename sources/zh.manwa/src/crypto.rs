use aes::Aes256;
use aidoku::alloc::Vec;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// AES-256 key for every encrypted image manwa serves: the UTF-8 bytes of a 32-character
/// string the site's reader script holds. Treat it as rotatable. A wrong key decrypts to
/// noise instead of failing, which is why every caller checks the magic bytes afterwards.
const IMAGE_KEY: [u8; 32] = *b"0B6666A0-BB59-1381-B746-a0E4C9AC";

const BLOCK: usize = 16;

/// Whether these bytes already look like an image container we can hand back untouched.
///
/// Aidoku only skips its own decoding step for responses it could not decode, so a body
/// that was never encrypted arrives here as a re-encoded PNG.
pub fn is_plain_image(data: &[u8]) -> bool {
	if data.len() < 12 {
		return false;
	}
	data.starts_with(&[0xFF, 0xD8, 0xFF])
		|| data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
		|| data.starts_with(b"GIF8")
		|| (data.starts_with(b"RIFF") && &data[8..12] == b"WEBP")
}

/// Decrypts a manwa image body.
///
/// Wire format is `IV (16 bytes) || AES-256-CBC ciphertext`, PKCS#7 padded. Decryption
/// happens in place and the plaintext is moved to the front of the same allocation.
pub fn decrypt_image(mut data: Vec<u8>) -> Option<Vec<u8>> {
	if data.len() < BLOCK * 2 || !data.len().is_multiple_of(BLOCK) {
		return None;
	}

	let mut iv = [0u8; BLOCK];
	iv.copy_from_slice(&data[..BLOCK]);

	let plain_len = Aes256CbcDec::new(&IMAGE_KEY.into(), &iv.into())
		.decrypt_padded_mut::<Pkcs7>(&mut data[BLOCK..])
		.ok()?
		.len();

	data.copy_within(BLOCK..BLOCK + plain_len, 0);
	data.truncate(plain_len);
	Some(data)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::alloc::vec;
	use aidoku_test::aidoku_test;

	/// Vector produced independently with `openssl enc -aes-256-cbc` using the real site key,
	/// so this checks our decryption against another implementation, not against itself.
	/// Plaintext is 20 bytes (not a block multiple) so PKCS#7 unpadding is exercised too.
	const IV: [u8; 16] = [
		0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
		0x0f,
	];
	const CIPHERTEXT: [u8; 32] = [
		0xbf, 0x41, 0xe9, 0x78, 0x77, 0x08, 0x9d, 0x2c, 0x3d, 0xfd, 0x5b, 0x8a, 0x75, 0x8a, 0x63,
		0xd6, 0x68, 0xfc, 0xe4, 0xc9, 0xce, 0xd8, 0x23, 0x12, 0x82, 0x88, 0x7d, 0xcf, 0x0b, 0x10,
		0x43, 0x3f,
	];
	const PLAINTEXT: [u8; 20] = [
		0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x02, 0x03, 0x04,
		0x05, 0x06, 0x07, 0x08, 0x09,
	];

	fn body() -> Vec<u8> {
		let mut v = Vec::from(IV);
		v.extend_from_slice(&CIPHERTEXT);
		v
	}

	#[aidoku_test]
	fn decrypts_openssl_vector() {
		assert_eq!(decrypt_image(body()).as_deref(), Some(&PLAINTEXT[..]));
	}

	#[aidoku_test]
	fn decrypted_output_is_a_recognizable_image() {
		let plain = decrypt_image(body()).expect("decryption should succeed");
		assert!(is_plain_image(&plain));
	}

	#[aidoku_test]
	fn rejects_bodies_that_cannot_be_ciphertext() {
		assert_eq!(decrypt_image(vec![0u8; 8]), None); // shorter than IV + one block
		assert_eq!(decrypt_image(vec![0u8; 40]), None); // not a block multiple
	}

	#[aidoku_test]
	fn recognizes_container_magic_bytes() {
		assert!(is_plain_image(&[
			0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0
		]));
		assert!(is_plain_image(b"RIFF\0\0\0\0WEBPVP8 "));
		assert!(is_plain_image(&[
			0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0
		]));
		assert!(!is_plain_image(&IV)); // raw ciphertext must not be mistaken for an image
		assert!(!is_plain_image(b"short"));
	}
}

#[cfg(all(test, feature = "fixtures"))]
#[rustfmt::skip]
#[path = "fixture_tests/crypto.rs"]
mod fixture_tests;
