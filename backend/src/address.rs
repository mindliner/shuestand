use std::str::FromStr;
use std::sync::Arc;

use bdk::bitcoin::Network;
use bdk::descriptor::{Descriptor, DescriptorPublicKey};
use chrono::Utc;
use uuid::Uuid;

use crate::db::{AddressState, Database, NewAddress, PoolAddress};

#[derive(Clone)]
pub struct AddressPool {
    db: Database,
    factory: AddressFactory,
    target: u32,
}

impl AddressPool {
    pub fn new(db: Database, factory: AddressFactory, target: u32) -> Self {
        Self {
            db,
            factory,
            target: target.max(1),
        }
    }

    pub async fn ensure_capacity(&self) -> anyhow::Result<()> {
        let mut available = self.db.count_available_addresses().await? as u32;
        if available >= self.target {
            return Ok(());
        }

        let mut next_index = self
            .db
            .max_derivation_index()
            .await?
            .map(|v| v + 1)
            .unwrap_or(0);

        while available < self.target {
            let address = self.factory.derive(next_index as u32)?;
            let now = Utc::now();
            let new_address = NewAddress {
                id: format!("addr_{}", Uuid::new_v4()),
                derivation_index: next_index as u32,
                address,
                state: AddressState::Available,
                created_at: now,
                updated_at: now,
            };
            match self.db.insert_address(&new_address).await? {
                true => {
                    available += 1;
                    next_index += 1;
                }
                false => {
                    tracing::warn!(target: "backend", index = next_index, "address already exists, advancing");
                    next_index += 1;
                }
            }
        }

        Ok(())
    }

    pub async fn assign_address(&self, deposit_id: &str) -> anyhow::Result<PoolAddress> {
        loop {
            if let Some(candidate) = self.db.fetch_available_address().await? {
                if self.db.claim_address(&candidate.id, deposit_id).await? {
                    return Ok(candidate);
                }
                continue;
            }

            self.ensure_capacity().await?;
        }
    }
}

#[derive(Clone)]
pub enum AddressFactory {
    Descriptor {
        descriptor: Arc<Descriptor<DescriptorPublicKey>>,
        network: Network,
    },
    Mock,
}

impl AddressFactory {
    pub fn from_config(descriptor: Option<String>, network: Network) -> anyhow::Result<Self> {
        if let Some(raw) = descriptor {
            let descriptor = Descriptor::from_str(&raw)?;
            Ok(Self::Descriptor {
                descriptor: Arc::new(descriptor),
                network,
            })
        } else {
            tracing::warn!(
                target: "backend",
                "BITCOIN_DESCRIPTOR not set; using mock address generator"
            );
            Ok(Self::Mock)
        }
    }

    pub fn derive(&self, index: u32) -> anyhow::Result<String> {
        match self {
            AddressFactory::Descriptor {
                descriptor,
                network,
            } => {
                let derived = descriptor.at_derivation_index(index)?;
                let address = derived.address(*network)?;
                Ok(address.to_string())
            }
            AddressFactory::Mock => Ok(format!("bc1pshu{:08x}", index)),
        }
    }
}
