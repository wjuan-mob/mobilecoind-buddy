pub use mc_transaction_types::TokenId;

pub struct TokenInfo {
    pub token_id: TokenId,
    pub symbol: String,
    pub fee: u64,
    pub decimals: u32,
}
