use aidoku::{
	alloc::{String, Vec},
	imports::{
		net::Request,
		std::current_date,
	},
	prelude::*,
	Chapter, Manga, MangaStatus, Viewer,
};

use crate::auth;

const API_URL: &str = "https://komiic.com/api/query";

// ---------------------------------------------------------------------------
// GraphQL query constants
// ---------------------------------------------------------------------------

pub const RECENT_UPDATE_QUERY: &str = r#"query recentUpdate($pagination: Pagination!) { recentUpdate(pagination: $pagination) { id title status year imageUrl authors { id name __typename } categories { id name __typename } dateUpdated monthViews views favoriteCount lastBookUpdate lastChapterUpdate contentType __typename } }"#;

pub const HOT_COMICS_QUERY: &str = r#"query hotComics($pagination: Pagination!) { hotComics(pagination: $pagination) { id title status year imageUrl authors { id name __typename } categories { id name __typename } dateUpdated monthViews views favoriteCount lastBookUpdate lastChapterUpdate contentType __typename } }"#;

pub const SEARCH_QUERY: &str = r#"query searchComicAndAuthorQuery($keyword: String!) { searchComicsAndAuthors(keyword: $keyword) { comics { id title status year imageUrl authors { id name __typename } categories { id name __typename } dateUpdated monthViews views favoriteCount lastBookUpdate lastChapterUpdate contentType __typename } __typename } }"#;

pub const COMIC_BY_ID_QUERY: &str = r#"query comicById($comicId: ID!) { comicById(comicId: $comicId) { id title status year imageUrl authors { id name __typename } categories { id name __typename } dateCreated dateUpdated views favoriteCount lastBookUpdate lastChapterUpdate contentType __typename } }"#;

pub const CHAPTERS_QUERY: &str = r#"query chapterByComicId($comicId: ID!) { chaptersByComicId(comicId: $comicId) { id serial type dateCreated dateUpdated size __typename } }"#;

pub const IMAGES_QUERY: &str = r#"query imagesByChapterId($chapterId: ID!) { imagesByChapterId(chapterId: $chapterId) { id kid height width __typename } }"#;

pub const IMAGE_TICKETS_QUERY: &str = r#"query getImageTickets($kids: [String!]!) { getImageTickets(kids: $kids) { url ticket kid __typename } }"#;

pub const COMICS_BY_CATEGORY_QUERY: &str = r#"query comicByCategory($categoryId: ID!, $pagination: Pagination!) { comicByCategory(categoryId: $categoryId, pagination: $pagination) { id title status year imageUrl authors { id name __typename } categories { id name __typename } dateUpdated monthViews views favoriteCount lastBookUpdate lastChapterUpdate contentType __typename } }"#;

#[allow(dead_code)]
pub const ALL_CATEGORY_QUERY: &str = r#"query allCategory { allCategory { id name __typename } }"#;

/// The only query that reports authentication state. Komiic serves every other
/// query anonymously when the token is dead, so this one is used to probe it.
pub const ACCOUNT_QUERY: &str = r#"query account { account { id email nickname __typename } }"#;

pub const IMAGE_LIMIT_QUERY: &str = r#"query getImageLimit { getImageLimit { limit usage resetInSeconds __typename } }"#;

// ---------------------------------------------------------------------------
// GraphQL request helper
// ---------------------------------------------------------------------------

/// Send a single GraphQL POST to the Komiic API. No retry, no defaults access.
/// Returns the HTTP status code and the raw response body.
pub fn graphql_once(
	query: &str,
	variables: &str,
	token: Option<&str>,
) -> aidoku::Result<(i32, String)> {
	let ts = current_date();
	let api_url = format!("{API_URL}?_={ts}");

	let body = format!(
		r#"{{"operationName":"{}","query":"{}","variables":{}}}"#,
		extract_operation_name(query),
		json_escape(query),
		variables
	);

	let mut req = Request::post(&api_url)?
		.header("Content-Type", "application/json")
		.header("Referer", "https://komiic.com")
		.header("Cache-Control", "no-cache, no-store, must-revalidate")
		.header("Pragma", "no-cache");

	// Sent as a header rather than a cookie on purpose: Aidoku prepends the
	// shared cookie jar's entries to any Cookie header a source sets, so a stale
	// komiic-access-token from the jar would shadow ours and the request would be
	// served anonymously.
	if let Some(t) = token {
		if !t.is_empty() {
			let authorization = format!("Bearer {t}");
			req = req.header("Authorization", authorization.as_str());
		}
	}

	let response = req.body(body.as_bytes()).send()?;
	let status = response.status_code();
	let text = response.get_string()?;
	Ok((status, text))
}

