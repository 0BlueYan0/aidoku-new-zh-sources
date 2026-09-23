//! Reading pipeline (phase 2).
//!
//! Opening a book needs a `cr` token that only the site's loader script produces in a
//! real browser, so a background web view opens the site's own viewer and a user
//! script captures the viewer's `/browserWebApi/c` request and response (no second
//! call, no extra reader session). From `c` come the signed CloudFront `url` and
//! `auth_info`; the source then fetches and decrypts `configuration_pack.json` on its
//! own, lists the pages in `configuration.contents` order, and builds each page's
//! image URL. Images are AES-free but tile-scrambled, so each page carries the data
//! `process_page_image` needs to unscramble it with a canvas.
//!
//! Auth expires ~60s after `c` (the CloudFront policy covers the whole book folder).
//! Refreshing uses `pb` (putBookmark), which returns a fresh `auth_info` without
//! opening a new reader slot; a second `c` would count as another concurrent open and
//! trip the site's 99x limit. `pb` needs the pcreader `SESSION` cookie (captured from
//! the web view) and a `Referer` of the viewer page. Its `auth_info` has a null
//! `uuid`, so the `uuid` from `c` is kept across refreshes.

use aidoku::{
	alloc::{string::ToString, String, Vec},
	helpers::uri::encode_uri_component,
	imports::{
		canvas::{Canvas, ImageRef, Rect},
		defaults::{defaults_get, defaults_set, DefaultValue},
		js::{WebView, WebViewUserScript},
		net::Request,
		std::current_date,
	},
	prelude::*,
	Page, PageContent, PageContext, Result,
};

use crate::crypto::decrypt_config;
use crate::descramble::{page_file_and_tiles_from_info, page_file_name, page_order, parse_page_info};
use crate::helper::{BASE_URL, USER_AGENT};

const READER_BASE: &str = "https://pcreader.bookwalker.com.tw";
const READSTORAGE_HOST: &str = "readstorage.bookwalker.com.tw";
/// The auth_info fields, in the order the viewer sends them as query parameters.
const AUTH_FIELDS: [&str; 8] = [
	"hti", "cfg", "bid", "uuid", "pfCd", "Policy", "Signature", "Key-Pair-Id",
];

/// Captures the viewer's own `c` request URL and response body onto separate window
/// properties, kept raw (not JSON-wrapped) so the response parses without a layer of
/// `\"` escaping.
const CAPTURE_SCRIPT: &str = "(function(){var O=XMLHttpRequest.prototype.open,S=XMLHttpRequest.prototype.send;XMLHttpRequest.prototype.open=function(m,u){this.__bwU=u;return O.apply(this,arguments);};XMLHttpRequest.prototype.send=function(){var x=this;x.addEventListener('load',function(){try{if(x.__bwU&&x.__bwU.indexOf('/browserWebApi/c?')>=0){window.__bwCReq=''+x.__bwU;window.__bwCRes=''+x.responseText;}}catch(e){}});return S.apply(this,arguments);};})();";
const READ_RES: &str = "window.__bwCRes||''";
const READ_REQ: &str = "window.__bwCReq||''";

/// How long to wait for the viewer to fire its `c` call.
const OPEN_TIMEOUT: i64 = 25;
/// Refresh the auth this many seconds after it was obtained. The CloudFront policy
/// lasts ~60s, so refresh a little before that.
const REFRESH_AFTER: i64 = 50;

// ---------------------------------------------------------------------------
// Page list
// ---------------------------------------------------------------------------

