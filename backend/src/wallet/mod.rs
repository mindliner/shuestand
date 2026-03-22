mod multi;
mod swap;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet as CashuWallet;
use cdk_sqlite::wallet::WalletSqliteDatabase as CashuSqliteDatabase;
use rand::RngCore;
use rand::rngs::OsRng;
use tokio::sync::Mutex;

pub use multi::MultiMintWalletManager;
pub use swap::{MintSwapOutcome, MintSwapService};
pub type WalletHandle = Arc<Mutex<CashuWallet>>;

#[derive(Clone, Debug)]
pub struct WalletConfig {
    pub mint_url: String,
    pub wallet_dir: Option<PathBuf>,
}

impl WalletConfig {
    pub fn new(mint_url: impl Into<String>, wallet_dir: Option<PathBuf>) -> Self {
        Self {
            mint_url: mint_url.into(),
            wallet_dir,
        }
    }

    pub fn base_dir(&self) -> PathBuf {
        resolve_wallet_dir(self.wallet_dir.as_deref())
    }
}

pub async fn open_wallet(config: &WalletConfig) -> Result<WalletHandle> {
    let base_dir = config.base_dir();
    let seed = load_or_generate_seed(&base_dir)?;

    let db_path = base_dir.join("wallet.sqlite");
    ensure_parent_dir(&db_path)?;
    let db = CashuSqliteDatabase::new(db_path).await?;
    let localstore = Arc::new(db);

    let wallet = CashuWallet::new(&config.mint_url, CurrencyUnit::Sat, localstore, seed, None)?;
    let _ = wallet.recover_incomplete_sagas().await;

    Ok(Arc::new(Mutex::new(wallet)))
}

pub fn default_wallet_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".shuestand")
        .join("cashu")
}

pub fn resolve_wallet_dir(wallet_dir: Option<&Path>) -> PathBuf {
    wallet_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_wallet_dir)
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn load_or_generate_seed(wallet_dir: &Path) -> std::io::Result<[u8; 64]> {
    fs::create_dir_all(wallet_dir)?;

    let seed_path = wallet_dir.join("seed");
    match fs::read(&seed_path) {
        Ok(bytes) if bytes.len() == 64 => {
            let mut seed = [0u8; 64];
            seed.copy_from_slice(&bytes);
            Ok(seed)
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Seed file must contain exactly 64 bytes: {}",
                seed_path.display()
            ),
        )),
        Err(_) => {
            let mut seed = [0u8; 64];
            OsRng.fill_bytes(&mut seed);
            fs::write(&seed_path, &seed)?;
            Ok(seed)
        }
    }
}