/// Send a GraphQL request with the stored auth token, renewing an expired
/// session first if needed. Returns the full response body as a string.
pub fn graphql_request(query: &str, variables: &str) -> aidoku::Result<String> {
	auth::ensure_session();

	let (status, body) = graphql_once(query, variables, auth::token().as_deref())?;

	// Komiic answers most queries anonymously instead of rejecting a dead token,
	// so this path only fires for the queries that do report the error.
	if is_auth_error(status, &body) && auth::token().is_some() {
		if let Some(token) = auth::recover_session() {
			let (retry_status, retry_body) = graphql_once(query, variables, Some(&token))?;
			if retry_status != 200 {
				bail!("Komiic HTTP {retry_status}");
			}
			return Ok(retry_body);
		}
	}

	if status != 200 {
		bail!("Komiic HTTP {status}");
	}
	Ok(body)
}

/// First `errors[].message` in a GraphQL response body, if there is one.
pub fn first_error_message(body: &str) -> Option<&str> {
	let errors = json_data_field(body, "errors")?;
	if json_top_level_objects(errors).is_empty() {
		return None;
	}
	json_str_value(errors, "message")
}

/// Whether a response says the token was not accepted. Komiic never replies
/// with 401: an unauthenticated `account` query returns HTTP 200 and
/// `{"errors":[{"message":"no token","path":["account"]}],"data":null}`,
/// and it answers exactly the same for a missing, expired or garbage token.
/// A 403 is not counted: that is a Cloudflare block, not an auth error.
pub fn is_auth_error(status: i32, body: &str) -> bool {
	status == 401 || first_error_message(body) == Some("no token")
}

/// Extract the operation name from a GraphQL query string.
/// E.g. "query recentUpdate($pagination: ...)" → "recentUpdate"
fn extract_operation_name(query: &str) -> &str {
	// Find "query " or "mutation "
	let start = if let Some(pos) = query.find("query ") {
		pos + 6
	} else if let Some(pos) = query.find("mutation ") {
		pos + 9
	} else {
		return "";
	};
	let rest = &query[start..];
	// Stop at the argument list, the selection set, or plain whitespace: a
	// query without variables has no '(' at all.
	let end = rest
		.find(|c: char| c == '(' || c == '{' || c.is_whitespace())
		.unwrap_or(rest.len());
	rest[..end].trim()
}

/// Escape a string for embedding as a JSON string value.
pub fn json_escape(value: &str) -> String {
	let mut out = String::new();
	for ch in value.chars() {
		match ch {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			c if (c as u32) < 0x20 => {
				out.push_str("\\u");
				let code = c as u32;
				for shift in [12u32, 8, 4, 0] {
					let digit = (code >> shift) & 0xF;
					out.push(char::from_digit(digit, 16).unwrap_or('0'));
				}
			}
			c => out.push(c),
		}
	}
	out
}

// ---------------------------------------------------------------------------
// Minimal JSON parsing helpers (no_std compatible)
// ---------------------------------------------------------------------------

/// Extract a JSON string value: `"key":"value"` → `value`
pub fn json_str_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
	let search = format!("\"{}\":\"", key);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];
	// Handle escaped quotes
	let mut end = 0;
	let bytes = rest.as_bytes();
	while end < bytes.len() {
		if bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\') {
			break;
		}
		end += 1;
	}
	Some(&rest[..end])
}

/// Extract a JSON number value: `"key":123` → `123`
pub fn json_num_value(json: &str, key: &str) -> Option<i64> {
	let search = format!("\"{}\":", key);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];

	let mut num_str = String::new();
	for ch in rest.chars() {
		if ch.is_ascii_digit() || ch == '-' {
			num_str.push(ch);
		} else if !num_str.is_empty() {
			break;
		}
	}

	num_str.parse::<i64>().ok()
}

/// Iterate over JSON array objects.
/// Given `"key":[{...},{...}]`, returns a Vec of the individual `{...}` strings.
pub fn json_array_objects<'a>(json: &'a str, key: &str) -> Vec<&'a str> {
	let mut results: Vec<&str> = Vec::new();
	let search = format!("\"{}\":[", key);
	let pos = match json.find(&search) {
		Some(p) => p + search.len(),
		None => return results,
	};

	let body = &json[pos..];
	let mut depth = 0i32;
	let mut obj_start: Option<usize> = None;

	for (i, ch) in body.char_indices() {
		match ch {
			'{' => {
				if depth == 0 {
					obj_start = Some(i);
				}
				depth += 1;
			}
			'}' => {
				depth -= 1;
				if depth == 0 {
					if let Some(start) = obj_start {
						results.push(&body[start..=i]);
					}
					obj_start = None;
				}
			}
			']' if depth == 0 => break,
			_ => {}
		}
	}

	results
}