pub fn get_pages(pid: &str) -> Result<Vec<Page>> {
	let cap = open_and_capture(pid)?;
	let c = parse_c(&cap.req, &cap.res).ok_or_else(|| error!("could not read the reader authorisation"))?;
	if c.status != "200" || c.auth_query.is_empty() {
		// 991/998 = the book is open elsewhere or too many concurrent opens; the site
		// clears it after a while.
		bail!("reader returned status {}", c.status);
	}
	store_session(pid, &c, &cap.session);

	let config_url = format!("{}configuration_pack.json?{}", c.url, c.auth_query);
	let body = Request::get(&config_url)?
		.header("User-Agent", USER_AGENT)
		.string()?;
	let data = json_field(&body, "data").ok_or_else(|| error!("configuration_pack had no data"))?;
	let (json, a, z, t) = decrypt_config(&data).ok_or_else(|| error!("could not decrypt the configuration"))?;

	let order = page_order(&json);
	// Diagnostic (temporary): confirms the open + decode chain on device.
	println!("[bookwalker] reader open ok, pages={}", order.len());
	let mut pages: Vec<Page> = Vec::new();
	for xhtml in order {
		let Some(info) = parse_page_info(&json, &xhtml) else {
			continue;
		};
		let file = page_file_name(&info, &xhtml, &a, &z, &t);
		let image_url = format!("{}{}/{}.jpeg", c.url, xhtml, file);
		let mut ctx = PageContext::new();
		ctx.insert("pid".into(), pid.into());
		ctx.insert("xhtml".into(), xhtml.clone());
		ctx.insert("a".into(), hex(&a));
		ctx.insert("z".into(), hex(&z));
		ctx.insert("t".into(), hex(&t));
		ctx.insert("no".into(), info.no.to_string());
		ctx.insert("sw".into(), info.size_width.to_string());
		ctx.insert("sh".into(), info.size_height.to_string());
		ctx.insert("bw".into(), info.block_width.to_string());
		ctx.insert("bh".into(), info.block_height.to_string());
		ctx.insert("dw".into(), info.dummy_width.to_string());
		ctx.insert("dh".into(), info.dummy_height.to_string());
		ctx.insert("ns".into(), info.ns.to_string());
		ctx.insert("ps".into(), info.ps.to_string());
		ctx.insert("rs".into(), info.rs.to_string());
		pages.push(Page {
			content: PageContent::url_context(image_url, ctx),
			..Default::default()
		});
	}
	if pages.is_empty() {
		bail!("the configuration listed no pages");
	}
	Ok(pages)
}

/// Attaches the current auth to a page image request, refreshing it via `pb` when it
/// is about to expire. The readstorage URL is a signed CloudFront link, so it needs
/// no cookies, only the query.
pub fn image_request(url: String, ctx: Option<&PageContext>) -> Result<Request> {
	// Covers and thumbnails (a different host) pass straight through.
	if !url.contains(READSTORAGE_HOST) {
		return Ok(Request::get(&url)?.header("User-Agent", USER_AGENT));
	}
	let Some(pid) = ctx.and_then(|c| c.get("pid")) else {
		return Ok(Request::get(&url)?.header("User-Agent", USER_AGENT));
	};
	let position = ctx.and_then(|c| c.get("xhtml")).map(|s| s.as_str());
	refresh_if_stale(pid, position);

	let auth = defaults_get::<String>(&key(pid, "auth")).unwrap_or_default();
	let full = if auth.is_empty() {
		url
	} else {
		format!("{url}?{auth}")
	};
	Ok(Request::get(&full)?.header("User-Agent", USER_AGENT))
}

// ---------------------------------------------------------------------------
// Descramble (called from PageImageProcessor)
// ---------------------------------------------------------------------------

