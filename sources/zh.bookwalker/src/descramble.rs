//! BookWalker TW (PUBLUS NFBR) page-image descrambler.
//!
//! Port of `NFBR.a6i.S1V` (hashed file name), `NFBR.a6i.l8m` (per-page seeds),
//! `NFBR.m6v` (xorshift PRNG family) and `NFBR.l8m.a3f` + `NFBR.a6G.l8m` (tile list).
//!
//! Renderer semantics: the app calls
//!   `ctx.drawImage(scrambled, destX,destY,w,h, srcX,srcY,w,h)`
//! i.e. it copies each tile FROM `(destX,destY)` in the scrambled image TO `(srcX,srcY)`
//! in the output. `tiles()` is called with `W = Size.Width + DummyWidth`,
//! `H = Size.Height + DummyHeight`; the output canvas is `Size` = `(W-DummyWidth, H-DummyHeight)`.
//!
//! JS `>>>`/`^`/`<<` are 32-bit; `a3f`/`tiles` also pack values above 2^32 through
//! `Math.floor(x/65536)` and `4294967296*n` — those are ported with u64/i64.

use aidoku::alloc::{format, string::String, vec, vec::Vec};

const N_TRIPLES: u32 = 81;
const N_VARIANTS: u32 = 6;
const N_COMBO: u32 = N_TRIPLES * N_VARIANTS; // 486
const DEFAULT_SEED: u32 = 2463534242;

const TRIPLES: [[u32; 3]; 81] = [
	[1, 3, 10], [1, 5, 16], [1, 5, 19], [1, 9, 29], [1, 11, 6], [1, 11, 16], [1, 19, 3],
	[1, 21, 20], [1, 27, 27], [2, 5, 15], [2, 5, 21], [2, 7, 7], [2, 7, 9], [2, 7, 25],
	[2, 9, 15], [2, 15, 17], [2, 15, 25], [2, 21, 9], [3, 1, 14], [3, 3, 26], [3, 3, 28],
	[3, 3, 29], [3, 5, 20], [3, 5, 22], [3, 5, 25], [3, 7, 29], [3, 13, 7], [3, 23, 25],
	[3, 25, 24], [3, 27, 11], [4, 3, 17], [4, 3, 27], [4, 5, 15], [5, 3, 21], [5, 7, 22],
	[5, 9, 7], [5, 9, 28], [5, 9, 31], [5, 13, 6], [5, 15, 17], [5, 17, 13], [5, 21, 12],
	[5, 27, 8], [5, 27, 21], [5, 27, 25], [5, 27, 28], [6, 1, 11], [6, 3, 17], [6, 17, 9],
	[6, 21, 7], [6, 21, 13], [7, 1, 9], [7, 1, 18], [7, 1, 25], [7, 13, 25], [7, 17, 21],
	[7, 25, 12], [7, 25, 20], [8, 7, 23], [8, 9, 23], [9, 5, 14], [9, 5, 25], [9, 11, 19],
	[9, 21, 16], [10, 9, 21], [10, 9, 25], [11, 7, 12], [11, 7, 16], [11, 17, 13],
	[11, 21, 13], [12, 9, 23], [13, 3, 17], [13, 3, 27], [13, 5, 19], [13, 17, 15],
	[14, 1, 15], [14, 13, 15], [15, 1, 29], [17, 15, 20], [17, 15, 23], [17, 15, 26],
];

/// One xorshift32 variant (`x` and shift results are 32-bit).
fn xorshift(variant: usize, mut x: u32, a: u32, b: u32, c: u32) -> u32 {
	match variant {
		0 => {
			x ^= x << a;
			x ^= x >> b;
			x ^= x << c;
		}
		1 => {
			x ^= x << c;
			x ^= x >> b;
			x ^= x << a;
		}
		2 => {
			x ^= x >> a;
			x ^= x << b;
			x ^= x >> c;
		}
		3 => {
			x ^= x >> c;
			x ^= x << b;
			x ^= x >> a;
		}
		4 => {
			x ^= x << a;
			x ^= x << c;
			x ^= x >> b;
		}
		_ => {
			x ^= x >> a;
			x ^= x >> c;
			x ^= x << b;
		}
	}
	x
}

/// `NFBR.m6v`: a selectable xorshift32 generator with an unbiased `below(n)`.
struct Rng {
	s: u32,
	t: [u32; 3],
	variant: usize,
}

impl Rng {
	fn new() -> Self {
		Rng {
			s: DEFAULT_SEED,
			t: TRIPLES[74],
			variant: 0,
		}
	}

	fn select(&mut self, triple_idx: u32, variant_idx: u32) {
		self.s = DEFAULT_SEED;
		self.t = TRIPLES[triple_idx as usize];
		self.variant = variant_idx as usize;
	}

	fn seed(&mut self, v: u32) {
		self.s = if v == 0 { DEFAULT_SEED } else { v };
	}

