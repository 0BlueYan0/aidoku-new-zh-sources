//! Blocked keywords, after the official zh.jmcomic `BlockState`: case-insensitive, a work is
//! hidden when its title, author or any tag contains a keyword. An entry written as a book id
//! (`NID12345`, the form the official app shows, or bare digits) hides that one book.
//!
//! The site has no book blocking of its own (its web client only blocks community posts and
//! users), so lists are filtered here, after the response arrives.

use aidoku::{
	alloc::{String, Vec},
	imports::defaults::defaults_get,
};

pub const BLOCKED_KEY: &str = "blockedKeywords";

#[derive(Default)]
pub struct BlockState {
	keywords: Vec<String>,
	ids: Vec<String>,
}

impl BlockState {
	pub fn load() -> Self {
		Self::new(defaults_get::<Vec<String>>(BLOCKED_KEY).unwrap_or_default())
	}

	pub fn new(entries: impl IntoIterator<Item = String>) -> Self {
		let mut state = Self::default();
		for entry in entries {
			let entry = entry.trim().to_lowercase();
			if entry.is_empty() {
				continue;
			}
			let id = entry.strip_prefix("nid").unwrap_or(&entry);
			let (list, value) = if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
				(&mut state.ids, String::from(id))
			} else {
				(&mut state.keywords, entry)
			};
			if !list.contains(&value) {
				list.push(value);
			}
		}
		state
	}

	pub fn is_empty(&self) -> bool {
		self.keywords.is_empty() && self.ids.is_empty()
	}

	pub fn blocks<'a>(&self, id: &str, fields: impl IntoIterator<Item = &'a str>) -> bool {
		if self.ids.iter().any(|blocked| blocked == id) {
			return true;
		}
		if self.keywords.is_empty() {
			return false;
		}
		fields.into_iter().any(|field| {
			let field = field.to_lowercase();
			self.keywords.iter().any(|kw| field.contains(kw.as_str()))
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku::alloc::vec;
	use aidoku_test::aidoku_test;

	fn state(entries: &[&str]) -> BlockState {
		BlockState::new(entries.iter().map(|s| String::from(*s)))
	}

	#[aidoku_test]
	fn keywords_match_inside_fields_ignoring_case() {
		let block = state(&["  NTR ", "巨乳"]);
		assert!(block.blocks("1", ["標題", "作者", "ntr"]));
		assert!(block.blocks("1", ["超巨乳合集"]));
		assert!(!block.blocks("1", ["純愛", "姐妹"]));
	}

	#[aidoku_test]
	fn ids_block_one_book() {
		let block = state(&["NID123", "456"]);
		assert!(block.blocks("123", ["純愛"]));
		assert!(block.blocks("456", Vec::<&str>::new()));
		assert!(!block.blocks("1234", ["NID123"]));
	}

	#[aidoku_test]
	fn empty_entries_are_ignored() {
		let block = state(&["", "   "]);
		assert!(block.is_empty());
		assert!(!block.blocks("1", vec!["anything"]));
	}
}
