//! BookWalker TW (PUBLUS NFBR) `configuration_pack.json` decryptor.
//!
//! Port of `NFBR.a6i.ConfigurationDecoder` (viewer_image_2.0.29). The ciphertext is
//! `{"version":"1.0","data":"<base64>"}`; the first 128 base64 chars carry the three
//! 32-byte keys A/Z/T, the rest is the payload. Pipeline (see the reference python):
//!   j4Y(buf) -> d1y -> n8K -> t7r -> M2w -> m7j -> j4Y(T) -> j4Y(Z) -> j4Y(A) -> s7x
//! Passphrase = "configuration_pack.json".

use aidoku::alloc::{vec, vec::Vec};

const PASSPHRASE: &[u8] = b"configuration_pack.json";

/// Minimal standard-alphabet base64 decoder (no line breaks; `=` padding handled).
fn b64_decode(s: &[u8]) -> Option<Vec<u8>> {
	let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
	let mut acc: u32 = 0;
	let mut nbits: u32 = 0;
	for &ch in s {
		let v: u32 = match ch {
			b'A'..=b'Z' => (ch - b'A') as u32,
			b'a'..=b'z' => (ch - b'a' + 26) as u32,
			b'0'..=b'9' => (ch - b'0' + 52) as u32,
			b'+' => 62,
			b'/' => 63,
			b'=' => break,
			b'\n' | b'\r' => continue,
			_ => return None,
		};
		acc = (acc << 6) | v;
		nbits += 6;
		if nbits >= 8 {
			nbits -= 8;
			out.push((acc >> nbits) as u8);
		}
	}
	Some(out)
}

/// RC4 key-scheduling algorithm.
fn ksa(key: &[u8]) -> [u8; 256] {
	let mut s = [0u8; 256];
	for (i, slot) in s.iter_mut().enumerate() {
		*slot = i as u8;
	}
	let mut j: u8 = 0;
	let n = key.len();
	for i in 0..256 {
		j = j.wrapping_add(s[i]).wrapping_add(key[i % n]);
		s.swap(i, j as usize);
	}
	s
}

/// RC4 pseudo-random generation algorithm (stateful keystream).
struct Prga {
	s: [u8; 256],
	i: u8,
	j: u8,
}

impl Prga {
	fn new(key: &[u8]) -> Self {
		Prga {
			s: ksa(key),
			i: 0,
			j: 0,
		}
	}

	fn next(&mut self) -> u8 {
		self.i = self.i.wrapping_add(1);
		self.j = self.j.wrapping_add(self.s[self.i as usize]);
		self.s.swap(self.i as usize, self.j as usize);
		let idx = self.s[self.i as usize].wrapping_add(self.s[self.j as usize]);
		self.s[idx as usize]
	}
}

fn rc4(data: &[u8], key: &[u8]) -> Vec<u8> {
	let mut p = Prga::new(key);
	data.iter().map(|&b| b ^ p.next()).collect()
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
	let mut out = Vec::new();
	for part in parts {
		out.extend_from_slice(part);
	}
	out
}

/// Swap the two adjacent `half`-sized blocks that end at index `b` (KgW `swap_halves`).
fn swap_halves(o: &mut [u8], b: usize, half: usize) {
	for k in 0..half {
		o.swap(b - k, b - half - k);
	}
}