	/// Rejection sampler for a uniform value in `0..n`.
	fn below(&mut self, n: u32) -> u32 {
		if n <= 1 {
			return 0;
		}
		let lim = 4294967295u32.wrapping_sub(n);
		let mut s = self.s;
		loop {
			s = xorshift(self.variant, s, self.t[0], self.t[1], self.t[2]);
			let r = s.wrapping_sub(1);
			let l = r % n;
			if !(lim < r.wrapping_sub(l)) {
				self.s = s;
				return l;
			}
		}
	}
}

/// `Fw6`: inside-out Fisher-Yates permutation of `0..n`.
fn perm(rng: &mut Rng, n: u32) -> Vec<i64> {
	let n = n as usize;
	let mut p = vec![0i64; n];
	for j in 0..n {
		let x = rng.below((j + 1) as u32) as usize;
		p[j] = p[x];
		p[x] = j as i64;
	}
	p
}

/// `dw6`.
fn edge(rng: &mut Rng, n: u32) -> u32 {
	if n < 4 {
		rng.below(n + 1)
	} else {
		rng.below(n - 1) + 1
	}
}

/// `cw6`.
fn other(rng: &mut Rng, e: u32, n: u32) -> u32 {
	if n == 0 {
		return 0;
	}
	let z = rng.below(n);
	if z < e {
		z
	} else {
		z + 1
	}
}

/// Sparse array with JS auto-extend semantics: unwritten slots read as "hole".
/// Comparisons against a hole are false (mirrors `x >= NaN` / `x <= NaN`).
struct Sparse {
	v: Vec<Option<i64>>,
}

impl Sparse {
	fn new() -> Self {
		Sparse { v: Vec::new() }
	}

	fn set(&mut self, i: usize, val: i64) {
		if i >= self.v.len() {
			self.v.resize(i + 1, None);
		}
		self.v[i] = Some(val);
	}

	fn get(&self, i: usize) -> Option<i64> {
		self.v.get(i).copied().flatten()
	}

	/// Materialise to a dense vector of at least `len` (holes -> 0). Holes never reach
	/// the final `mw6` reads, and where they would, `x < 0` matches `x < NaN` (both false).
	fn into_dense(self, len: usize) -> Vec<i64> {
		let mut out: Vec<i64> = self.v.iter().map(|x| x.unwrap_or(0)).collect();
		if out.len() < len {
			out.resize(len, 0);
		}
		out
	}
}

fn ge(a: i64, o: Option<i64>) -> bool {
	matches!(o, Some(w) if a >= w)
}

fn le(a: i64, o: Option<i64>) -> bool {
	matches!(o, Some(w) if a <= w)
}

/// `Iw6` (KgW case 54), verbatim. Fills `w06`/`y06` in place.
#[allow(clippy::too_many_arguments)]
fn iw6(rng: &mut Rng, w06: &mut Sparse, y06: &mut Sparse, h06: i64, b06: i64, v06: i64, q06: i64) {
	let mut f06 = v06;
	let mut u06 = q06;
	let mut d06 = h06;
	let mut k06 = b06;
	let mut d_big = 0i64; // D06
	let mut c06 = 0i64;
	while 0 < f06 + u06 {
		let vv = rng.below((f06 + u06) as u32) as i64; // V06
		if vv < f06 {
			if vv < d06 {
				let mut o06 = c06;
				while o06 > 0 && !ge(d_big, w06.get((o06 - 1) as usize)) {
					o06 -= 1;
				}
				let mut f_scan = c06 + u06;
				while f_scan < q06 && !ge(d_big, w06.get(f_scan as usize)) {
					f_scan += 1;
				}
				let val = rng.below((f_scan - o06) as u32) as i64 + o06;
				y06.set(d_big as usize, val);
				d_big += 1;
				d06 -= 1;
			} else {
				let mut o06 = c06;
				while o06 > 0 && !le(d_big + f06, w06.get((o06 - 1) as usize)) {
					o06 -= 1;
				}
				let mut f_scan = c06 + u06;
				while f_scan < q06 && !le(d_big + f06, w06.get(f_scan as usize)) {
					f_scan += 1;
				}
				let val = rng.below((f_scan - o06) as u32) as i64 + o06;
				y06.set((d_big + f06 - 1) as usize, val);
			}
			f06 -= 1;
		} else {
			if vv - f06 < k06 {
				let mut o06 = d_big;
				while o06 > 0 && !ge(c06, y06.get((o06 - 1) as usize)) {
					o06 -= 1;
				}
				let mut f_scan = d_big + f06;
				while f_scan < v06 && !ge(c06, y06.get(f_scan as usize)) {
					f_scan += 1;
				}
				let val = rng.below((f_scan - o06) as u32) as i64 + o06;
				w06.set(c06 as usize, val);
				c06 += 1;
				k06 -= 1;
			} else {
				let mut o06 = d_big;
				while o06 > 0 && !le(c06 + u06, y06.get((o06 - 1) as usize)) {
					o06 -= 1;
				}
				let mut f_scan = d_big + f06;
				while f_scan < v06 && !le(c06 + u06, y06.get(f_scan as usize)) {
					f_scan += 1;
				}
				let val = rng.below((f_scan - o06) as u32) as i64 + o06;
				w06.set((c06 + u06 - 1) as usize, val);
			}
			u06 -= 1;
		}
	}
}

