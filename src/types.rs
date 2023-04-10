pub use mc_transaction_types::{Amount, TokenId};
use rust_decimal::{prelude::*, Decimal};
use std::str::FromStr;

pub struct TokenInfo {
    pub token_id: TokenId,
    pub symbol: String,
    pub fee: u64,
    pub decimals: u32,
}

impl TokenInfo {
    /// Try parsing a user-specified, scaled value, and modify decimals to make it
    /// a u64 in the smallest representable units
    pub fn try_scaled_to_u64(&self, scaled_value_str: &str) -> Result<u64, String> {
        let parsed_value = Decimal::from_str(scaled_value_str).map_err(|err| err.to_string())?;
        let scale = Decimal::new(1, self.decimals);
        let rescaled_value = parsed_value
            .checked_div(scale)
            .ok_or("decimal overflow".to_string())?;
        let u64_value = rescaled_value
            .round()
            .to_u64()
            .ok_or("u64 overflow".to_string())?;
        Ok(u64_value)
    }
}
