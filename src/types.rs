pub use mc_transaction_types::{Amount, TokenId};

use mc_transaction_extra::{SignedContingentInput, SignedContingentInputAmounts};
use rust_decimal::{prelude::*, Decimal};
use std::str::FromStr;

/// Info available about a particular token id, which can be used to display it,
/// or to compute fees.
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

/// A validated quote that we got from the deqs
#[derive(Clone, Debug)]
pub struct ValidatedQuote {
    /// The sci, needed when we match against the quote
    pub sci: SignedContingentInput,
    /// The sci amounts, produced by sci.validate(). Needed to help with partial fill arithmetic.
    pub amounts: SignedContingentInputAmounts,
    /// u64 timestamp
    pub timestamp: u64,
}

impl TryFrom<&deqs_api::deqs::Quote> for ValidatedQuote {
    type Error = String;
    fn try_from(src: &deqs_api::deqs::Quote) -> Result<Self, Self::Error> {
        let sci = SignedContingentInput::try_from(src.get_sci()).map_err(|err| err.to_string())?;
        let amounts = sci.validate().map_err(|err| err.to_string())?;
        let timestamp = src.timestamp;

        Ok(Self {
            sci,
            amounts,
            timestamp,
        })
    }
}

impl ValidatedQuote {
    /// Get information to render this quote as part of a quote book.
    /// Depending on which is the base and which is the counter, this ends up on the bid or ask side.
    /// TokenInfo are used to scale the token amounts appropriately for display.
    pub fn get_quote_info(&self, base_token_id: TokenId, counter_token_id: TokenId, token_infos: &[TokenInfo]) -> Result<QuoteInfo, String> {
        let base_token_info: &TokenInfo = token_infos
           .iter()
           .find(|info| info.token_id == base_token_id).ok_or("missing base token info".to_owned())?;

        let counter_token_info: &TokenInfo = token_infos
           .iter()
           .find(|info| info.token_id == counter_token_id).ok_or("missing counter token info".to_owned())?;

        if self.amounts.pseudo_output.token_id == base_token_id {
            // Quote is offering the base token, so this should be an ask
            let quote_side = QuoteSide::Ask;

            if let Some(partial_fill_change) = self.amounts.partial_fill_change.as_ref() {
                if &self.amounts.pseudo_output != partial_fill_change {
                    return Err("Ask SCI is too complicated for this implementation (partial fill change not equal to pseudo output)".to_owned());
                }
                if !self.amounts.required_outputs.is_empty() {
                    return Err("Ask SCI is too complicated for this implementation (mixing partial fill and required outputs)".to_owned());
                }
                if self.amounts.partial_fill_outputs.len() != 1 {
                    return Err("Ask SCI is too complicated for this implementation (expected one partial fill output)".to_owned());
                }
                if self.amounts.partial_fill_outputs[0].token_id != counter_token_id {
                    return Err("Ask SCI does not belong to this book (partial fill output)".to_owned());
                }
                // TODO: should handle overflow at i64 conversion
                let volume = Decimal::new(self.amounts.pseudo_output.value as i64, base_token_info.decimals);
                let counter_volume = Decimal::new(self.amounts.partial_fill_outputs[0].value as i64, counter_token_info.decimals);
                let price = counter_volume / volume;
                Ok(QuoteInfo {
                    quote_side,
                    price,
                    volume,
                    is_partial_fill: true,
                    timestamp: self.timestamp,
                })
            } else {
                if !self.amounts.partial_fill_outputs.is_empty() {
                    return Err("Invalid Ask SCI".to_owned());
                }
                if self.amounts.required_outputs.len() != 1 {
                    return Err("Ask SCI is too complicated for this implementation (expected one required output)".to_owned());
                }
                if self.amounts.required_outputs[0].token_id != counter_token_id {
                    return Err("Ask SCI does not belong to this book (required_output)".to_owned());
                }
                // TODO: should handle overflow at i64 conversion
                let volume = Decimal::new(self.amounts.pseudo_output.value as i64, base_token_info.decimals);
                let counter_volume = Decimal::new(self.amounts.required_outputs[0].value as i64, counter_token_info.decimals);
                let price = counter_volume / volume;
                Ok(QuoteInfo {
                    quote_side,
                    price,
                    volume,
                    is_partial_fill: false,
                    timestamp: self.timestamp,
                })
            }

        } else if self.amounts.pseudo_output.token_id == counter_token_id {
            // Quote is offering the counter token, so this should be an bid
            let quote_side = QuoteSide::Bid;

            if let Some(partial_fill_change) = self.amounts.partial_fill_change.as_ref() {
                if &self.amounts.pseudo_output != partial_fill_change {
                    return Err("Bid SCI is too complicated for this implementation (partial fill change not equal to pseudo output)".to_owned());
                }
                if !self.amounts.required_outputs.is_empty() {
                    return Err("Bid SCI is too complicated for this implementation (mixing partial fill and required outputs)".to_owned());
                }
                if self.amounts.partial_fill_outputs.len() != 1 {
                    return Err("Bid SCI is too complicated for this implementation (expected one partial fill output)".to_owned());
                }
                if self.amounts.partial_fill_outputs[0].token_id != base_token_id {
                    return Err("Bid SCI does not belong to this book (partial fill output)".to_owned());
                }
                // TODO: should handle overflow at i64 conversion
                let counter_volume = Decimal::new(self.amounts.pseudo_output.value as i64, counter_token_info.decimals);
                let volume = Decimal::new(self.amounts.partial_fill_outputs[0].value as i64, base_token_info.decimals);
                let price = counter_volume / volume;
                Ok(QuoteInfo {
                    quote_side,
                    price,
                    volume,
                    is_partial_fill: true,
                    timestamp: self.timestamp,
                })
            } else {
                if !self.amounts.partial_fill_outputs.is_empty() {
                    return Err("Invalid Bid SCI".to_owned());
                }
                if self.amounts.required_outputs.len() != 1 {
                    return Err("Bid SCI is too complicated for this implementation (expected one required output)".to_owned());
                }
                if self.amounts.required_outputs[0].token_id != base_token_id {
                    return Err("Bid SCI does not belong to this book (required_output)".to_owned());
                }
                // TODO: should handle overflow at i64 conversion
                let counter_volume = Decimal::new(self.amounts.pseudo_output.value as i64, counter_token_info.decimals);
                let volume = Decimal::new(self.amounts.required_outputs[0].value as i64, base_token_info.decimals);
                let price = counter_volume / volume;
                Ok(QuoteInfo {
                    quote_side,
                    price,
                    volume,
                    is_partial_fill: false,
                    timestamp: self.timestamp,
                })
            }
        } else {
            Err("SCI does not belong to this book (pseudo-output)".to_owned())
        }
    }
}

#[derive(Clone, Debug)]
pub enum QuoteSide {
    Bid,
    Ask
}

/// Information about a quote that we render in the ui
pub struct QuoteInfo {
    /// Which side of the book this quote is on.
    /// This is relative to a particular pair being displayed
    pub quote_side: QuoteSide,

    /// The price of the base token in units of the counter token, implied by this quote
    pub price: Decimal,
    
    /// The maximum volume of base token for this quote.
    pub volume: Decimal,

    /// Whether this is a partial fill quote
    pub is_partial_fill: bool,

    /// Timestamp of the quote
    pub timestamp: u64,
}
