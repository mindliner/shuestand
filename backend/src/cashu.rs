use std::str::FromStr;

use cdk::nuts::Token;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenMintError {
    #[error("token is malformed")]
    Malformed,
    #[error("token references multiple or unsupported mints")]
    MultiMint,
}

pub fn token_mint_url(token_raw: &str) -> Result<String, TokenMintError> {
    let parsed = Token::from_str(token_raw).map_err(|_| TokenMintError::Malformed)?;
    let mint_url = parsed
        .mint_url()
        .map_err(|_| TokenMintError::MultiMint)?
        .to_string();
    Ok(mint_url)
}

pub fn token_total_amount(token_raw: &str) -> Result<u64, TokenMintError> {
    let parsed = Token::from_str(token_raw).map_err(|_| TokenMintError::Malformed)?;
    let amount = parsed
        .value()
        .map_err(|_| TokenMintError::MultiMint)?
        .to_u64();
    Ok(amount)
}

pub fn token_fingerprint(token_raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token_raw.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}
