"""Build src/glyph_table.rs from obfuscation fonts served by lightnovel.app.

The site replaces ~3500 common characters in chapter text with other code points
and ships a per-book font whose glyph at the fake code point draws the real
character. The glyph outlines come from one fixed base font; each served font only
permutes them among the pool code points and jitters coordinates by a few units.
Point count and contour end indices survive the jitter, so they identify the base
glyph. This script labels every base glyph once, offline:

1. Code points whose outline is identical in every sample font are never
   obfuscated. The rest form the pool; real characters are also pool members.
2. Each pool glyph is rasterised and matched against the same pool characters
   rendered in a reference CJK font (Microsoft YaHei). A greedy one-to-one
   assignment on cosine similarity labels it.
3. Labels are computed independently per sample font and must agree: a base
   glyph (same contour ends) has to get the same character in every font.

Usage (needs fontTools, numpy, Pillow; at least three sample fonts):
	python build_glyph_table.py <fonts dir with *.ttf / *.woff2> [reference.ttc]
Fetch samples with the hub method GetNovelContent (the `Font` field) and download
`https://api.lightnovel.life/font/<hash>.ttf`. Different books give different fonts.
"""

import glob
import os
import sys
from collections import Counter, defaultdict

import numpy as np
from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.ttLib import TTCollection, TTFont
from PIL import Image, ImageDraw

sys.stdout.reconfigure(encoding="utf-8")
N = 48
REF_POINTS = 16


