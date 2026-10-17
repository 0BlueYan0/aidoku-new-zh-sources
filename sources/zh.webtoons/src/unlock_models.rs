//! Server offers and the immutable quote reviewed before a user-confirmed debit.
use aidoku::{alloc::{String, Vec}, prelude::*, Result};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductMenu {
	pub product: Product,
	pub title_info: ProductTitle,
	#[serde(default)]
	pub premium_benefit_user: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductTitle { pub title_name: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Product {
	pub title_no: i32,
	pub episode_no: i32,
	pub episode_title: Option<String>,
	pub product_id: String,
	pub sale_unit_list: Vec<Offer>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
	pub product_id: String,
	pub product_sale_unit_id: String,
	pub policy_price_currency: String,
	pub policy_price: i32,
	pub sale_unit_type: String,
	pub end_sale_date: i64,
	pub rental_days: Option<i32>,
}
impl Offer {
	fn coin_eligible(&self, product: &str, premium: bool, now_ms: i64) -> bool {
		let kind = matches!(self.sale_unit_type.as_str(), "PREVIEW" | "COMPLETE")
			|| (premium && matches!(self.sale_unit_type.as_str(),
				"PREVIEW_PREMIUM_DISCOUNT" | "PREVIEW_PREMIUM_DISCOUNT_V2"
				| "COMPLETE_PREMIUM_DISCOUNT" | "COMPLETE_PREMIUM_DISCOUNT_V2"));
		kind && self.product_id == product && !self.product_sale_unit_id.is_empty()
			&& self.policy_price > 0 && !self.policy_price_currency.is_empty()
			&& self.end_sale_date > now_ms
	}

	pub fn label(&self) -> String {
		let kind = if self.sale_unit_type.starts_with("PREVIEW") { "搶先看" } else { "買斷" };
		match self.rental_days {
			Some(days) if days > 0 => format!("租借 {days} 天・{} 代幣", self.policy_price),
			_ => format!("{kind}・{} 代幣", self.policy_price),
		}
	}
}
impl ProductMenu {
	pub fn coin_offers(&self, title: i32, episode: i32, now_ms: i64) -> Result<Vec<Offer>> {
		let expected = format!("linewebtoon-WEBTOON-{title}-{episode}");
		if title <= 0 || episode <= 0 || self.product.title_no != title
			|| self.product.episode_no != episode || self.product.product_id != expected {
			return Err(error!("解鎖方案與所選章節不符"));
		}
		Ok(self.product.sale_unit_list.iter()
			.filter(|offer| offer.coin_eligible(&expected, self.premium_benefit_user, now_ms))
			.cloned().collect())
	}
}

#[derive(Deserialize)]
pub struct CoinBalanceResult { pub balance: CoinBalance }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinBalance {
	pub coin_type: String,
	pub amount: i64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Quote {
	pub account_id: String,
	pub title_no: i32,
	pub episode_no: i32,
	pub manga_title: String,
	pub chapter_title: String,
	pub offer: Offer,
	pub prepared_at: i64,
	pub nonce: String,
}
impl Quote {
	pub fn validate(&self, account: &str, now: i64, menu: &ProductMenu, balance: &CoinBalance) -> Result<()> {
		if self.account_id.is_empty() || self.account_id != account {
			return Err(error!("帳號已變更，請重新選擇解鎖方案"));
		}
		if !(0..=180).contains(&(now - self.prepared_at)) {
			return Err(error!("報價已過期，請重新載入解鎖方案"));
		}
		let offers = menu.coin_offers(self.title_no, self.episode_no, now * 1000)?;
		if !offers.contains(&self.offer) {
			return Err(error!("方案、價格或有效期限已變更，未扣除代幣"));
		}
		if balance.coin_type != self.offer.policy_price_currency {
			return Err(error!("代幣種類與解鎖方案不符，未扣除代幣"));
		}
		if balance.amount < i64::from(self.offer.policy_price) {
			return Err(error!("代幣不足，未送出解鎖請求"));
		}
		Ok(())
	}
}

#[derive(Deserialize)]
pub struct BuyResult { pub payment: Option<Payment> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payment { pub payment_status: Option<String> }
impl BuyResult {
	pub fn complete(&self) -> bool {
		self.payment.as_ref().and_then(|payment| payment.payment_status.as_deref()) == Some("COMPLETE")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidoku_test::aidoku_test;

	fn menu() -> ProductMenu {
		crate::models::decode(r#"{"message":{"result":{"product":{"titleNo":10568,"episodeNo":2,"productId":"linewebtoon-WEBTOON-10568-2","episodeTitle":"第1集","saleUnitList":[{"productId":"linewebtoon-WEBTOON-10568-2","productSaleUnitId":"sale-2","policyPriceCurrency":"TW_WCOIN","policyPrice":27,"saleUnitType":"COMPLETE","endSaleDate":32503647599000},{"productId":"linewebtoon-WEBTOON-10568-2","productSaleUnitId":"ad-2","policyPriceCurrency":"TW_WCOIN","policyPrice":0,"saleUnitType":"REWARD_AD","endSaleDate":32503647599000}]},"titleInfo":{"titleName":"咒術迴戰"},"premiumBenefitUser":false}}}"#).unwrap()
	}

	fn quote() -> Quote {
		Quote { account_id: "account-a".into(), title_no: 10568, episode_no: 2,
			manga_title: "咒術迴戰".into(), chapter_title: "第1集".into(),
			offer: menu().product.sale_unit_list[0].clone(), prepared_at: 100, nonce: "q1".into() }
	}

	#[aidoku_test]
	fn coin_offers_exclude_reward_ads_and_zero_price_claims() {
		let offers = menu().coin_offers(10568, 2, 100_000).unwrap();
		assert_eq!(offers.len(), 1);
		assert_eq!(offers[0].policy_price, 27);
		assert_eq!(offers[0].label(), "買斷・27 代幣");
	}

	#[aidoku_test]
	fn confirmation_rechecks_account_quote_age_exact_offer_currency_and_balance() {
		let current = menu();
		let balance = CoinBalance { coin_type: "TW_WCOIN".into(), amount: 100 };
		assert!(quote().validate("account-a", 101, &current, &balance).is_ok());
		assert!(quote().validate("account-b", 101, &current, &balance).is_err());
		assert!(quote().validate("account-a", 401, &current, &balance).is_err());
		let mut changed = menu(); changed.product.sale_unit_list[0].policy_price = 28;
		assert!(quote().validate("account-a", 101, &changed, &balance).is_err());
		let mut changed = menu(); changed.product.sale_unit_list[0].product_sale_unit_id = "other-sale".into();
		assert!(quote().validate("account-a", 101, &changed, &balance).is_err());
		let mut changed = menu(); changed.product.sale_unit_list[0].end_sale_date = 100_000;
		assert!(quote().validate("account-a", 101, &changed, &balance).is_err());
		assert!(quote().validate("account-a", 101, &current, &CoinBalance { coin_type: "OTHER".into(), amount: 100 }).is_err());
		assert!(quote().validate("account-a", 101, &current, &CoinBalance { coin_type: "TW_WCOIN".into(), amount: 26 }).is_err());
	}

	#[aidoku_test]
	fn only_complete_payment_status_confirms_a_debit() {
		let paid: BuyResult = crate::models::decode(r#"{"message":{"result":{"payment":{"paymentStatus":"COMPLETE"}}}}"#).unwrap();
		assert!(paid.complete());
		let pending: BuyResult = crate::models::decode(r#"{"message":{"result":{"payment":{"paymentStatus":"PENDING"}}}}"#).unwrap();
		assert!(!pending.complete());
	}
}