/// `mw6`, verbatim. Produces the interleaved index list consumed by `tiles`.
#[allow(clippy::too_many_arguments)]
fn mw6(
	g_dim: i64,      // Gw6 = vw6
	l_dim: i64,      // Lw6 = qw6
	r_perm: &[i64],  // Rw6 = fw6 (len g_dim*l_dim)
	s_perm: &[i64],  // Sw6 = jw6 (len g_dim)
	x_perm: &[i64],  // Xw6 = nw6 (len l_dim)
	p_arr: &[i64],   // Pw6 = bw6 (len g_dim)
	l_arr: &[i64],   // lw6 = pw6 (len l_dim)
	r_scalar: i64,   // rw6 = Dw6
	j_scalar: i64,   // Jw6 = Qw6
	s_arr: &[i64],   // sw6 = xw6 (len g_dim)
	m_arr: &[i64],   // Mw6 = tw6 (len l_dim)
	a86: i64,        // A86 = Uw6
	z86: i64,        // Z86 = Kw6
) -> Vec<i64> {
	let w86_cap = g_dim + 1; // W86
	let y86_cap = l_dim + 1; // Y86
	let h86 = w86_cap << 1;
	let b86 = y86_cap << 1;
	let mut g86: Vec<i64> = Vec::new();

	for w in 0..g_dim {
		for c in 0..l_dim {
			let z = r_perm[(w + c * g_dim) as usize];
			let n = z % g_dim;
			let e = (z - n) / g_dim;
			let t = if w < m_arr[c as usize] { w } else { w + w86_cap };
			let k = if c < s_arr[w as usize] { c } else { c + y86_cap };
			let hh = if n < l_arr[e as usize] { n } else { n + w86_cap };
			let first = (if e < p_arr[n as usize] { e } else { e + y86_cap }) * h86 + t;
			g86.push(first);
			g86.push(hh * b86 + k);
		}
	}

	g86.push(j_scalar * h86 + a86);
	g86.push(r_scalar * b86 + z86);

	for w in 0..g_dim {
		let c = s_arr[w as usize];
		let n = s_perm[w as usize];
		let hh = if n < r_scalar { n } else { n + w86_cap };
		let e = p_arr[n as usize];
		let t = if w < a86 { w } else { w + w86_cap };
		g86.push(e * h86 + t);
		g86.push(hh * b86 + c);
	}

	for c in 0..l_dim {
		let w = m_arr[c as usize];
		let e = x_perm[c as usize];
		let n = l_arr[e as usize];
		let k = if c < z86 { c } else { c + y86_cap };
		g86.push((if e < j_scalar { e } else { e + y86_cap }) * h86 + w);
		g86.push(n * b86 + k);
	}

	g86
}

#[inline]
fn low32(x: u64) -> u32 {
	x as u32
}

/// `NFBR.l8m.a3f`. `p06`/`l06`/`r06`/`j06` may exceed 2^32 (packed keys).
fn a3f(p06: u64, l06: u64, r06: u64, j06: u64) -> Vec<i64> {
	let mut s06 = Rng::new();
	let m06 = low32(l06) ^ low32(r06) ^ low32(j06);
	let aw6 = (p06 / 65536) as u32;
	let zw6 = (l06 / 65536) as u32;
	let tw6 = (r06 / 65536) as u32;
	let kw6 = (j06 / 65536) as u32;
	let mut cw6 = zw6 ^ tw6 ^ kw6;
	let mut nw6 = aw6 ^ kw6;
	let ew6_0 = low32(p06) ^ low32(l06);
	let zw6x_0 = low32(p06) ^ low32(r06);
	let gw6_0 = low32(p06) ^ low32(j06);

	cw6 >>= 16;
	let ww6 = cw6 % N_VARIANTS;
	let yw6 = (cw6 - ww6) / N_VARIANTS % N_TRIPLES;
	s06.select(yw6, ww6);
	s06.seed(m06);
	let bw6 = s06.below(65536) | (s06.below(65536) << 16);
	let vw6 = zw6 >> 16;
	let qw6 = tw6 >> 16;
	let ew6 = ew6_0 ^ bw6;
	let zw6x = zw6x_0 ^ bw6;
	let gw6 = gw6_0 ^ bw6;
	nw6 = (nw6 >> 16) ^ s06.below(512);
	let vw6v = nw6 % N_VARIANTS;
	let ow6 = (nw6 - vw6v) / N_VARIANTS % N_TRIPLES;
	s06.select(ow6, vw6v);
	s06.seed(ew6);
	let fw6 = perm(&mut s06, vw6 * qw6);
	s06.seed(zw6x);
	let uw6 = edge(&mut s06, vw6);
	let kw6e = edge(&mut s06, qw6);
	let dw6 = other(&mut s06, uw6, vw6);
	let qw6o = other(&mut s06, kw6e, qw6);
	s06.seed(gw6);
	let mut tw6a = Sparse::new();
	let mut xw6a = Sparse::new();
	iw6(&mut s06, &mut tw6a, &mut xw6a, uw6 as i64, kw6e as i64, vw6 as i64, qw6 as i64);
	let jw6 = perm(&mut s06, vw6);
	let nw6p = perm(&mut s06, qw6);
	let mut pw6a = Sparse::new();
	let mut bw6a = Sparse::new();
	iw6(&mut s06, &mut pw6a, &mut bw6a, dw6 as i64, qw6o as i64, vw6 as i64, qw6 as i64);

	let tw6v = tw6a.into_dense(qw6 as usize);
	let xw6v = xw6a.into_dense(vw6 as usize);
	let pw6v = pw6a.into_dense(qw6 as usize);
	let bw6v = bw6a.into_dense(vw6 as usize);

	mw6(
		vw6 as i64,
		qw6 as i64,
		&fw6,
		&jw6,
		&nw6p,
		&bw6v,
		&pw6v,
		dw6 as i64,
		qw6o as i64,
		&xw6v,
		&tw6v,
		uw6 as i64,
		kw6e as i64,
	)
}