/// Unscrambles one page from its 32px tiles. Returns `None` if the bytes are not a
/// JPEG (an expired-auth error body, say) or the context is incomplete.
pub fn descramble(scrambled_bytes: &[u8], ctx: &PageContext) -> Option<ImageRef> {
	// An expired-auth response is a small XML/HTML error (~1.7 KB); a real page is
	// far larger. Size, not magic bytes, decides — the app decodes the JPEG and hands
	// the processor PNG bytes, so the response does not start with the JPEG marker.
	if scrambled_bytes.len() < 4096 {
		return None;
	}
	let info = parse_page_info_from_ctx(ctx)?;
	let xhtml = ctx.get("xhtml")?;
	let a = unhex(ctx.get("a")?)?;
	let z = unhex(ctx.get("z")?)?;
	let t = unhex(ctx.get("t")?)?;

	let (_file, tiles) = page_file_and_tiles_from_info(&info, xhtml, &a, &z, &t);
	let scrambled = ImageRef::new(scrambled_bytes);
	let mut canvas = Canvas::new(info.size_width as f32, info.size_height as f32);
	for tile in tiles {
		// The site draws the region at (dest) of the scrambled image onto (src) of the
		// output, so read src_rect = dest, write dst_rect = src.
		canvas.copy_image(
			&scrambled,
			Rect::new(tile.dest_x as f32, tile.dest_y as f32, tile.width as f32, tile.height as f32),
			Rect::new(tile.src_x as f32, tile.src_y as f32, tile.width as f32, tile.height as f32),
		);
	}
	Some(canvas.get_image())
}

// ---------------------------------------------------------------------------
// Web view open + capture
// ---------------------------------------------------------------------------

struct Capture {
	req: String,
	res: String,
	session: String,
}

fn open_and_capture(pid: &str) -> Result<Capture> {
	let webview = WebView::new();
	let mut script = WebViewUserScript::new(CAPTURE_SCRIPT.into());
	script.at_document_end = false; // run before the viewer's own scripts
	script.for_main_frame_only = true;
	webview
		.add_user_script(script)
		.map_err(|_| error!("could not install the capture script"))?;

	let url = format!("{BASE_URL}/browserViewer/{pid}/read");
	let request = Request::get(&url)?.header("User-Agent", USER_AGENT);
	webview
		.load_blocking(request)
		.map_err(|_| error!("could not open the reader"))?;

	// `load_blocking` already waited; poll for the viewer's captured `c` (see CLAUDE.md
	// on not calling `wait_for_load` after `load_blocking`).
	let started = current_date();
	loop {
		if let Ok(res) = webview.eval(READ_RES) {
			if !res.trim().is_empty() {
				let req = webview.eval(READ_REQ).unwrap_or_default();
				let session = pcreader_session(&webview);
				return Ok(Capture { req, res, session });
			}
		}
		if current_date() - started >= OPEN_TIMEOUT {
			bail!("the reader did not authorise in time");
		}
	}
}

/// The pcreader `SESSION` cookie value, needed to call `pb` from the source's own
/// requests (its jar is separate from the web view's).
fn pcreader_session(webview: &WebView) -> String {
	webview
		.get_cookies()
		.ok()
		.into_iter()
		.flatten()
		.find(|cookie| cookie.name == "SESSION" && cookie.domain.contains("pcreader"))
		.map(|cookie| cookie.value)
		.unwrap_or_default()
}

struct CInfo {
	status: String,
	url: String,
	auth_query: String,
	uuid: String,
	cid: String,
	u1: String,
	bid: String,
}

fn parse_c(req: &str, res: &str) -> Option<CInfo> {
	if res.trim().is_empty() {
		return None;
	}
	Some(CInfo {
		status: json_field(res, "status").unwrap_or_default(),
		url: json_field(res, "url").unwrap_or_default(),
		auth_query: build_auth_query(res, None),
		uuid: auth_field(res, "uuid").unwrap_or_default(),
		cid: query_param(req, "cid").unwrap_or_default(),
		u1: query_param(req, "u1").unwrap_or_default(),
		bid: query_param(req, "BID").unwrap_or_default(),
	})
}

/// Builds the `hti=..&cfg=..&...` query from an `auth_info` object. `pb` returns a
/// null `uuid`, so `uuid_override` supplies the one captured from `c`.
fn build_auth_query(json: &str, uuid_override: Option<&str>) -> String {
	let Some(start) = json.find("\"auth_info\"") else {
		return String::new();
	};
	let object = &json[start..];
	let mut parts: Vec<String> = Vec::new();
	for field in AUTH_FIELDS {
		let value = if field == "uuid" {
			match uuid_override {
				Some(uuid) if !uuid.is_empty() => Some(uuid.to_string()),
				_ => json_field(object, field),
			}
		} else {
			json_field(object, field)
		};
		if let Some(value) = value.filter(|v| v != "null" && !v.is_empty()) {
			parts.push(format!("{field}={value}"));
		}
	}
	parts.join("&")
}