/// `j4Y`: block bit-transform + intra-block permutation + rotated re-emission.
/// Operates on `buf[..n]`; `h`/`w`/`c` seed the transform from their first 32 bytes.
fn j4y(buf: &mut [u8], n: usize, h: Option<&[u8]>, w: Option<&[u8]>, c: Option<&[u8]>) {
	let mut g: u8 = 0;
	let mut cap_w: u8 = 0;
	for arr in [h, w, c].into_iter().flatten() {
		for &x in arr.iter().take(32) {
			g = g.wrapping_add(x);
			cap_w ^= x;
		}
	}
	let flag_y = (g & 2) != 2;
	let flag_h = (g & 4) != 4;
	let flag_b = (g & 8) != 8;
	let v = (cap_w >> 5) as usize;
	let q = 8 - v;

	let mut qq = 0usize;
	while qq < n {
		let mut x = qq + 32;
		let partial = x > n;
		if partial {
			x = n;
		}
		let j = x - qq;
		let mut cap_g = g;
		let mut cap_l = cap_w;
		let mut o = vec![0u8; j];
		for b in 0..j {
			let mut t = buf[qq + b];
			if flag_y {
				t = ((t & 0x55) << 1) | ((t >> 1) & 0x55);
			}
			if flag_h {
				t = ((t & 0x33) << 2) | ((t >> 2) & 0x33);
			}
			if flag_b {
				t = ((t & 0x0f) << 4) | ((t >> 4) & 0x0f);
			}
			o[b] = t;
			cap_g = cap_g.wrapping_add(t);
			cap_l ^= t;
		}
		let cx = (cap_g & 2) != 2;
		let cp = (cap_g & 4) != 4;
		let cl = (cap_g & 8) != 8;
		let cj = (cap_g & 16) != 16;
		let cs = (cap_g & 32) != 32;
		for b in 0..j {
			if (b & 1) == 1 {
				if cx {
					o.swap(b, b - 1);
				}
				if (b & 3) == 3 {
					if cp {
						swap_halves(&mut o, b, 2);
					}
					if (b & 7) == 7 {
						if cl {
							swap_halves(&mut o, b, 4);
						}
						if (b & 15) == 15 {
							if cj {
								swap_halves(&mut o, b, 8);
							}
							if (b & 31) == 31 && cs {
								swap_halves(&mut o, b, 16);
							}
						}
					}
				}
			}
		}
		let s_shift = (cap_l >> 3) as usize;
		let s_val = if partial { s_shift % j } else { s_shift & 31 };
		if v == 0 {
			let mut p = j - s_val;
			for slot in buf.iter_mut().take(x).skip(qq) {
				if p == j {
					p = 0;
				}
				*slot = o[p];
				p += 1;
			}
		} else {
			let mut p = j - s_val - 1;
			for slot in buf.iter_mut().take(x).skip(qq) {
				let mut t = (o[p] as u16) << q;
				p += 1;
				if p == j {
					p = 0;
				}
				t |= (o[p] as u16) >> v;
				*slot = (t & 0xff) as u8;
			}
		}
		qq = x;
	}
}

/// `M2w`: index-selected swaps between A/Z/T/buf driven by each byte.
fn read4(code: usize, g: usize, a: &[u8; 32], z: &[u8; 32], t: &[u8; 32], buf: &[u8]) -> u8 {
	match code {
		0 => a[g],
		1 => z[g],
		2 => t[g],
		_ => buf[g],
	}
}

#[allow(clippy::too_many_arguments)]
fn write4(
	code: usize,
	g: usize,
	val: u8,
	a: &mut [u8; 32],
	z: &mut [u8; 32],
	t: &mut [u8; 32],
	buf: &mut [u8],
) {
	match code {
		0 => a[g] = val,
		1 => z[g] = val,
		2 => t[g] = val,
		_ => buf[g] = val,
	}
}

/// (utf8 json bytes, A, Z, T final keys).
pub type DecryptedConfig = (Vec<u8>, [u8; 32], [u8; 32], [u8; 32]);