/// One descramble rectangle. The app copies the tile FROM `(dest_x,dest_y)` in the
/// scrambled image TO `(src_x,src_y)` in the decoded output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
	pub src_x: i64,
	pub src_y: i64,
	pub dest_x: i64,
	pub dest_y: i64,
	pub width: i64,
	pub height: i64,
}

/// Per-page seeds (`NFBR.a6i.l8m` output), mirroring the JS field names.
pub struct Seeds {
	pub v1p: u32,
	pub w6b: u32,
	pub i8e: u32,
	pub a6l: u32,
	pub j1r: u32, // BlockWidth
	pub u3l: u32, // BlockHeight
	pub y2a: u32, // DummyWidth
	pub r7l: u32, // DummyHeight
}

#[allow(clippy::too_many_arguments)]
fn emit_range(
	out: &mut Vec<Tile>,
	list: &[i64],
	from: usize,
	to: usize,
	width: i64,
	height: i64,
	bw: i64,
	bh: i64,
	nx: i64,
	ny: i64,
	zx: i64,
	zy: i64,
	ox: i64,
	oy: i64,
) {
	if width == 0 || height == 0 {
		return;
	}
	let mut k = from;
	while k < to {
		let t = list[k];
		k += 1;
		let x = list[k];
		k += 1;
		let jj = x % zy;
		let nn = (x - jj) / zy;
		let ii = t % zx;
		let bb = (t - ii) / zx;
		let src_x = ii * bw - if nx < ii { ox } else { 0 };
		let src_y = jj * bh - if ny < jj { oy } else { 0 };
		let dest_x = nn * bw - if nx < nn { ox } else { 0 };
		let dest_y = bb * bh - if ny < bb { oy } else { 0 };
		out.push(Tile {
			src_x,
			src_y,
			dest_x,
			dest_y,
			width,
			height,
		});
	}
}

/// `NFBR.a6G.l8m`: the full tile list. `w = Size.Width + DummyWidth`,
/// `h = Size.Height + DummyHeight`.
pub fn tiles(pg: &Seeds, w: i64, h: i64) -> Vec<Tile> {
	let bw = pg.j1r as i64;
	let bh = pg.u3l as i64;
	let s1 = pg.w6b as u64;
	let s2 = pg.i8e as u64;
	let s3 = pg.a6l as u64;
	let vv = pg.v1p as i64;
	let nx = w / bw;
	let ny = h / bh;
	let rx = w % bw;
	let ry = h % bh;
	let zx = (nx + 1) << 1;
	let zy = (ny + 1) << 1;
	let ox = (nx + 1) * bw - rx;
	let oy = (ny + 1) * bh - ry;

	let mut rng = Rng::new();
	let b = (vv as u32) ^ (nx as u32) ^ (ny as u32);
	let vi = b % N_VARIANTS;
	let ti = (b - vi) / N_VARIANTS % N_TRIPLES;
	rng.select(ti, vi);
	rng.seed((s1 as u32) ^ (s2 as u32) ^ (s3 as u32));
	let mut key: u64 = rng.below(65536) as u64;
	key += 65536u64 * rng.below(65536) as u64;
	key += 4294967296u64 * rng.below(512) as u64;

	let list = a3f(
		key,
		4294967296u64 * nx as u64 + s1,
		4294967296u64 * ny as u64 + s2,
		4294967296u64 * vv as u64 + s3,
	);

	let mut out = Vec::new();
	let mut a = 0usize;
	let mut bnd = ((nx * ny) << 1) as usize;
	emit_range(&mut out, &list, a, bnd, bw, bh, bw, bh, nx, ny, zx, zy, ox, oy);
	a = bnd;
	bnd += 2;
	emit_range(&mut out, &list, a, bnd, rx, ry, bw, bh, nx, ny, zx, zy, ox, oy);
	a = bnd;
	bnd += (nx << 1) as usize;
	emit_range(&mut out, &list, a, bnd, bw, ry, bw, bh, nx, ny, zx, zy, ox, oy);
	a = bnd;
	bnd += (ny << 1) as usize;
	emit_range(&mut out, &list, a, bnd, rx, bh, bw, bh, nx, ny, zx, zy, ox, oy);
	out
}

/// Content-wide keys derived from the final A/Z/T (`N1h` hooks).
struct ContentKeys {
	b6o: u64,
	e1u: u32,
	l7g: u32,
	w2u: u32,
	u2g: [u8; 32],
}

