//! Keyword search, on the Simplified site: tw has none of its own.
//!
//! The site lets a search through only with three cookies, collected the way its page
//! script does (measured 2026-10-04):
//! 1. `search_guard=css` sets `jieqiSearchCss`.
//! 2. `search_guard=js` answers a script that sets `jieqiSearchJs`.
//! 3. `search_guard=redeem`, sent with the first cookie, sets `jieqiSearchTicket`.
//!
//! A ticket admits one search; a search without a valid one answers 200 with an empty
//! body. So every search runs all four requests. The site converts Traditional keywords
//! to Simplified and matches titles and author names; results are in Simplified.

use aidoku::{
	alloc::{format, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{html::Html, std::current_date},
	prelude::*,
	MangaPageResult, Result,
};

use crate::helper::{base_url, parse_details, parse_link, parse_manga_list, request, CN_URL};

/// `name=value` of the cookie `name` in a `Set-Cookie` header or a script.
fn cookie(text: &str, name: &str) -> Option<String> {
	let start = text.find(&format!("{name}="))?;
	let pair = &text[start..];
	let end = pair.find([';', '"', ',', '\n']).unwrap_or(pair.len());
	Some(String::from(pair[..end].trim()))
}

/// The guard ties its cookies to the user agent they were issued to (a ticket taken
/// with another agent's cookies is refused), so every step goes through `request` with
/// the same agent as the search itself.
fn set_cookie(url: &str, cookies: &str, name: &str) -> Result<String> {
	let response = request(url)?.header("Cookie", cookies).send()?;
	response
		.get_header("Set-Cookie")
		.and_then(|header: String| cookie(&header, name))
		.ok_or_else(|| error!("[linovelib] search: no {name}"))
}

fn ticket() -> Result<String> {
	let css = set_cookie(&format!("{CN_URL}/search.html?search_guard=css"), "night=0", "jieqiSearchCss")?;
	let script = request(&format!("{CN_URL}/search.html?search_guard=js"))?
		.header("Cookie", format!("night=0; {css}").as_str())
		.string()?;
	let js = cookie(&script, "jieqiSearchJs").ok_or_else(|| error!("[linovelib] search: no jieqiSearchJs"))?;
	let cookies = format!("night=0; {css}; {js}");
	let ticket = set_cookie(
		&format!("{CN_URL}/search.html?search_guard=redeem&r={}000", current_date()),
		&cookies,
		"jieqiSearchTicket",
	)?;
	Ok(format!("{cookies}; {ticket}"))
}

pub fn search(query: &str, page: i32) -> Result<MangaPageResult> {
	let cookies = ticket()?;
	let url = format!("{CN_URL}/search/{}_{page}.html", encode_uri_component(query.trim()));
	let response = request(&url)?
		.header("Cookie", cookies.as_str())
		.send()?;
	let final_url = response.get_url().unwrap_or_default();
	let body = response.get_string()?;
	if body.trim().is_empty() {
		bail!("[linovelib] search: ticket refused");
	}
	let html = Html::parse_with_url(&body, &final_url)?;
	let base = base_url();
	// A search with one result answers with a redirect to that book.
	if let Some((key, None)) = parse_link(&final_url) {
		let entries = parse_details(&html, &key, base).into_iter().collect::<Vec<_>>();
		return Ok(MangaPageResult {
			entries,
			has_next_page: false,
		});
	}
	Ok(parse_manga_list(&html, base))
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn reads_cookies() {
		let header = "jieqiSearchCss=497526.Bc3N-Zd0; Path=/; Max-Age=3600; SameSite=Lax; Secure; HttpOnly";
		assert_eq!(cookie(header, "jieqiSearchCss").as_deref(), Some("jieqiSearchCss=497526.Bc3N-Zd0"));
		let script = include_str!("fixtures/search_guard_js.txt");
		let js = cookie(script, "jieqiSearchJs").expect("cookie");
		assert!(js.starts_with("jieqiSearchJs=497526."), "{js}");
		assert!(!js.contains(';') && !js.contains('"'));
		assert_eq!(cookie("x=1", "jieqiSearchTicket"), None);
	}
}
