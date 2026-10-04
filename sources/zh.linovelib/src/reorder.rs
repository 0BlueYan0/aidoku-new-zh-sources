//! The reading order of a chapter page's paragraphs.
//!
//! The site serves every chapter page with its `<p>` paragraphs shuffled, and the
//! obfuscated tail of `chapterlog.js` (v1006c1.6, deobfuscated 2026-10-04) puts them
//! back in the browser. Only the non-empty `<p>` children of `#acontent` take part;
//! images, line breaks and ad blocks keep their places. The first 20 paragraphs never
//! move. The rest went through a Fisher-Yates shuffle driven by a linear congruential
//! generator seeded from the chapter id alone (every `_N` page of a chapter uses the
//! same seed), and paragraph `i` of the served page belongs at position `order[i]`.
//!
//! Checked against the browser on seven pages of both sites: the restored order hashed
//! the same as the paragraphs the page shows.

use aidoku::alloc::Vec;

const FIXED: usize = 20;
const MULTIPLIER: u64 = 9302;
const INCREMENT: u64 = 49397;
const MODULUS: u64 = 233_280;

/// `order[i]` is the reading position of the `i`-th paragraph as served.
pub fn order(count: usize, chapter_id: u64) -> Vec<usize> {
	let mut order: Vec<usize> = (0..count).collect();
	if count <= FIXED {
		return order;
	}
	let rest = &mut order[FIXED..];
	let mut seed = chapter_id * 126 + 232;
	for i in (1..rest.len()).rev() {
		seed = (seed * MULTIPLIER + INCREMENT) % MODULUS;
		// The script computes `Math.floor(seed / 233280 * (i + 1))` in doubles; the
		// integer form `seed * (i + 1) / 233280` rounds differently on some steps.
		let j = (seed as f64 / MODULUS as f64 * (i + 1) as f64) as usize;
		rest.swap(i, j);
	}
	order
}

/// The paragraphs in reading order.
pub fn restore<T>(served: Vec<T>, chapter_id: u64) -> Vec<T> {
	let order = order(served.len(), chapter_id);
	let mut slots: Vec<Option<T>> = served.iter().map(|_| None).collect();
	for (item, position) in served.into_iter().zip(order) {
		slots[position] = Some(item);
	}
	slots.into_iter().flatten().collect()
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn short_pages_keep_their_order() {
		assert_eq!(order(20, 403), (0..20).collect::<Vec<usize>>());
		assert_eq!(restore(Vec::from(["a", "b"]), 403), Vec::from(["a", "b"]));
	}

	#[aidoku_test]
	fn is_a_permutation_that_fixes_the_first_twenty() {
		for (count, id) in [(21, 1), (122, 403), (500, 154933)] {
			let order = order(count, id);
			assert_eq!(order[..FIXED], (0..FIXED).collect::<Vec<usize>>()[..]);
			let mut sorted = order.clone();
			sorted.sort_unstable();
			assert_eq!(sorted, (0..count).collect::<Vec<usize>>());
		}
	}
}