// ---------------------------------------------------------------------------
// Auth refresh (pb)
// ---------------------------------------------------------------------------

/// Refreshes the auth via `pb` if it is older than `REFRESH_AFTER`. The timestamp is
/// advanced before the request so parallel image requests do not all fire `pb`.
fn refresh_if_stale(pid: &str, position: Option<&str>) {
	let at = defaults_get::<String>(&key(pid, "at"))
		.and_then(|v| v.parse::<i64>().ok())
		.unwrap_or(0);
	if current_date() - at < REFRESH_AFTER {
		return;
	}
	set_at(pid, current_date());

	let (Some(cid), Some(u1), Some(bid), Some(uuid), Some(session)) = (
		defaults_get::<String>(&key(pid, "cid")),
		defaults_get::<String>(&key(pid, "u1")),
		defaults_get::<String>(&key(pid, "bid")),
		defaults_get::<String>(&key(pid, "uuid")),
		defaults_get::<String>(&key(pid, "ses")),
	) else {
		return;
	};

	let position = position.unwrap_or("item/xhtml/p-001.xhtml");
	let bookmark = format!(
		"{{\"position\":\"{position}\",\"type\":\"epub\",\"bookmarks\":[]}}"
	);
	let body = format!(
		"cid={cid}&u1={u1}&bookmark={}&timestamp=&BID={bid}",
		encode_uri_component(&bookmark)
	);
	let referer = format!("{READER_BASE}/51/30/viewer.html?cid={cid}&cty=1");
	let cookie = format!("SESSION={session}");
	let pb_url = format!("{READER_BASE}/browserWebApi/pb");
	let response = Request::post(&pb_url).map_err(|_| ()).and_then(|request| {
		request
			.header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
			.header("Referer", referer.as_str())
			.header("Cookie", cookie.as_str())
			.header("User-Agent", USER_AGENT)
			.body(body.as_bytes())
			.string()
			.map_err(|_| ())
	});
	match response {
		Ok(json) => {
			let auth = build_auth_query(&json, Some(&uuid));
			// Diagnostic (temporary): confirms auth refresh works on device.
			println!("[bookwalker] reader auth refreshed, authlen={}", auth.len());
			if !auth.is_empty() {
				defaults_set(&key(pid, "auth"), DefaultValue::String(auth));
			}
		}
		Err(_) => println!("[bookwalker] reader auth refresh failed"),
	}
}

// ---------------------------------------------------------------------------
// Session storage (per book)
// ---------------------------------------------------------------------------

/// Comma-separated list of every pid that has session keys, so logout can find them.
const PIDS_KEY: &str = "rd_pids";
/// Every per-book key name written by `store_session` / `set_at`.
const SESSION_FIELDS: [&str; 7] = ["auth", "cid", "u1", "bid", "uuid", "ses", "at"];

fn key(pid: &str, name: &str) -> String {
	format!("rd_{name}_{pid}")
}

fn set_at(pid: &str, at: i64) {
	defaults_set(&key(pid, "at"), DefaultValue::String(at.to_string()));
}

fn store_session(pid: &str, c: &CInfo, session: &str) {
	defaults_set(&key(pid, "auth"), DefaultValue::String(c.auth_query.clone()));
	defaults_set(&key(pid, "cid"), DefaultValue::String(c.cid.clone()));
	defaults_set(&key(pid, "u1"), DefaultValue::String(c.u1.clone()));
	defaults_set(&key(pid, "bid"), DefaultValue::String(c.bid.clone()));
	defaults_set(&key(pid, "uuid"), DefaultValue::String(c.uuid.clone()));
	defaults_set(&key(pid, "ses"), DefaultValue::String(session.to_string()));
	set_at(pid, current_date());
	remember_pid(pid);
}