/// Decrypt the `data` string. Returns (utf8 json bytes, A, Z, T final keys).
pub fn decrypt_config(data_b64: &str) -> Option<DecryptedConfig> {
	let data = data_b64.as_bytes();
	if data.len() < 128 {
		return None;
	}
	let head = b64_decode(&data[..128])?;
	if head.len() < 96 {
		return None;
	}
	let mut a = [0u8; 32];
	let mut z = [0u8; 32];
	let mut t = [0u8; 32];
	a.copy_from_slice(&head[0..32]);
	z.copy_from_slice(&head[32..64]);
	t.copy_from_slice(&head[64..96]);

	let mut buf = b64_decode(&data[128..])?;
	let n = buf.len();
	let k = PASSPHRASE;

	// j4Y PVE=0
	j4y(&mut buf, n, Some(&a), Some(&z), Some(&t));

	// d1y
	let s = ksa(&cat(&[&z, k, &t]));
	for (idx, b) in buf.iter_mut().enumerate() {
		*b ^= s[idx & 255];
	}

	// n8K: odd indices, descending
	let mut p = Prga::new(&cat(&[k, &a, &z]));
	let mut idx = (n | 1) - 2;
	loop {
		buf[idx] ^= p.next();
		if idx < 2 {
			break;
		}
		idx -= 2;
	}

	// t7r: even indices, descending
	let mut p = Prga::new(&cat(&[&t, k, &a]));
	let mut idx = (n - 1) & !1usize;
	loop {
		buf[idx] ^= p.next();
		if idx < 2 {
			break;
		}
		idx -= 2;
	}

	// M2w
	for g in 0..core::cmp::min(n, 32) {
		let e = buf[g] ^ a[g] ^ z[g] ^ t[g];
		let c12 = ((e & 12) >> 2) as usize;
		let c3 = (e & 3) as usize;
		let v12 = read4(c12, g, &a, &z, &t, &buf);
		let v3 = read4(c3, g, &a, &z, &t, &buf);
		write4(c3, g, v12, &mut a, &mut z, &mut t, &mut buf);
		write4(c12, g, v3, &mut a, &mut z, &mut t, &mut buf);
		let c192 = ((e & 192) >> 6) as usize;
		let c48 = ((e & 48) >> 4) as usize;
		let v192 = read4(c192, g, &a, &z, &t, &buf);
		let v48 = read4(c48, g, &a, &z, &t, &buf);
		write4(c48, g, v192, &mut a, &mut z, &mut t, &mut buf);
		write4(c192, g, v48, &mut a, &mut z, &mut t, &mut buf);
	}

	// m7j: rotate the keys through RC4 (order matters; each uses the freshly built ones)
	let t_new = rc4(&t, &cat(&[&z, &a, k]));
	let z_new = rc4(&z, &cat(&[&a, k, &t_new]));
	let a_new = rc4(&a, &cat(&[k, &t_new, &z_new]));
	a.copy_from_slice(&a_new);
	z.copy_from_slice(&z_new);
	t.copy_from_slice(&t_new);

	// j4Y PVE=1..3 (in place, later calls see the already-transformed keys)
	{
		let (a_ro, z_ro) = (a, z);
		j4y(&mut t, 32, Some(&a_ro), Some(&z_ro), None);
	}
	{
		let (a_ro, t_ro) = (a, t);
		j4y(&mut z, 32, Some(&a_ro), Some(&t_ro), None);
	}
	{
		let (z_ro, t_ro) = (z, t);
		j4y(&mut a, 32, Some(&z_ro), Some(&t_ro), None);
	}

	// s7x
	let mut p = Prga::new(&cat(&[&t, &z, k]));
	for b in buf.iter_mut() {
		*b ^= p.next();
	}

	Some((buf, a, z, t))
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku::alloc::string::String;
	use aidoku_test::aidoku_test;

	const DATA_B64: &str = include_str!("../tests/fixtures/bw_config_data.b64");
	const EXPECTED_A: &str = "e823a8b2894379f7f4854a9cb2de7d192f41be0a43b1f23697f0a0913b15322b";
	const EXPECTED_Z: &str = "fd2de0473289ce7beb3f78b22553ddce7b21e0f2b58e5b4efb637041debf33e9";
	const EXPECTED_T: &str = "322a51b6b99e53cfeadf7f5355ffb24a815dc00b5d0361f5b82a5c7e9317cef6";

	fn hex(bytes: &[u8]) -> String {
		let mut s = String::new();
		for b in bytes {
			s.push_str(&aidoku::alloc::format!("{b:02x}"));
		}
		s
	}

	#[aidoku_test]
	fn decrypts_config_keys_and_json() {
		let (json, a, z, t) = decrypt_config(DATA_B64).expect("decrypt");
		assert_eq!(hex(&a), EXPECTED_A);
		assert_eq!(hex(&z), EXPECTED_Z);
		assert_eq!(hex(&t), EXPECTED_T);
		let text = core::str::from_utf8(&json).expect("utf-8 json");
		assert!(text.contains("\"item/xhtml/p-001.xhtml\""));
	}
}