fn sum32(a: &[u8; 32]) -> u64 {
	a.iter().map(|&x| x as u64).sum()
}

fn xor_words(a: &[u8; 32]) -> u32 {
	let mut r = 0u32;
	let mut i = 0;
	while i < 32 {
		r ^= (a[i] as u32) << 24 ^ (a[i + 1] as u32) << 16 ^ (a[i + 2] as u32) << 8 ^ (a[i + 3] as u32);
		i += 4;
	}
	r
}

fn content_keys(a: &[u8; 32], z: &[u8; 32], t: &[u8; 32]) -> ContentKeys {
	let mut u2g = [0u8; 32];
	for i in 0..32 {
		u2g[i] = a[i] ^ z[i] ^ t[i];
	}
	ContentKeys {
		b6o: sum32(a) + sum32(z) + sum32(t),
		e1u: xor_words(a),
		l7g: xor_words(z),
		w2u: xor_words(t),
		u2g,
	}
}

/// The numeric fields of one `Page` object needed for descrambling.
pub struct PageInfo {
	pub no: u64,
	pub size_width: i64,
	pub size_height: i64,
	pub block_width: u32,
	pub block_height: u32,
	pub dummy_width: u32,
	pub dummy_height: u32,
	pub ns: u32,
	pub ps: u32,
	pub rs: u32,
}

/// `NFBR.a6i.l8m` (Page.n4q): per-page seeds. `k9j` is the xhtml path,
/// `file_name` is `String(Page.No)`.
fn page_seeds(ck: &ContentKeys, k9j: &str, file_name: &str, info: &PageInfo) -> Seeds {
	let mut k: u64 = 47;
	for ch in k9j.encode_utf16() {
		k += ch as u64;
	}
	for ch in file_name.encode_utf16() {
		k += ch as u64;
	}
	k += ck.b6o;
	let s0 = (k & 255) as u32;
	let mut s = s0;
	s |= s << 8;
	s |= s << 16;
	Seeds {
		v1p: (k % N_COMBO as u64) as u32,
		w6b: s ^ ck.e1u ^ info.ns,
		i8e: s ^ ck.l7g ^ info.ps,
		a6l: s ^ ck.w2u ^ info.rs,
		j1r: info.block_width,
		u3l: info.block_height,
		y2a: info.dummy_width,
		r7l: info.dummy_height,
	}
}

/// `NFBR.a6i.S1V` (Page.P1j): the hashed base file name, e.g. `1055836d080ba885d1`.
fn hashed_name(ck: &ContentKeys, k9j: &str, file_name: &str) -> String {
	let n = &ck.u2g;
	let prefix = match file_name.parse::<i64>() {
		Ok(no) if (0..16383).contains(&no) => {
			let h = format!("{no:x}");
			format!("{:x}{}", h.len(), h)
		}
		_ => format!("0{file_name}"),
	};

	let wmz = format!("{k9j}/");
	let emz = wmz.encode_utf16().count();
	let zmz = file_name.encode_utf16().count();
	let gmz = n.len();
	let wmz_full: Vec<u16> = wmz.encode_utf16().chain(file_name.encode_utf16()).collect();
	let ymz = emz + zmz;
	let hmz = zmz << 1;
	let bmz = (1 + emz) << 1;
	let vmz = (1 + ymz) << 1;

	let mut q: Vec<i64> = vec![0i64; vmz];
	let mut zi = 0usize;
	q[zi] = 0;
	zi += 1;
	q[zi] = 59;
	zi += 1;
	for &unit in wmz_full.iter().take(ymz) {
		let hh = unit as u32;
		q[zi] = (hh >> 8) as i64;
		zi += 1;
		q[zi] = (hh & 255) as i64;
		zi += 1;
	}

	let mut o = hmz + vmz + vmz;
	let mut rounds = 3i64;
	while o < 256 {
		rounds += 1;
		o += vmz;
	}

	let mut kk: u64 = 1670739;
	let mut dd: u64 = 1282576;
	let mut cc: u64 = 2237221;
	let mut a_idx = bmz;
	let mut z_idx = 0usize;
	let mut t_round = 0i64;
	loop {
		while a_idx < vmz {
			let c1 = cc ^ ((q[a_idx] as u64) ^ (n[z_idx] as u64));
			a_idx += 1;
			z_idx += 1;
			let d = 435u64 * c1;
			let u = 435u64 * dd + ((7 & c1) << 18) + (d >> 22);
			let f = 435u64 * kk + ((3 & dd) << 19) + ((4194296 & c1) >> 3) + (u >> 21);
			cc = 4194303 & d;
			dd = 2097151 & u;
			kk = 2097151 & f;
			if gmz <= z_idx {
				z_idx = 0;
			}
		}
		t_round += 1;
		if t_round >= rounds {
			break;
		}
		a_idx = 0;
	}

	let out_bytes = [
		((kk >> 13) as u8) ^ n[0],
		(((kk >> 5) & 255) as u8) ^ n[1],
		((((31 & kk) << 3) | (dd >> 18)) as u8) ^ n[2],
		(((dd >> 10) & 255) as u8) ^ n[3],
		(((dd >> 2) & 255) as u8) ^ n[4],
		((((3 & dd) << 6) | (cc >> 16)) as u8) ^ n[5],
		(((cc >> 8) & 255) as u8) ^ n[6],
		((255 & cc) as u8) ^ n[7],
	];
	let mut s = prefix;
	for b in out_bytes {
		s.push_str(&format!("{b:02x}"));
	}
	s
}

