//! Genre ids of the catalogue's `tagid` field with their names on each site, so a tag
//! tapped on a book (a name, in the language of the site it came from) finds its id.
//! Taken from the filter panels of both sites on 2026-10-04. tw labels id 60 `同人`,
//! but the books it lists are tagged `職場` and the Simplified site calls it `职场`;
//! tw ignores the `同人` id (333) the Simplified site has. Must match `res/filters.json`.

/// (id, Traditional name, Simplified name)
pub const TAGS: [(&str, &str, &str); 59] = [
	("64", "戀愛", "恋爱"),
	("48", "後宮", "后宫"),
	("63", "校園", "校园"),
	("27", "百合", "百合"),
	("26", "轉生", "转生"),
	("47", "異世界", "异世界"),
	("15", "奇幻", "奇幻"),
	("61", "冒險", "冒险"),
	("222", "歡樂向", "欢乐向"),
	("231", "女性視角", "女性视角"),
	("219", "龍傲天", "龙傲天"),
	("96", "魔法", "魔法"),
	("67", "青春", "青春"),
	("31", "性轉", "性转"),
	("198", "病嬌", "病娇"),
	("217", "妹妹", "妹妹"),
	("225", "青梅竹馬", "青梅竹马"),
	("18", "戰鬥", "战斗"),
	("256", "NTR", "NTR"),
	("223", "人外", "人外"),
	("227", "大小姐", "大小姐"),
	("189", "黑暗", "黑暗"),
	("68", "懸疑", "悬疑"),
	("56", "科幻", "科幻"),
	("201", "偽娘", "伪娘"),
	("55", "戰爭", "战争"),
	("185", "蘿莉", "萝莉"),
	("229", "復仇", "复仇"),
	("199", "鬥智", "斗智"),
	("131", "異能", "异能"),
	("241", "獵奇", "猎奇"),
	("191", "輕文學", "轻文学"),
	("60", "職場", "职场"),
	("226", "經營", "经营"),
	("246", "JK", "JK"),
	("135", "機戰", "机战"),
	("261", "女兒", "女儿"),
	("221", "末日", "末日"),
	("220", "犯罪", "犯罪"),
	("239", "旅行", "旅行"),
	("124", "驚悚", "惊悚"),
	("98", "治癒", "治愈"),
	("97", "推理", "推理"),
	("205", "日本文學", "日本文学"),
	("248", "遊戲", "游戏"),
	("228", "耽美", "耽美"),
	("211", "美食", "美食"),
	("245", "群像", "群像"),
	("249", "大逃殺", "大逃杀"),
	("233", "音樂", "音乐"),
	("132", "格鬥", "格斗"),
	("28", "熱血", "热血"),
	("180", "溫馨", "温馨"),
	("224", "腦洞", "脑洞"),
	("328", "惡役", "恶役"),
	("304", "JC", "JC"),
	("254", "間諜", "间谍"),
	("146", "競技", "竞技"),
	("263", "宅文化", "宅文化"),
];

/// The id of a genre given as an id or as its name on either site.
pub fn tag_id(value: &str) -> Option<&'static str> {
	let value = value.trim();
	TAGS.iter()
		.find(|(id, tw, cn)| *id == value || *tw == value || *cn == value)
		.map(|(id, _, _)| *id)
}

#[cfg(test)]
mod test {
	use super::*;
	use aidoku::alloc::format;
	use aidoku_test::aidoku_test;

	#[aidoku_test]
	fn matches_filters_json() {
		let json = include_str!("../res/filters.json");
		// Options and ids are each one quoted string per line.
		for (id, tw, _) in TAGS {
			assert!(json.contains(&format!("\"{tw}\"")), "{tw}");
			assert!(json.contains(&format!("\"{id}\"")), "{id}");
		}
	}

	#[aidoku_test]
	fn finds_ids_by_name() {
		assert_eq!(tag_id("戀愛"), Some("64"));
		assert_eq!(tag_id("恋爱"), Some("64"));
		assert_eq!(tag_id("64"), Some("64"));
		assert_eq!(tag_id("職場"), Some("60"));
		assert_eq!(tag_id("同人"), None);
	}
}