def ref_indices(n):
	# Must match font::ref_indices in the Rust source.
	return [i * (n - 1) // (REF_POINTS - 1) for i in range(REF_POINTS)]
OUT = os.path.join(os.path.dirname(__file__), "..", "src", "glyph_table.rs")


def outlines(font):
	glyf = font["glyf"]
	res = {}
	for cp, name in font.getBestCmap().items():
		coords, ends, _ = glyf[name].getCoordinates(glyf)
		res[cp] = (tuple(map(tuple, coords)), tuple(ends))
	return res


def polygons(font, name):
	gs = font.getGlyphSet()
	pen = DecomposingRecordingPen(gs)
	gs[name].draw(pen)
	polys, cur = [], []
	for op, args in pen.value:
		if op == "moveTo":
			cur = [args[0]]
		elif op in ("lineTo", "qCurveTo", "curveTo"):
			cur.extend(p for p in args if p is not None)
		elif op in ("closePath", "endPath"):
			if cur:
				polys.append(cur)
			cur = []
	return polys


def raster(polys):
	pts = [p for poly in polys for p in poly]
	if not pts:
		return np.zeros(N * N, np.float32)
	x0, x1 = min(p[0] for p in pts), max(p[0] for p in pts)
	y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
	s = max(x1 - x0, y1 - y0, 1)
	size = 4 * N
	acc = np.zeros((size, size), bool)
	for poly in polys:
		q = [((p[0] - x0) / s * (size - 1), (size - 1) - (p[1] - y0) / s * (size - 1)) for p in poly]
		if len(q) < 3:
			continue
		im = Image.new("1", (size, size), 0)
		ImageDraw.Draw(im).polygon(q, fill=1)
		acc ^= np.array(im, bool)
	a = np.asarray(Image.fromarray((acc * 255).astype(np.uint8)).resize((N, N), Image.BILINEAR), np.float32).ravel()
	a -= a.mean()
	n = np.linalg.norm(a)
	return a / n if n else a


def greedy_assign(sim):
	order = np.argsort(-sim, axis=None)
	rows, cols = np.unravel_index(order, sim.shape)
	used_r, used_c, res = set(), set(), {}
	for i, j in zip(rows.tolist(), cols.tolist()):
		if i in used_r or j in used_c:
			continue
		used_r.add(i)
		used_c.add(j)
		res[i] = j
		if len(res) == sim.shape[0]:
			break
	return res


def sig_hash(ends):
	h = 0x811C9DC5
	for v in (len(ends),) + ends:
		for b in (v & 0xFF, v >> 8):
			h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
	return h


def main():
	font_dir = sys.argv[1]
	ref_path = sys.argv[2] if len(sys.argv) > 2 else r"C:\Windows\Fonts\msyh.ttc"
	paths = sorted(glob.glob(os.path.join(font_dir, "*.ttf")) + glob.glob(os.path.join(font_dir, "*.woff2")))
	if len(paths) < 3:
		sys.exit("need at least three sample fonts")
	fonts = [TTFont(p) for p in paths]
	outs = [outlines(f) for f in fonts]
	base = outs[0]
	if any(set(o) != set(base) for o in outs):
		sys.exit("sample fonts cover different code points")
	pool = sorted(cp for cp in base if any(o[cp] != base[cp] for o in outs[1:]))
	print("code points", len(base), "pool", len(pool))

	ref = TTCollection(ref_path).fonts[0] if ref_path.lower().endswith(".ttc") else TTFont(ref_path)
	ref_cmap = ref.getBestCmap()
	missing = [cp for cp in pool if cp not in ref_cmap]
	if missing:
		sys.exit("reference font lacks %d pool characters" % len(missing))
	ref_mat = np.stack([raster(polygons(ref, ref_cmap[cp])) for cp in pool])

	# Greedy assignment occasionally swaps two look-alike characters (要/耍, 天/夭) in
	# one font. A real character keeps the same contour layout in every font, so each
	# character takes the layout most fonts agree on and the minority samples are dropped.
	labelled = []
	for font, out in zip(fonts, outs):
		cmap = font.getBestCmap()
		mat = np.stack([raster(polygons(font, cmap[cp])) for cp in pool])
		assign = greedy_assign(mat @ ref_mat.T)
		labelled.append([(pool[assign[i]], out[cp]) for i, cp in enumerate(pool)])

	votes = defaultdict(Counter)
	for rows_ in labelled:
		for ch, (_, ends) in rows_:
			votes[ch][ends] += 1
	if len(votes) != len(pool):
		sys.exit("assignment is not one-to-one")
	layout = {}
	for ch, c in votes.items():
		ends, n = c.most_common(1)[0]
		if n * 2 <= len(paths):
			sys.exit("no majority layout for %s: %s" % (chr(ch), dict(c)))
		layout[ch] = ends

	# (ends) -> real char -> list of coordinate arrays, collected over all fonts
	samples = defaultdict(lambda: defaultdict(list))
	dropped = []
	for k, rows_ in enumerate(labelled):
		for ch, (coords, ends) in rows_:
			if layout[ch] != ends:
				dropped.append("%s@%s" % (chr(ch), os.path.basename(paths[k])[:8]))
				continue
			samples[ends][ch].append(np.array(coords, float))
	print("minority labels dropped:", len(dropped), " ".join(dropped))

	rows = []
	refs = []
	errors = 0
	for ends, chars in samples.items():
		h = sig_hash(ends)
		if len(chars) == 1:
			rows.append((h, next(iter(chars)), 0xFFFF))
			continue
		idx = ref_indices(ends[-1] + 1)
		means = {ch: np.mean(arrs, axis=0)[idx] for ch, arrs in chars.items()}
		for ch, arrs in chars.items():
			for arr in arrs:
				best = min(means, key=lambda k: np.abs(arr[idx] - means[k]).sum())
				errors += best != ch
			rows.append((h, ch, len(refs)))
			refs.append(np.rint(means[ch]).astype(int))
	if errors:
		sys.exit("coordinate tie-break misclassified %d samples" % errors)
	hashes = defaultdict(set)
	for ends in samples:
		hashes[sig_hash(ends)].add(ends)
	shared = sum(1 for v in hashes.values() if len(v) > 1)
	if shared:
		sys.exit("%d signature hash collisions between different contour layouts" % shared)
	if any(ch > 0xFFFF for _, ch, _ in rows) or any(cp > 0xFFFF for cp in pool):
		sys.exit("non-BMP character in pool")
	rows.sort()
	print("signatures", len(samples), "rows", len(rows), "tie-break refs", len(refs))

	with open(OUT, "w", encoding="utf-8", newline="\n") as f:
		f.write("// Generated by tools/build_glyph_table.py from %d sample fonts. Do not edit.\n\n" % len(paths))
		f.write("/// Obfuscated code points, sorted.\n")
		f.write("pub static POOL: [u16; %d] = [\n" % len(pool))
		for i in range(0, len(pool), 16):
			f.write("\t" + ", ".join("0x%04X" % cp for cp in pool[i:i + 16]) + ",\n")
		f.write("];\n\n")
		f.write("/// (contour layout hash, real character, index into REFS or 0xFFFF), sorted by hash.\n")
		f.write("pub static SIGS: [(u32, u16, u16); %d] = [\n" % len(rows))
		for h, ch, r in rows:
			f.write("\t(0x%08X, 0x%04X, 0x%04X),\n" % (h, ch, r))
		f.write("];\n\n")
		f.write("/// %d evenly spaced points (font::ref_indices) of the base glyph, for layouts shared by several characters.\n" % REF_POINTS)
		f.write("pub static REFS: [[(i16, i16); %d]; %d] = [\n" % (REF_POINTS, len(refs)))
		for r in refs:
			pts = list(r)
			f.write("\t[" + ", ".join("(%d, %d)" % (x, y) for x, y in pts) + "],\n")
		f.write("];\n")
	print("wrote", os.path.normpath(OUT))


if __name__ == "__main__":
	main()