// ---- minimal JSON field extraction (only after decrypt, so kept tiny) ----

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
	if needle.is_empty() || from > hay.len() {
		return None;
	}
	let end = hay.len().checked_sub(needle.len())?;
	(from..=end).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn match_braces(hay: &[u8], from: usize) -> Option<(usize, usize)> {
	let mut i = from;
	while i < hay.len() && hay[i] != b'{' {
		i += 1;
	}
	if i >= hay.len() {
		return None;
	}
	let start = i;
	let mut depth = 0i32;
	while i < hay.len() {
		match hay[i] {
			b'{' => depth += 1,
			b'}' => {
				depth -= 1;
				if depth == 0 {
					return Some((start, i + 1));
				}
			}
			_ => {}
		}
		i += 1;
	}
	None
}

fn uint_field(scope: &[u8], name: &str) -> Option<u64> {
	let needle = format!("\"{name}\":");
	let p = find(scope, needle.as_bytes(), 0)?;
	let mut i = p + needle.len();
	while i < scope.len() && scope[i] == b' ' {
		i += 1;
	}
	let mut v = 0u64;
	let mut any = false;
	while i < scope.len() && scope[i].is_ascii_digit() {
		v = v * 10 + (scope[i] - b'0') as u64;
		i += 1;
		any = true;
	}
	any.then_some(v)
}

fn size_field(page: &[u8], name: &str) -> Option<u64> {
	let sp = find(page, b"\"Size\":", 0)?;
	let (start, end) = match_braces(page, sp)?;
	uint_field(&page[start..end], name)
}

fn slice_page_object<'a>(json: &'a [u8], xhtml_key: &str) -> Option<&'a [u8]> {
	// Match the object key `"<path>":`, not the same path as a `"file"` /
	// `"original-file-path"` value inside `configuration.contents` (those are followed
	// by `,`, only the real key is followed by `:`).
	let key_needle = format!("\"{xhtml_key}\":");
	let kpos = find(json, key_needle.as_bytes(), 0)?;
	let ppos = find(json, b"\"Page\":", kpos)?;
	let (start, end) = match_braces(json, ppos)?;
	Some(&json[start..end])
}

/// Parse the `Page` object for `xhtml_key` out of the decrypted config JSON.
pub fn parse_page_info(json: &[u8], xhtml_key: &str) -> Option<PageInfo> {
	let page = slice_page_object(json, xhtml_key)?;
	Some(PageInfo {
		no: uint_field(page, "No")?,
		size_width: size_field(page, "Width")? as i64,
		size_height: size_field(page, "Height")? as i64,
		block_width: uint_field(page, "BlockWidth")? as u32,
		block_height: uint_field(page, "BlockHeight")? as u32,
		dummy_width: uint_field(page, "DummyWidth")? as u32,
		dummy_height: uint_field(page, "DummyHeight")? as u32,
		ns: uint_field(page, "NS")? as u32,
		ps: uint_field(page, "PS")? as u32,
		rs: uint_field(page, "RS")? as u32,
	})
}

/// Convenience: given decrypted config JSON, one xhtml key and the final A/Z/T keys,
/// return the hashed page file base name and its tile list.
/// The hashed file name and tile list for one page, from an already-parsed
/// [`PageInfo`], so the image processor can recompute tiles without re-parsing the
/// whole config.
pub fn page_file_and_tiles_from_info(
	info: &PageInfo,
	xhtml_key: &str,
	a: &[u8; 32],
	z: &[u8; 32],
	t: &[u8; 32],
) -> (String, Vec<Tile>) {
	let ck = content_keys(a, z, t);
	let file_name = format!("{}", info.no);
	let seeds = page_seeds(&ck, xhtml_key, &file_name, info);
	let file = hashed_name(&ck, xhtml_key, &file_name);
	let w = info.size_width + seeds.y2a as i64;
	let h = info.size_height + seeds.r7l as i64;
	let tl = tiles(&seeds, w, h);
	(file, tl)
}

/// The hashed file base name only (for building the image URL), without the tiles.
pub fn page_file_name(
	info: &PageInfo,
	xhtml_key: &str,
	a: &[u8; 32],
	z: &[u8; 32],
	t: &[u8; 32],
) -> String {
	let ck = content_keys(a, z, t);
	hashed_name(&ck, xhtml_key, &format!("{}", info.no))
}

