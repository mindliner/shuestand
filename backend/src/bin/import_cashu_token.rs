use std::path::PathBuf;

use backend::wallet::{WalletConfig, open_wallet};
use cdk::wallet::ReceiveOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let token_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/cashu_token.txt".to_string());
    let token = std::fs::read_to_string(&token_path)?;
    let token = token.trim();

    if token.is_empty() {
        anyhow::bail!("token file {token_path} was empty");
    }

    let mint_url = std::env::var("CASHU_MINT_URL")?;
    let wallet_dir = std::env::var("CASHU_WALLET_DIR").ok().map(PathBuf::from);

    let config = WalletConfig::new(mint_url, wallet_dir);
    let wallet = open_wallet(&config).await?;
    let amount = wallet
        .lock()
        .await
        .receive(token, ReceiveOptions::default())
        .await?;

    println!(
        "Imported {} sats into {}",
        amount.to_u64(),
        config.base_dir().display()
    );
    Ok(())
}
