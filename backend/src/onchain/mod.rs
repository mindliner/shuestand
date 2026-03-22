mod postgres_db;

use self::postgres_db::PgWalletDatabase;
use crate::db::Database;
use anyhow::{Context, Result};
use bdk::Error as BdkError;
use bdk::bitcoin::{Address, Network};
use bdk::blockchain::electrum::{ElectrumBlockchain, ElectrumBlockchainConfig};
use bdk::blockchain::esplora::{EsploraBlockchain, EsploraError};
use bdk::blockchain::{Blockchain, ConfigurableBlockchain};
use bdk::database::{BatchDatabase, MemoryDatabase};
use bdk::wallet::{AddressIndex, Wallet};
use bdk::{Balance, FeeRate, SignOptions, SyncOptions};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::task::spawn_blocking;
use uuid::Uuid;

enum WalletBackend {
    Memory(Wallet<MemoryDatabase>),
    Postgres(Wallet<PgWalletDatabase>),
}

const CHAIN_STOP_GAP: usize = 10;
const ESPLORA_MAX_RETRIES: usize = 6;

#[derive(Clone)]
pub enum BlockchainClient {
    Esplora(Arc<EsploraBlockchain>),
    Electrum(Arc<ElectrumBlockchain>),
}

impl BlockchainClient {
    pub fn from_esplora(base_url: &str) -> Self {
        let client = EsploraBlockchain::new(base_url, CHAIN_STOP_GAP).with_concurrency(1);
        Self::Esplora(Arc::new(client))
    }

    pub fn from_electrum(blockchain: ElectrumBlockchain) -> Self {
        Self::Electrum(Arc::new(blockchain))
    }

    pub fn from_electrum_config(
        url: &str,
        socks5: Option<&str>,
        retry: u8,
        timeout_secs: Option<u8>,
        validate_domain: bool,
    ) -> Result<Self, BdkError> {
        let config = ElectrumBlockchainConfig {
            url: url.to_string(),
            socks5: socks5.map(|v| v.to_string()),
            retry,
            timeout: timeout_secs,
            stop_gap: CHAIN_STOP_GAP,
            validate_domain,
        };
        let blockchain = ElectrumBlockchain::from_config(&config)?;
        Ok(Self::from_electrum(blockchain))
    }

    fn sync_wallet<D: BatchDatabase>(&self, wallet: &mut Wallet<D>) -> Result<(), BdkError> {
        match self {
            BlockchainClient::Esplora(client) => {
                wallet.sync(client.as_ref(), SyncOptions::default())
            }
            BlockchainClient::Electrum(client) => {
                wallet.sync(client.as_ref(), SyncOptions::default())
            }
        }
    }

    fn broadcast(&self, tx: &bdk::bitcoin::Transaction) -> Result<(), BdkError> {
        match self {
            BlockchainClient::Esplora(client) => Blockchain::broadcast(client.as_ref(), tx),
            BlockchainClient::Electrum(client) => Blockchain::broadcast(client.as_ref(), tx),
        }
    }
}

impl WalletBackend {
    fn get_balance(&self) -> Result<Balance, BdkError> {
        match self {
            WalletBackend::Memory(wallet) => wallet.get_balance(),
            WalletBackend::Postgres(wallet) => wallet.get_balance(),
        }
    }