/// The ordered list of page xhtml keys from `configuration.contents`. That array is
/// the reading order (cover first, then p-001..), authoritative over sorting names.
pub fn page_order(json: &[u8]) -> Vec<String> {
	let mut order: Vec<String> = Vec::new();
	let Some(contents_at) = find_key(json, b"\"contents\"") else {
		return order;
	};
	// Scan from the start of the contents array to its closing bracket, pulling each
	// object's "file" value in order.
	let Some(array_start) = json[contents_at..].iter().position(|&b| b == b'[') else {
		return order;
	};
	let start = contents_at + array_start;
	let mut depth = 0i32;
	let mut i = start;
	while i < json.len() {
		match json[i] {
			b'[' => depth += 1,
			b']' => {
				depth -= 1;
				if depth == 0 {
					break;
				}
			}
			// "file":"..."
			b'"' if json[i..].starts_with(b"\"file\"") => {
				if let Some(value) = read_string_after_colon(json, i) {
					order.push(value);
				}
			}
			_ => {}
		}
		i += 1;
	}
	order
}

/// Byte offset of a top-level-ish key needle (e.g. `"contents"`). Simple substring
/// search; the config's structure makes a full parser unnecessary.
fn find_key(json: &[u8], needle: &[u8]) -> Option<usize> {
	json.windows(needle.len()).position(|w| w == needle)
}

