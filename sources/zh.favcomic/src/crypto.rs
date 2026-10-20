use aes::Aes128;
use aidoku::alloc::Vec;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

type Aes128CbcDec = cbc::Decryptor<Aes128>;

/// AES-128 key used for every encrypted image favcomic serves.
///
/// The site ships it base64-encoded as `NlgrYjYuRT5ic1hifSs9Tg==`, reachable only from the
/// obfuscated worker bundle at runtime. It has been stable across manga uploaded four years
/// apart, but treat it as rotatable: if it ever changes, decryption yields garbage rather
/// than failing, which is why every caller re-checks the magic bytes afterwards.
const IMAGE_KEY: [u8; 16] = *b"6X+b6.E>bsXb}+=N";

const BLOCK: usize = 16;

/// Whether these bytes already look like an image container we can hand back untouched.
///
/// Needed because Aidoku only skips its own decoding step for responses it could not decode:
/// a body that was never encrypted arrives here as a re-encoded PNG instead.
pub fn is_plain_image(data: &[u8]) -> bool {
	if data.len() < 12 {
		return false;
	}
	data.starts_with(&[0xFF, 0xD8, 0xFF])
		|| data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
		|| data.starts_with(b"GIF8")
		|| (data.starts_with(b"RIFF") && &data[8..12] == b"WEBP")
}

/// Decrypts a favcomic image body.
///
/// Wire format is `IV (16 bytes) || AES-128-CBC ciphertext`, PKCS#7 padded. Decryption happens
/// in place and the plaintext is moved to the front of the same allocation.
pub fn decrypt_image(mut data: Vec<u8>) -> Option<Vec<u8>> {
	if data.len() < BLOCK * 2 || !data.len().is_multiple_of(BLOCK) {
		return None;
	}

	let mut iv = [0u8; BLOCK];
	iv.copy_from_slice(&data[..BLOCK]);

	let plain_len = Aes128CbcDec::new(&IMAGE_KEY.into(), &iv.into())
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

	/// Vector produced independently with `openssl enc -aes-128-cbc` using the real site key,
	/// so this checks our decryption against another implementation, not against itself.
	/// Plaintext is 20 bytes (not a block multiple) so PKCS#7 unpadding is exercised too.
	const IV: [u8; 16] = [
		0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
		0x0f,
	];
	const CIPHERTEXT: [u8; 32] = [
		0x04, 0x6a, 0xde, 0x4a, 0xae, 0x22, 0x57, 0x0b, 0xb8, 0x44, 0x7c, 0xb0, 0x15, 0x9c, 0x91,
		0x30, 0x57, 0xe9, 0xc6, 0xaa, 0xb0, 0x16, 0x3a, 0xb3, 0x3a, 0x11, 0x93, 0x5a, 0x22, 0xd7,
		0xd6, 0xff,
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
