use aidoku::{
	alloc::{String, Vec},
	imports::defaults::defaults_get,
};

use crate::lines;

/// Used when neither PicaWeb nor a test has given a line yet.
pub const DEFAULT_LINE: &str = "go2778.com";
pub const OFFICIAL: &str = "official";
pub const LINE_KEY: &str = "api_line";
pub const CUSTOM_KEY: &str = "customBaseUrl";

/// The reader's own proxy domain from the text field, reduced to a bare host. Accepts
/// `go2778.com`, `https://go2778.com/` and `https://picaapi.go2778.com`; anything that is
/// not a host name is ignored.
pub fn custom_line() -> Option<String> {
	let value = defaults_get::<String>(CUSTOM_KEY)?;
	normalize_custom(&value)
}

pub fn normalize_custom(value: &str) -> Option<String> {
	let value = value.trim();
	let rest = value.find("://").map_or(value, |index| &value[index + 3..]);
	let host = rest.split(['/', '?', '#']).next().unwrap_or("");
	let host = host.to_ascii_lowercase();
	let host = host.strip_prefix("picaapi.").unwrap_or(&host);
	lines::is_domain(host).then(|| String::from(host))
}

/// What the reader picked in the line menu, if anything.
fn chosen_line() -> Option<String> {
	defaults_get::<String>(LINE_KEY).filter(|value| !value.is_empty())
}

/// Whether the line in use comes from the PicaWeb list (not the official line, not the
/// reader's own domain).
pub fn uses_proxy_list() -> bool {
	custom_line().is_none() && chosen_line().as_deref() != Some(OFFICIAL)
}

/// Line in use: the reader's own domain, else the official line when picked, else the
/// picked proxy line while it still answers, else the first line that answered.
pub fn get_api_line() -> String {
	if let Some(custom) = custom_line() {
		return custom;
	}
	match chosen_line() {
		Some(line) if line == OFFICIAL => line,
		Some(line) if lines::usable(&line) => line,
		_ => lines::best_line(),
	}
}

pub fn get_app_channel() -> String {
	defaults_get::<String>("appChannel").unwrap_or_else(|| String::from("3"))
}

pub fn get_image_quality() -> String {
	defaults_get::<String>("imageQuality").unwrap_or_else(|| String::from("original"))
}

pub fn get_blocklist() -> Vec<String> {
	defaults_get::<Vec<String>>("blockGenres").unwrap_or_default()
}

#[cfg(test)]
pub fn clear_line_choice() {
	use aidoku::imports::defaults::{defaults_set, DefaultValue};
	for key in [LINE_KEY, CUSTOM_KEY] {
		defaults_set(key, DefaultValue::String(String::new()));
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::imports::defaults::{defaults_set, DefaultValue};
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn custom_domains() {
		assert_eq!(
			normalize_custom("go2778.com").as_deref(),
			Some("go2778.com")
		);
		assert_eq!(
			normalize_custom(" https://picaapi.New-Line.top/status.json ").as_deref(),
			Some("new-line.top")
		);
		assert_eq!(
			normalize_custom("http://acbbb.com/").as_deref(),
			Some("acbbb.com")
		);
		assert_eq!(normalize_custom(""), None);
		assert_eq!(normalize_custom("not a domain"), None);
	}

	#[aidoku_test]
	fn line_in_use() {
		lines::clear();
		clear_line_choice();
		assert_eq!(get_api_line(), "go2778.com");
		defaults_set(LINE_KEY, DefaultValue::String(String::from("acbbb.com")));
		assert_eq!(get_api_line(), "acbbb.com");
		// A picked line that is not on the list any more gives way to the best one.
		defaults_set(LINE_KEY, DefaultValue::String(String::from("gone.com")));
		assert_eq!(get_api_line(), "go2778.com");
		defaults_set(LINE_KEY, DefaultValue::String(String::from(OFFICIAL)));
		assert_eq!(get_api_line(), OFFICIAL);
		assert!(!uses_proxy_list());
		// The reader's own domain wins over the menu.
		defaults_set(CUSTOM_KEY, DefaultValue::String(String::from("mine.com")));
		assert_eq!(get_api_line(), "mine.com");
		clear_line_choice();
		lines::clear();
	}
}