/// Find a top-level JSON object for the given data key.
/// E.g. find `"data":{ "recentUpdate": [...] }` and return the inner content.
pub fn json_data_field<'a>(json: &'a str, field: &str) -> Option<&'a str> {
	// Find "field":
	let search = format!("\"{}\":", field);
	let pos = json.find(&search)?;
	let start = pos + search.len();
	let rest = &json[start..];

	// Find the start of the value
	let first_char = rest.chars().next()?;
	if first_char == '[' {
		// It's an array - find the matching ]
		let mut depth = 0i32;
		for (i, ch) in rest.char_indices() {
			match ch {
				'[' => depth += 1,
				']' => {
					depth -= 1;
					if depth == 0 {
						return Some(&rest[..=i]);
					}
				}
				_ => {}
			}
		}
	} else if first_char == '{' {
		// It's an object - find the matching }
		let mut depth = 0i32;
		for (i, ch) in rest.char_indices() {
			match ch {
				'{' => depth += 1,
				'}' => {
					depth -= 1;
					if depth == 0 {
						return Some(&rest[..=i]);
					}
				}
				_ => {}
			}
		}
	}
	None
}

/// Extract top-level objects from a JSON array string like `[{...},{...}]`.
pub fn json_top_level_objects(array_str: &str) -> Vec<&str> {
	let mut results: Vec<&str> = Vec::new();
	let mut depth = 0i32;
	let mut obj_start: Option<usize> = None;

	for (i, ch) in array_str.char_indices() {
		match ch {
			'{' => {
				if depth == 0 {
					obj_start = Some(i);
				}
				depth += 1;
			}
			'}' => {
				depth -= 1;
				if depth == 0 {
					if let Some(start) = obj_start {
						results.push(&array_str[start..=i]);
					}
					obj_start = None;
				}
			}
			_ => {}
		}
	}

	results
}

// ---------------------------------------------------------------------------
// Data parsing
// ---------------------------------------------------------------------------

/// Parse a comic JSON object into a Manga struct.
pub fn parse_comic(obj: &str) -> Option<Manga> {
	let id = json_str_value(obj, "id")?;
	let title = json_str_value(obj, "title")?;

	let image_url = json_str_value(obj, "imageUrl");
	let cover = image_url.map(|u| String::from(u));

	let status_str = json_str_value(obj, "status").unwrap_or("UNKNOWN");
	let status = match status_str {
		"ONGOING" => MangaStatus::Ongoing,
		"END" | "COMPLETED" => MangaStatus::Completed,
		_ => MangaStatus::Unknown,
	};

	// Extract authors
	let authors_objs = json_array_objects(obj, "authors");
	let authors: Vec<String> = authors_objs
		.iter()
		.filter_map(|a| json_str_value(a, "name").map(String::from))
		.collect();

	// Extract categories as tags
	let category_objs = json_array_objects(obj, "categories");
	let tags: Vec<String> = category_objs
		.iter()
		.filter_map(|c| json_str_value(c, "name").map(String::from))
		.collect();

	let url = format!("https://komiic.com/comic/{id}");

	Some(Manga {
		key: String::from(id),
		title: String::from(title),
		cover,
		authors: if authors.is_empty() {
			None
		} else {
			Some(authors)
		},
		tags: if tags.is_empty() { None } else { Some(tags) },
		status,
		url: Some(url),
		viewer: Viewer::Webtoon,
		..Default::default()
	})
}

/// Parse a chapter JSON object into a Chapter struct.
/// Chapter type can be "book" (volume) or "chapter".
pub fn parse_chapter(obj: &str) -> Option<Chapter> {
	let id = json_str_value(obj, "id")?;
	let serial = json_num_value(obj, "serial")? as f32;
	let chapter_type = json_str_value(obj, "type").unwrap_or("chapter");

	let title = match chapter_type {
		"book" => Some(format!("卷 {}", serial as i32)),
		_ => Some(format!("第 {} 話", serial as i32)),
	};

	let url = Some(format!("https://komiic.com/chapter/{id}"));

	Some(Chapter {
		key: String::from(id),
		title,
		chapter_number: Some(serial),
		url,
		..Default::default()
	})
}

/// Build a JSON array of kid strings for the getImageTickets query.
#[allow(dead_code)]
pub fn build_kids_json_array(kids: &[&str]) -> String {
	let mut result = String::from("[");
	for (i, kid) in kids.iter().enumerate() {
		if i > 0 {
			result.push(',');
		}
		result.push('"');
		result.push_str(kid);
		result.push('"');
	}
	result.push(']');
	result
}

// ---------------------------------------------------------------------------
// Token helpers
// ---------------------------------------------------------------------------

/// Extract `komiic-access-token` from a `set-cookie` header value.
pub fn extract_token_from_cookie(cookie_header: &str) -> Option<&str> {
	let search = "komiic-access-token=";
	let pos = cookie_header.find(search)?;
	let start = pos + search.len();
	let rest = &cookie_header[start..];
	let end = rest.find(';').unwrap_or(rest.len());
	Some(&rest[..end])
}