/// Given the offset of a `"key"` token, read the quoted string value that follows its
/// colon. Skips the key's own closing quote, whitespace and the colon.
fn read_string_after_colon(json: &[u8], key_at: usize) -> Option<String> {
	let mut i = key_at + 1;
	// past the key text and its closing quote
	while i < json.len() && json[i] != b'"' {
		i += 1;
	}
	i += 1; // closing quote of the key
	while i < json.len() && (json[i] == b' ' || json[i] == b':' || json[i] == b'\t') {
		i += 1;
	}
	if i >= json.len() || json[i] != b'"' {
		return None;
	}
	i += 1;
	let mut out = String::new();
	while i < json.len() && json[i] != b'"' {
		out.push(json[i] as char);
		i += 1;
	}
	Some(out)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::crypto;
	use aidoku_test::aidoku_test;

	const DATA_B64: &str = include_str!("../tests/fixtures/bw_config_data.b64");
	const P001_TILES: &str = include_str!("../tests/fixtures/p001_tiles.txt");

	const EXPECTED_A: [u8; 32] = [
		0xe8, 0x23, 0xa8, 0xb2, 0x89, 0x43, 0x79, 0xf7, 0xf4, 0x85, 0x4a, 0x9c, 0xb2, 0xde, 0x7d,
		0x19, 0x2f, 0x41, 0xbe, 0x0a, 0x43, 0xb1, 0xf2, 0x36, 0x97, 0xf0, 0xa0, 0x91, 0x3b, 0x15,
		0x32, 0x2b,
	];
	const EXPECTED_Z: [u8; 32] = [
		0xfd, 0x2d, 0xe0, 0x47, 0x32, 0x89, 0xce, 0x7b, 0xeb, 0x3f, 0x78, 0xb2, 0x25, 0x53, 0xdd,
		0xce, 0x7b, 0x21, 0xe0, 0xf2, 0xb5, 0x8e, 0x5b, 0x4e, 0xfb, 0x63, 0x70, 0x41, 0xde, 0xbf,
		0x33, 0xe9,
	];
	const EXPECTED_T: [u8; 32] = [
		0x32, 0x2a, 0x51, 0xb6, 0xb9, 0x9e, 0x53, 0xcf, 0xea, 0xdf, 0x7f, 0x53, 0x55, 0xff, 0xb2,
		0x4a, 0x81, 0x5d, 0xc0, 0x0b, 0x5d, 0x03, 0x61, 0xf5, 0xb8, 0x2a, 0x5c, 0x7e, 0x93, 0x17,
		0xce, 0xf6,
	];

	fn expected_tiles() -> Vec<Tile> {
		P001_TILES
			.lines()
			.filter(|l| !l.trim().is_empty())
			.map(|line| {
				let n: Vec<i64> = line
					.split_whitespace()
					.map(|f| f.parse::<i64>().unwrap())
					.collect();
				Tile {
					src_x: n[0],
					src_y: n[1],
					dest_x: n[2],
					dest_y: n[3],
					width: n[4],
					height: n[5],
				}
			})
			.collect()
	}

	#[aidoku_test]
	fn p001_seeds_match() {
		let (json, a, z, t) = crypto::decrypt_config(DATA_B64).expect("decrypt");
		assert_eq!(a, EXPECTED_A);
		assert_eq!(z, EXPECTED_Z);
		assert_eq!(t, EXPECTED_T);
		let info = parse_page_info(&json, "item/xhtml/p-001.xhtml").expect("page info");
		let ck = content_keys(&a, &z, &t);
		let seeds = page_seeds(&ck, "item/xhtml/p-001.xhtml", "0", &info);
		assert_eq!(seeds.v1p, 331);
		assert_eq!(seeds.w6b, 1760755336);
		assert_eq!(seeds.i8e, 2759209329);
		assert_eq!(seeds.a6l, 3333769491);
		assert_eq!(seeds.j1r, 32);
		assert_eq!(seeds.u3l, 32);
		assert_eq!(seeds.y2a, 2);
		assert_eq!(seeds.r7l, 0);
	}

	#[aidoku_test]
	fn p001_file_name_and_tiles_match() {
		let (json, a, z, t) = crypto::decrypt_config(DATA_B64).expect("decrypt");
		let info = parse_page_info(&json, "item/xhtml/p-001.xhtml").expect("page info");
		let (file, got) = page_file_and_tiles_from_info(&info, "item/xhtml/p-001.xhtml", &a, &z, &t);
		assert_eq!(file, "1055836d080ba885d1");
		let want = expected_tiles();
		assert_eq!(got.len(), want.len(), "tile count");
		for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
			assert_eq!(g, w, "tile {i} differs");
		}
	}

	fn parse_tiles(text: &str) -> Vec<Tile> {
		text.lines()
			.filter(|l| !l.trim().is_empty())
			.map(|line| {
				let n: Vec<i64> = line
					.split([',', ' ', '\t'])
					.filter(|f| !f.is_empty())
					.map(|f| f.parse::<i64>().unwrap())
					.collect();
				Tile {
					src_x: n[0],
					src_y: n[1],
					dest_x: n[2],
					dest_y: n[3],
					width: n[4],
					height: n[5],
				}
			})
			.collect()
	}

	/// Each distinct page-name checksum yields a distinct tile map; p-001 is verified
	/// above, and these cover the other ten (V1p 332..341) so a seed-dependent port bug
	/// cannot hide on pages the first test never exercises.
	#[aidoku_test]
	fn all_seed_variants_match() {
		let (json, a, z, t) = crypto::decrypt_config(DATA_B64).expect("decrypt");
		let cases = [
			("item/xhtml/p-002.xhtml", include_str!("../tests/fixtures/p-002_tiles.txt")),
			("item/xhtml/p-003.xhtml", include_str!("../tests/fixtures/p-003_tiles.txt")),
			("item/xhtml/p-004.xhtml", include_str!("../tests/fixtures/p-004_tiles.txt")),
			("item/xhtml/p-005.xhtml", include_str!("../tests/fixtures/p-005_tiles.txt")),
			("item/xhtml/p-006.xhtml", include_str!("../tests/fixtures/p-006_tiles.txt")),
			("item/xhtml/p-007.xhtml", include_str!("../tests/fixtures/p-007_tiles.txt")),
			("item/xhtml/p-008.xhtml", include_str!("../tests/fixtures/p-008_tiles.txt")),
			("item/xhtml/p-009.xhtml", include_str!("../tests/fixtures/p-009_tiles.txt")),
			("item/xhtml/p-019.xhtml", include_str!("../tests/fixtures/p-019_tiles.txt")),
			("item/xhtml/p-029.xhtml", include_str!("../tests/fixtures/p-029_tiles.txt")),
		];
		let mut fails: Vec<String> = Vec::new();
		for (key, fixture) in cases {
			let info = parse_page_info(&json, key).expect("page info");
			let (_f, got) = page_file_and_tiles_from_info(&info, key, &a, &z, &t);
			let want = parse_tiles(fixture);
			if got.len() != want.len() {
				fails.push(format!("{key}: len {} vs {}", got.len(), want.len()));
				continue;
			}
			if let Some(i) = got.iter().zip(want.iter()).position(|(g, w)| g != w) {
				fails.push(format!("{key}: diff@{i} got {:?} want {:?}", got[i], want[i]));
			}
		}
		assert!(fails.is_empty(), "{}", fails.join(" | "));
	}

	#[aidoku_test]
	fn p004_page_info_and_seeds() {
		let (json, a, z, t) = crypto::decrypt_config(DATA_B64).expect("decrypt");
		let info = parse_page_info(&json, "item/xhtml/p-004.xhtml").expect("info");
		assert_eq!(
			(info.no, info.ns, info.ps, info.rs),
			(0, 1643678398, 2967969180, 3443573305),
			"page info"
		);
		let ck = content_keys(&a, &z, &t);
		let seeds = page_seeds(&ck, "item/xhtml/p-004.xhtml", "0", &info);
		assert_eq!(
			(seeds.v1p, seeds.w6b, seeds.i8e, seeds.a6l),
			(334, 3298095290, 404663434, 1291156589),
			"seeds"
		);
	}

	#[aidoku_test]
	fn p004_a3f_first() {
		// p-004: key from tiles() RNG; l06/r06/j06 = 2^32*{nx=35,ny=50,V1p=334} + {w6b,i8e,a6l}.
		let list = a3f(1355461849185, 153621950650, 215153028234, 1435810233453);
		assert_eq!(list.len(), 3672, "list length");
		let got: Vec<i64> = list.iter().take(8).copied().collect();
		assert_eq!(got, [1080, 4794, 432, 6631, 576, 1634, 6480, 309], "a3f first 8");
	}

	#[aidoku_test]
	fn page_order_follows_contents() {
		let (json, _a, _z, _t) = crypto::decrypt_config(DATA_B64).expect("decrypt");
		let order = page_order(&json);
		assert_eq!(order.len(), 174, "page count");
		assert_eq!(order[0], "item/xhtml/p-cover.xhtml");
		assert_eq!(order[1], "item/xhtml/p-001.xhtml");
		assert_eq!(order[173], "item/xhtml/p-173.xhtml");
	}
}
