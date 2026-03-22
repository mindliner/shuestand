use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use tokio::sync::RwLock;

use super::{WalletConfig, WalletHandle, open_wallet};

pub struct MultiMintWalletManager {
    canonical_mint: String,
    canonical_wallet: WalletHandle,
    base_dir: PathBuf,
    foreign_wallets: RwLock<HashMap<String, WalletHandle>>,
}

impl MultiMintWalletManager {
    pub fn new(canonical_mint: String, canonical_wallet: WalletHandle, base_dir: PathBuf) -> Self {
        Self {
            canonical_mint,
            canonical_wallet,
            base_dir,
            foreign_wallets: RwLock::new(HashMap::new()),
        }
    }

    pub fn canonical_mint(&self) -> &str {
        &self.canonical_mint
    }

    pub fn canonical_wallet(&self) -> WalletHandle {
        self.canonical_wallet.clone()
    }

    pub async fn wallet_for_mint(&self, mint_url: &str) -> anyhow::Result<WalletHandle> {
        if mint_url == self.canonical_mint {
            return Ok(self.canonical_wallet());
        }

        if let Some(handle) = self.foreign_wallets.read().await.get(mint_url) {
            return Ok(handle.clone());
        }

        let mut guard = self.foreign_wallets.write().await;
        if let Some(existing) = guard.get(mint_url) {
            return Ok(existing.clone());
        }

        let wallet_dir = self.foreign_wallet_dir(mint_url);
        let config = WalletConfig::new(mint_url.to_string(), Some(wallet_dir.clone()));
        let wallet = open_wallet(&config)
            .await
            .with_context(|| format!("opening wallet for foreign mint {mint_url}"))?;
        guard.insert(mint_url.to_string(), wallet.clone());
        Ok(wallet)
    }

    fn foreign_wallet_dir(&self, mint_url: &str) -> PathBuf {
        let slug = mint_slug(mint_url);
        self.base_dir.join("foreign").join(slug)
    }
}

fn mint_slug(mint_url: &str) -> String {
    let mut slug = String::with_capacity(mint_url.len().min(64));
    for ch in mint_url.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else {
            slug.push('_');
        }
        if slug.len() >= 96 {
            break;
        }
    }
    if slug.is_empty() { "mint".into() } else { slug }
}