    fn sync_with(&mut self, blockchain: &BlockchainClient) -> Result<(), BdkError> {
        let mut attempt = 0;
        loop {
            let result = match self {
                WalletBackend::Memory(wallet) => blockchain.sync_wallet(wallet),
                WalletBackend::Postgres(wallet) => blockchain.sync_wallet(wallet),
            };

            match result {
                Ok(_) => return Ok(()),
                Err(err) if is_rate_limited(&err) && attempt < ESPLORA_MAX_RETRIES => {
                    let backoff = Duration::from_secs(2u64.pow(attempt as u32));
                    tracing::warn!(
                        target: "backend",
                        attempt,
                        backoff_secs = backoff.as_secs(),
                        "esplora rate-limited sync; retrying"
                    );
                    thread::sleep(backoff);
                    attempt += 1;
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn next_external_address(&mut self) -> Result<String, BdkError> {
        match self {
            WalletBackend::Memory(wallet) => wallet
                .get_address(AddressIndex::New)
                .map(|info| info.address.to_string()),
            WalletBackend::Postgres(wallet) => wallet
                .get_address(AddressIndex::New)
                .map(|info| info.address.to_string()),
        }
    }

    fn send_to(
        &mut self,
        destination: &Address,
        amount_sats: u64,
        fee_rate_vb: f32,
        blockchain: &BlockchainClient,
    ) -> Result<String, BdkError> {
        self.sync_with(blockchain)?;
        match self {
            WalletBackend::Memory(wallet) => {
                broadcast_payment(wallet, destination, amount_sats, fee_rate_vb, blockchain)
            }
            WalletBackend::Postgres(wallet) => {
                broadcast_payment(wallet, destination, amount_sats, fee_rate_vb, blockchain)
            }
        }
    }
}

fn broadcast_payment<D: BatchDatabase>(
    wallet: &mut Wallet<D>,
    destination: &Address,
    amount_sats: u64,
    fee_rate_vb: f32,
    blockchain: &BlockchainClient,
) -> Result<String, BdkError> {
    let mut builder = wallet.build_tx();
    builder.set_recipients(vec![(destination.script_pubkey(), amount_sats)]);
    builder.fee_rate(FeeRate::from_sat_per_vb(fee_rate_vb));

    let (mut psbt, details) = builder.finish()?;
    wallet.sign(&mut psbt, SignOptions::default())?;
    blockchain.broadcast(&psbt.extract_tx())?;

    Ok(details.txid.to_string())
}

pub struct OnchainWallet {
    wallet: Arc<Mutex<WalletBackend>>,
    blockchain: BlockchainClient,
    network: Network,
}

pub struct OnchainBalance {
    pub confirmed: u64,
    pub trusted_pending: u64,
    pub untrusted_pending: u64,
    pub immature: u64,
}

impl OnchainWallet {
    pub async fn new(
        db: &Database,
        spend_descriptor: &str,
        change_descriptor: Option<&str>,
        network: Network,
        blockchain: BlockchainClient,
    ) -> Result<Self> {
        let wallet_id = derive_wallet_id(spend_descriptor, change_descriptor, network);
        let pg_wallet = PgWalletDatabase::new(
            db.pool.clone(),
            wallet_id,
            spend_descriptor,
            change_descriptor,
            &network.to_string(),
        )
        .await?;
        let wallet = Wallet::new(spend_descriptor, change_descriptor, network, pg_wallet)?;
        Ok(Self::from_backend(
            WalletBackend::Postgres(wallet),
            network,
            blockchain,
        ))
    }

    pub fn new_in_memory(
        spend_descriptor: &str,
        change_descriptor: Option<&str>,
        network: Network,
        blockchain: BlockchainClient,
    ) -> Result<Self> {
        let wallet = Wallet::new(
            spend_descriptor,
            change_descriptor,
            network,
            MemoryDatabase::default(),
        )?;
        Ok(Self::from_backend(
            WalletBackend::Memory(wallet),
            network,
            blockchain,
        ))
    }

    fn from_backend(
        backend: WalletBackend,
        network: Network,
        blockchain: BlockchainClient,
    ) -> Self {
        Self {
            wallet: Arc::new(Mutex::new(backend)),
            blockchain,
            network,
        }
    }

    pub async fn sync(&self) -> Result<()> {
        let wallet = self.wallet.clone();
        let blockchain = self.blockchain.clone();
        spawn_blocking(move || {
            let mut guard = wallet.lock().expect("wallet mutex poisoned");
            guard.sync_with(&blockchain)
        })
        .await??;
        Ok(())
    }

    pub async fn balance(&self) -> Result<OnchainBalance> {
        let wallet = self.wallet.clone();
        let summary = spawn_blocking(move || {
            let guard = wallet.lock().expect("wallet mutex poisoned");
            guard.get_balance()
        })
        .await??;
        Ok(OnchainBalance {
            confirmed: summary.confirmed,
            trusted_pending: summary.trusted_pending,
            untrusted_pending: summary.untrusted_pending,
            immature: summary.immature,
        })
    }

    pub async fn next_external_address(&self) -> Result<String> {
        let wallet = self.wallet.clone();
        let address = spawn_blocking(move || {
            let mut guard = wallet.lock().expect("wallet mutex poisoned");
            guard.next_external_address()
        })
        .await??;
        Ok(address)
    }

    pub async fn send_to_address(
        &self,
        address: &str,
        amount_sats: u64,
        fee_rate_vb: f32,
    ) -> Result<String> {
        let destination = Address::from_str(address)
            .context("parsing destination address")?
            .require_network(self.network)
            .context("address network mismatch")?;
        let wallet = self.wallet.clone();
        let blockchain = self.blockchain.clone();

        let txid = spawn_blocking(move || {
            let mut guard = wallet.lock().expect("wallet mutex poisoned");
            guard.send_to(&destination, amount_sats, fee_rate_vb, &blockchain)
        })
        .await??;
        Ok(txid)
    }
}

fn is_rate_limited(err: &BdkError) -> bool {
    matches!(err, BdkError::Esplora(inner) if matches!(inner.as_ref(), EsploraError::HttpResponse(429)))
}

fn derive_wallet_id(
    spend_descriptor: &str,
    change_descriptor: Option<&str>,
    network: Network,
) -> Uuid {
    let payload = format!(
        "{}|{}|{}",
        spend_descriptor,
        change_descriptor.unwrap_or(""),
        network
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, payload.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bdk::bitcoin::bip32::ExtendedPrivKey;

    const TEST_SEED: [u8; 32] = [42u8; 32];
    const TEST_ESPLORA: &str = "https://mempool.space/signet/api";

    fn test_descriptors() -> (String, String) {
        let xprv = ExtendedPrivKey::new_master(Network::Signet, &TEST_SEED)
            .expect("static seed should produce xprv");
        let external = format!("wpkh({}/84'/1'/0'/0/*)", xprv);
        let change = format!("wpkh({}/84'/1'/0'/1/*)", xprv);
        (external, change)
    }

    #[tokio::test]
    async fn next_address_advances_without_sync() -> Result<()> {
        let (external, change) = test_descriptors();
        let wallet = OnchainWallet::new_in_memory(
            &external,
            Some(&change),
            Network::Signet,
            BlockchainClient::from_esplora(TEST_ESPLORA),
        )?;

        let first = wallet.next_external_address().await?;
        let second = wallet.next_external_address().await?;

        assert_ne!(first, second, "derivations must advance index");
        assert!(first.starts_with("tb1") && second.starts_with("tb1"));
        Ok(())
    }

    #[tokio::test]
    async fn empty_balance_reads_zeroes() -> Result<()> {
        let (external, change) = test_descriptors();
        let wallet = OnchainWallet::new_in_memory(
            &external,
            Some(&change),
            Network::Signet,
            BlockchainClient::from_esplora(TEST_ESPLORA),
        )?;

        let balance = wallet.balance().await?;
        assert_eq!(balance.confirmed, 0);
        assert_eq!(balance.trusted_pending, 0);
        assert_eq!(balance.untrusted_pending, 0);
        assert_eq!(balance.immature, 0);
        Ok(())
    }
}