fn remember_pid(pid: &str) {
	let mut pids = defaults_get::<String>(PIDS_KEY).unwrap_or_default();
	if pids.split(',').any(|known| known == pid) {
		return;
	}
	if !pids.is_empty() {
		pids.push(',');
	}
	pids.push_str(pid);
	defaults_set(PIDS_KEY, DefaultValue::String(pids));
}

/// Drops every stored reader session. Called on logout, so nothing that belonged to
/// the previous account (the pcreader `SESSION` cookie above all) survives it.
pub fn clear_sessions() {
	let pids = defaults_get::<String>(PIDS_KEY).unwrap_or_default();
	let mut count = 0;
	for pid in pids.split(',').filter(|pid| !pid.is_empty()) {
		for field in SESSION_FIELDS {
			defaults_set(&key(pid, field), DefaultValue::Null);
		}
		count += 1;
	}
	defaults_set(PIDS_KEY, DefaultValue::Null);
	println!("[bookwalker] cleared reader sessions for {count} books");
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn parse_page_info_from_ctx(ctx: &PageContext) -> Option<crate::descramble::PageInfo> {
	Some(crate::descramble::PageInfo {
		no: ctx.get("no")?.parse().ok()?,
		size_width: ctx.get("sw")?.parse().ok()?,
		size_height: ctx.get("sh")?.parse().ok()?,
		block_width: ctx.get("bw")?.parse().ok()?,
		block_height: ctx.get("bh")?.parse().ok()?,
		dummy_width: ctx.get("dw")?.parse().ok()?,
		dummy_height: ctx.get("dh")?.parse().ok()?,
		ns: ctx.get("ns")?.parse().ok()?,
		ps: ctx.get("ps")?.parse().ok()?,
		rs: ctx.get("rs")?.parse().ok()?,
	})
}

fn hex(bytes: &[u8; 32]) -> String {
	let mut out = String::with_capacity(64);
	for b in bytes {
		out.push(nibble(b >> 4));
		out.push(nibble(b & 0x0f));
	}
	out
}

fn nibble(n: u8) -> char {
	if n < 10 {
		(b'0' + n) as char
	} else {
		(b'a' + n - 10) as char
	}
}

fn unhex(s: &str) -> Option<[u8; 32]> {
	let bytes = s.as_bytes();
	if bytes.len() != 64 {
		return None;
	}
	let mut out = [0u8; 32];
	for (i, byte) in out.iter_mut().enumerate() {
		let hi = hex_val(bytes[i * 2])?;
		let lo = hex_val(bytes[i * 2 + 1])?;
		*byte = (hi << 4) | lo;
	}
	Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
	match c {
		b'0'..=b'9' => Some(c - b'0'),
		b'a'..=b'f' => Some(c - b'a' + 10),
		b'A'..=b'F' => Some(c - b'A' + 10),
		_ => None,
	}
}

/// Read a flat JSON string/number field without a JSON library. Values in the `c` and
/// `pb` responses are plain (no escaped quotes), so a scan is enough.
fn json_field(body: &str, key: &str) -> Option<String> {
	let pattern = format!("\"{key}\"");
	let after = body[body.find(&pattern)? + pattern.len()..].trim_start();
	let after = after.strip_prefix(':')?.trim_start();
	if let Some(rest) = after.strip_prefix('"') {
		let end = rest.find('"')?;
		Some(rest[..end].to_string())
	} else {
		let end = after.find([',', '}', ']']).unwrap_or(after.len());
		Some(after[..end].trim().to_string())
	}
}

/// Reads a field from inside the `auth_info` object specifically.
fn auth_field(json: &str, field: &str) -> Option<String> {
	let start = json.find("\"auth_info\"")?;
	json_field(&json[start..], field)
}

fn query_param(url: &str, name: &str) -> Option<String> {
	let query = url.split_once('?')?.1;
	query.split('&').find_map(|pair| {
		let (key, value) = pair.split_once('=')?;
		(key == name && !value.is_empty()).then(|| value.to_string())
	})
}
