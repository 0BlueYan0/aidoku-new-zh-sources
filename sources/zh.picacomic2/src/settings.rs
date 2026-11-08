use aidoku::{
	alloc::{String, Vec},
	imports::defaults::defaults_get,
};

/// The proxy line selected by default (`api_line` in `res/settings.json`).
pub const DEFAULT_LINE: &str = "go2778.com";

/// Selected line: a proxy domain, or `official`.
pub fn get_api_line() -> String {
	defaults_get::<String>("api_line")
		.filter(|value| !value.is_empty())
		.unwrap_or_else(|| String::from(DEFAULT_LINE))
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
