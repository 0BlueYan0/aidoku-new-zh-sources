//! Genre names and ids for `/v1/mangas?genre=`.
//!
//! The site has no genre list endpoint (`/v1/genres` answers an empty array), so this
//! table was built on 2026-09-24 by querying `genre=1..400` and intersecting the genre
//! names of every title returned. Only genres with at least 10 titles are kept. The
//! names are the site's own; simplified and traditional spellings (校园 / 校園) are
//! separate genres with separate titles. Must match the `genre` filter in
//! `res/filters.json`.

const GENRES: [(&str, u32); 33] = [
	("恋爱", 6),
	("奇幻", 8),
	("少年", 75),
	("都市", 23),
	("奇幻冒險", 127),
	("校园", 132),
	("愛情", 253),
	("歐式宮廷", 284),
	("古风", 19),
	("悬疑", 2),
	("搞笑", 4),
	("治愈", 7),
	("動作", 150),
	("剧情", 3),
	("逆袭", 32),
	("穿越", 20),
	("热血", 33),
	("战斗", 63),
	("重生", 46),
	("校園", 257),
	("系统", 67),
	("冒险", 38),
	("大女主", 30),
	("异能", 51),
	("日常", 5),
	("青春", 11),
	("影視化", 258),
	("玄幻", 27),
	("現代/職場", 260),
	("大人系", 288),
	("复仇", 31),
	("劇情", 115),
	("KK独家", 195),
];

/// The id of a genre by its displayed name, as a tapped tag arrives.
pub fn id_for_name(name: &str) -> Option<u32> {
	GENRES
		.iter()
		.find(|(genre, _): &&(&str, u32)| *genre == name)
		.map(|(_, id): &(&str, u32)| *id)
}
