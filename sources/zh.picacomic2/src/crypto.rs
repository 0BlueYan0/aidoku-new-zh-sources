use aidoku::{alloc::String, prelude::*, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 as lowercase hex.
pub fn encrypt(data: &[u8], key: &[u8]) -> Result<String> {
	let mut mac = HmacSha256::new_from_slice(key).map_err(|_| error!("Invalid key length"))?;
	mac.update(data);
	Ok(format!("{:x}", mac.finalize().into_bytes()))
}
