use crate::Config;
use mc_transaction_types::TokenId;
//use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};

pub struct TokenInfo {
    pub token_id: TokenId,
    pub symbol: String,
    pub fee: u64,
    pub decimals: u32,
}

pub struct Worker {
    config: Config,
    errors: VecDeque<String>,
}

impl Worker {
    pub fn new(config: Config) -> Result<Self, WorkerInitError> {
        Ok(Worker {
            config,
            errors: Default::default(),
        })
    }

    pub fn get_b58_address(&self) -> String {
        "2D9XJuEn1dBZdNudPRjH9ZifaiNUKJP8VNGoGCLbGMmBb8sDy7vsFfnTAH5EnhAj6kLGTWS5A2RjfLBXN7frqR6NbsezMv9og1KPJMpRTrG".to_string()
    }

    pub fn get_sync_percent(&self) -> String {
        "97".to_string()
    }

    pub fn get_token_info(&self) -> Vec<TokenInfo> {
        vec![
            TokenInfo {
                token_id: 0.into(),
                symbol: "MOB".to_string(),
                fee: 9999,
                decimals: 12,
            },
            TokenInfo {
                token_id: 1.into(),
                symbol: "EUSD".to_string(),
                fee: 9999,
                decimals: 6,
            },
            TokenInfo {
                token_id: 8192.into(),
                symbol: "FauxUSD".to_string(),
                fee: 9999,
                decimals: 6,
            },
        ]
    }

    pub fn get_balances(&self) -> HashMap<TokenId, u64> {
        let mut result = HashMap::default();
        result.insert(TokenId::from(0), 10_000_000_000_000);
        result.insert(TokenId::from(1), 60_000_000);
        result.insert(TokenId::from(8192), 90_000_000);
        result
    }

    pub fn send(&mut self, value: u64, token_id: TokenId, recipient: String) {
        eprintln!("send: {} of {} to {}", value, token_id, recipient);
    }

    pub fn top_error(&self) -> Option<String> {
        self.errors.get(0).cloned()
    }

    pub fn pop_error(&mut self) {
        self.errors.pop_front();
    }
}

/// An error returned by the worker that prevented initialization.
/// Errors that occur after initalization are logged, and sent to the self.errors queue for display to the user.
#[derive(Clone, Debug)]
pub enum WorkerInitError {}
