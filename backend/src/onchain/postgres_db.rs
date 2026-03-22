use std::future::Future;

use bdk::bitcoin::consensus::encode::{deserialize, serialize};
use bdk::bitcoin::hashes::Hash as BitcoinHash;
use bdk::bitcoin::{OutPoint, Script, ScriptBuf, Transaction, TxOut, Txid};
use bdk::database::{BatchDatabase, BatchOperations, Database, SyncTime};
use bdk::{BlockTime, Error as BdkError, KeychainKind, LocalUtxo, TransactionDetails};
use sqlx::{PgPool, Row};
use tokio::runtime::Handle;
use tokio::task::block_in_place;
use uuid::Uuid;

#[derive(Clone)]
pub struct PgWalletDatabase {
    pool: PgPool,
    wallet_id: Uuid,
    handle: Handle,
}

impl PgWalletDatabase {
    pub async fn new(
        pool: PgPool,
        wallet_id: Uuid,
        spend_descriptor: &str,
        change_descriptor: Option<&str>,
        network: &str,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO onchain_wallets (id, spend_descriptor, change_descriptor, network)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (id) DO UPDATE
               SET spend_descriptor = EXCLUDED.spend_descriptor,
                   change_descriptor = EXCLUDED.change_descriptor,
                   network = EXCLUDED.network,
                   updated_at = NOW()"#,
        )
        .bind(wallet_id)
        .bind(spend_descriptor)
        .bind(change_descriptor)
        .bind(network)
        .execute(&pool)
        .await?;

        Ok(Self {
            pool,
            wallet_id,
            handle: Handle::current(),
        })
    }

    fn run<F, T>(&self, fut: F) -> Result<T, BdkError>
    where
        F: Future<Output = Result<T, sqlx::Error>>,
    {
        block_in_place(|| self.handle.block_on(fut)).map_err(map_sqlx_err)
    }
}

fn map_sqlx_err(err: sqlx::Error) -> BdkError {
    BdkError::Generic(format!("postgres wallet db error: {err}"))
}

fn encode_txid(txid: &Txid) -> Vec<u8> {
    txid.as_byte_array().to_vec()
}

fn decode_txid(bytes: &[u8]) -> Result<Txid, BdkError> {
    Txid::from_slice(bytes).map_err(|e| BdkError::Generic(format!("invalid txid bytes: {e}")))
}

fn encode_keychain(kind: KeychainKind) -> i16 {
    match kind {
        KeychainKind::External => 0,
        KeychainKind::Internal => 1,
    }
}

fn decode_keychain(value: i16) -> Result<KeychainKind, BdkError> {
    match value {
        0 => Ok(KeychainKind::External),
        1 => Ok(KeychainKind::Internal),
        other => Err(BdkError::Generic(format!("unknown keychain kind: {other}"))),
    }
}

fn encode_script(script: &Script) -> Vec<u8> {
    script.as_bytes().to_vec()
}

fn decode_script(bytes: &[u8]) -> Result<ScriptBuf, BdkError> {
    Ok(ScriptBuf::from_bytes(bytes.to_vec()))
}

fn encode_transaction(tx: &Transaction) -> Vec<u8> {
    serialize(tx)
}

fn decode_transaction(bytes: &[u8]) -> Result<Transaction, BdkError> {
    deserialize(bytes).map_err(|e| BdkError::Generic(format!("invalid transaction bytes: {e}")))
}

fn encode_block_time(sync_time: SyncTime) -> (i32, i64) {
    (
        sync_time.block_time.height as i32,
        sync_time.block_time.timestamp as i64,
    )
}

fn decode_block_time(height: i32, timestamp: i64) -> SyncTime {
    SyncTime {
        block_time: BlockTime {
            height: height as u32,
            timestamp: timestamp as u64,
        },
    }
}

fn encode_txout(txout: &TxOut) -> (Vec<u8>, i64) {
    (encode_script(&txout.script_pubkey), txout.value as i64)
}

fn decode_txout(script: Vec<u8>, value: i64) -> Result<TxOut, BdkError> {
    Ok(TxOut {
        value: value as u64,
        script_pubkey: decode_script(&script)?,
    })
}

#[derive(Default)]
pub struct PgWalletBatch {
    ops: Vec<BatchOp>,
}

enum BatchOp {
    SetScript {
        script: ScriptBuf,
        keychain: KeychainKind,
        child: u32,
    },
    SetUtxo {
        utxo: LocalUtxo,
    },
    SetRawTx {
        tx: Transaction,
    },
    SetTx {
        details: TransactionDetails,
    },
    SetLastIndex {
        keychain: KeychainKind,
        value: u32,
    },
    SetSyncTime {
        sync: SyncTime,
    },
    DelScriptByPath {
        keychain: KeychainKind,
        child: u32,
    },
    DelScriptByScript {
        script: ScriptBuf,
    },
    DelUtxo {
        outpoint: OutPoint,
    },
    DelRawTx {
        txid: Txid,
    },
    DelTx {
        txid: Txid,
        include_raw: bool,
    },
    DelLastIndex {
        keychain: KeychainKind,
    },
    DelSyncTime,
}

impl BatchOperations for PgWalletBatch {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<(), BdkError> {
        self.ops.push(BatchOp::SetScript {
            script: ScriptBuf::from(script),
            keychain,
            child,
        });
        Ok(())
    }

    fn set_utxo(&mut self, utxo: &LocalUtxo) -> Result<(), BdkError> {
        self.ops.push(BatchOp::SetUtxo { utxo: utxo.clone() });
        Ok(())
    }

    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), BdkError> {
        self.ops.push(BatchOp::SetRawTx {
            tx: transaction.clone(),
        });
        Ok(())
    }

    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), BdkError> {
        self.ops.push(BatchOp::SetTx {
            details: transaction.clone(),
        });
        Ok(())
    }

    fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), BdkError> {
        self.ops.push(BatchOp::SetLastIndex { keychain, value });
        Ok(())
    }

    fn set_sync_time(&mut self, data: SyncTime) -> Result<(), BdkError> {
        self.ops.push(BatchOp::SetSyncTime { sync: data });
        Ok(())
    }

    fn del_script_pubkey_from_path(
        &mut self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<ScriptBuf>, BdkError> {
        self.ops.push(BatchOp::DelScriptByPath { keychain, child });
        Ok(None)
    }

    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, BdkError> {
        self.ops.push(BatchOp::DelScriptByScript {
            script: ScriptBuf::from(script),
        });
        Ok(None)
    }

    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, BdkError> {
        self.ops.push(BatchOp::DelUtxo {
            outpoint: *outpoint,
        });
        Ok(None)
    }

    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, BdkError> {
        self.ops.push(BatchOp::DelRawTx { txid: *txid });
        Ok(None)
    }

    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, BdkError> {
        self.ops.push(BatchOp::DelTx {
            txid: *txid,
            include_raw,
        });
        Ok(None)
    }

    fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, BdkError> {
        self.ops.push(BatchOp::DelLastIndex { keychain });
        Ok(None)
    }

    fn del_sync_time(&mut self) -> Result<Option<SyncTime>, BdkError> {
        self.ops.push(BatchOp::DelSyncTime);
        Ok(None)
    }
}

impl BatchOperations for PgWalletDatabase {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<(), BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let script_bytes = encode_script(script);
        let keychain_code = encode_keychain(keychain);
        let child_index = child as i32;
        self.run(async move {
            sqlx::query(
                r#"INSERT INTO onchain_wallet_scripts (wallet_id, keychain, child_index, script)
                   VALUES ($1, $2, $3, $4)
                   ON CONFLICT (wallet_id, keychain, child_index)
                   DO UPDATE SET script = EXCLUDED.script, updated_at = NOW()"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .bind(child_index)
            .bind(script_bytes)
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn set_utxo(&mut self, utxo: &LocalUtxo) -> Result<(), BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid = encode_txid(&utxo.outpoint.txid);
        let vout = utxo.outpoint.vout as i32;
        let (script, value) = encode_txout(&utxo.txout);
        let keychain_code = encode_keychain(utxo.keychain);
        let is_spent = utxo.is_spent;
        self.run(async move {
            sqlx::query(
                r#"INSERT INTO onchain_wallet_utxos (wallet_id, txid, vout, script, value, keychain, is_spent)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (wallet_id, txid, vout)
                   DO UPDATE SET script = EXCLUDED.script,
                                 value = EXCLUDED.value,
                                 keychain = EXCLUDED.keychain,
                                 is_spent = EXCLUDED.is_spent,
                                 updated_at = NOW()"#,
            )
            .bind(wallet_id)
            .bind(txid)
            .bind(vout)
            .bind(script)
            .bind(value)
            .bind(keychain_code)
            .bind(is_spent)
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid = encode_txid(&transaction.txid());
        let tx_bytes = encode_transaction(transaction);
        self.run(async move {
            sqlx::query(
                r#"INSERT INTO onchain_wallet_raw_txs (wallet_id, txid, transaction)
                   VALUES ($1, $2, $3)
                   ON CONFLICT (wallet_id, txid)
                   DO UPDATE SET transaction = EXCLUDED.transaction, updated_at = NOW()"#,
            )
            .bind(wallet_id)
            .bind(txid)
            .bind(tx_bytes)
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), BdkError> {
        if let Some(tx) = &transaction.transaction {
            self.set_raw_tx(tx)?;
        }
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid = encode_txid(&transaction.txid);
        let received = transaction.received as i64;
        let sent = transaction.sent as i64;
        let fee = transaction.fee.map(|v| v as i64);
        let (conf_height, conf_timestamp) = transaction
            .confirmation_time
            .as_ref()
            .map(|bt| (Some(bt.height as i32), Some(bt.timestamp as i64)))
            .unwrap_or((None, None));
        self.run(async move {
            sqlx::query(
                r#"INSERT INTO onchain_wallet_txs (wallet_id, txid, received, sent, fee, confirmation_height, confirmation_timestamp)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (wallet_id, txid)
                   DO UPDATE SET received = EXCLUDED.received,
                                 sent = EXCLUDED.sent,
                                 fee = EXCLUDED.fee,
                                 confirmation_height = EXCLUDED.confirmation_height,
                                 confirmation_timestamp = EXCLUDED.confirmation_timestamp,
                                 updated_at = NOW()"#,
            )
            .bind(wallet_id)
            .bind(txid)
            .bind(received)
            .bind(sent)
            .bind(fee)
            .bind(conf_height)
            .bind(conf_timestamp)
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_code = encode_keychain(keychain);
        let val = value as i32;
        self.run(async move {
            sqlx::query(
                r#"INSERT INTO onchain_wallet_last_indices (wallet_id, keychain, value)
                   VALUES ($1, $2, $3)
                   ON CONFLICT (wallet_id, keychain)
                   DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .bind(val)
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn set_sync_time(&mut self, data: SyncTime) -> Result<(), BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let (height, timestamp) = encode_block_time(data);
        self.run(async move {
            sqlx::query(
                r#"INSERT INTO onchain_wallet_sync_times (wallet_id, block_height, block_timestamp)
                   VALUES ($1, $2, $3)
                   ON CONFLICT (wallet_id)
                   DO UPDATE SET block_height = EXCLUDED.block_height,
                                 block_timestamp = EXCLUDED.block_timestamp,
                                 updated_at = NOW()"#,
            )
            .bind(wallet_id)
            .bind(height)
            .bind(timestamp)
            .execute(&pool)
            .await?;
            Ok(())
        })
    }

    fn del_script_pubkey_from_path(
        &mut self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<ScriptBuf>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_code = encode_keychain(keychain);
        let child_index = child as i32;
        let result = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_scripts
                   WHERE wallet_id = $1 AND keychain = $2 AND child_index = $3
                   RETURNING script"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .bind(child_index)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = result {
            let bytes: Vec<u8> = row.get("script");
            Ok(Some(decode_script(&bytes)?))
        } else {
            Ok(None)
        }
    }

    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let script_bytes = encode_script(script);
        let result = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_scripts
                   WHERE wallet_id = $1 AND script = $2
                   RETURNING keychain, child_index"#,
            )
            .bind(wallet_id)
            .bind(script_bytes)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = result {
            let keychain = decode_keychain(row.get("keychain"))?;
            let child = row.get::<i32, _>("child_index") as u32;
            Ok(Some((keychain, child)))
        } else {
            Ok(None)
        }
    }

    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid = encode_txid(&outpoint.txid);
        let vout = outpoint.vout as i32;
        let result = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_utxos
                   WHERE wallet_id = $1 AND txid = $2 AND vout = $3
                   RETURNING script, value, keychain, is_spent"#,
            )
            .bind(wallet_id)
            .bind(txid)
            .bind(vout)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = result {
            let txout = decode_txout(row.get("script"), row.get("value"))?;
            let keychain = decode_keychain(row.get("keychain"))?;
            let is_spent: bool = row.get("is_spent");
            Ok(Some(LocalUtxo {
                outpoint: *outpoint,
                txout,
                keychain,
                is_spent,
            }))
        } else {
            Ok(None)
        }
    }

    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid_bytes = encode_txid(txid);
        let result = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_raw_txs
                   WHERE wallet_id = $1 AND txid = $2
                   RETURNING transaction"#,
            )
            .bind(wallet_id)
            .bind(txid_bytes)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = result {
            let tx = decode_transaction(&row.get::<Vec<u8>, _>("transaction"))?;
            Ok(Some(tx))
        } else {
            Ok(None)
        }
    }

    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid_bytes = encode_txid(txid);
        let rows = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_txs
                   WHERE wallet_id = $1 AND txid = $2
                   RETURNING received, sent, fee, confirmation_height, confirmation_timestamp"#,
            )
            .bind(wallet_id)
            .bind(txid_bytes)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = rows {
            let received: i64 = row.get("received");
            let sent: i64 = row.get("sent");
            let fee: Option<i64> = row.get("fee");
            let height: Option<i32> = row.get("confirmation_height");
            let timestamp: Option<i64> = row.get("confirmation_timestamp");
            let confirmation_time = match (height, timestamp) {
                (Some(h), Some(ts)) => Some(BlockTime {
                    height: h as u32,
                    timestamp: ts as u64,
                }),
                _ => None,
            };
            let transaction = if include_raw {
                self.get_raw_tx(txid)?
            } else {
                None
            };
            Ok(Some(TransactionDetails {
                transaction,
                txid: *txid,
                received: received as u64,
                sent: sent as u64,
                fee: fee.map(|v| v as u64),
                confirmation_time,
            }))
        } else {
            Ok(None)
        }
    }

    fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_code = encode_keychain(keychain);
        let result = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_last_indices
                   WHERE wallet_id = $1 AND keychain = $2
                   RETURNING value"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = result {
            Ok(Some(row.get::<i32, _>("value") as u32))
        } else {
            Ok(None)
        }
    }

    fn del_sync_time(&mut self) -> Result<Option<SyncTime>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let result = self.run(async move {
            sqlx::query(
                r#"DELETE FROM onchain_wallet_sync_times
                   WHERE wallet_id = $1
                   RETURNING block_height, block_timestamp"#,
            )
            .bind(wallet_id)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = result {
            let height: i32 = row.get("block_height");
            let timestamp: i64 = row.get("block_timestamp");
            Ok(Some(decode_block_time(height, timestamp)))
        } else {
            Ok(None)
        }
    }
}

impl Database for PgWalletDatabase {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        keychain: KeychainKind,
        bytes: B,
    ) -> Result<(), BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let checksum = bytes.as_ref().to_vec();
        let keychain_code = encode_keychain(keychain);
        let existing = self.run(async move {
            sqlx::query(
                r#"SELECT checksum FROM onchain_wallet_descriptor_checksums
                   WHERE wallet_id = $1 AND keychain = $2"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = existing {
            let stored: Vec<u8> = row.get("checksum");
            if stored == checksum {
                Ok(())
            } else {
                Err(BdkError::ChecksumMismatch)
            }
        } else {
            let pool = self.pool.clone();
            self.run(async move {
                sqlx::query(
                    r#"INSERT INTO onchain_wallet_descriptor_checksums (wallet_id, keychain, checksum)
                       VALUES ($1, $2, $3)"#,
                )
                .bind(wallet_id)
                .bind(keychain_code)
                .bind(checksum)
                .execute(&pool)
                .await?;
                Ok(())
            })
        }
    }

    fn iter_script_pubkeys(
        &self,
        keychain: Option<KeychainKind>,
    ) -> Result<Vec<ScriptBuf>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_filter = keychain.map(encode_keychain);
        let rows = self.run(async move {
            if let Some(code) = keychain_filter {
                sqlx::query(
                    r#"SELECT script FROM onchain_wallet_scripts
                       WHERE wallet_id = $1 AND keychain = $2"#,
                )
                .bind(wallet_id)
                .bind(code)
                .fetch_all(&pool)
                .await
            } else {
                sqlx::query(
                    r#"SELECT script FROM onchain_wallet_scripts
                       WHERE wallet_id = $1"#,
                )
                .bind(wallet_id)
                .fetch_all(&pool)
                .await
            }
        })?;
        rows.into_iter()
            .map(|row| decode_script(&row.get::<Vec<u8>, _>("script")))
            .collect()
    }

    fn iter_utxos(&self) -> Result<Vec<LocalUtxo>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let rows = self.run(async move {
            sqlx::query(
                r#"SELECT txid, vout, script, value, keychain, is_spent
                   FROM onchain_wallet_utxos WHERE wallet_id = $1"#,
            )
            .bind(wallet_id)
            .fetch_all(&pool)
            .await
        })?;
        rows.into_iter()
            .map(|row| {
                let txid = decode_txid(&row.get::<Vec<u8>, _>("txid"))?;
                let vout = row.get::<i32, _>("vout") as u32;
                let txout = decode_txout(row.get("script"), row.get("value"))?;
                let keychain = decode_keychain(row.get("keychain"))?;
                let is_spent: bool = row.get("is_spent");
                Ok(LocalUtxo {
                    outpoint: OutPoint { txid, vout },
                    txout,
                    keychain,
                    is_spent,
                })
            })
            .collect()
    }

    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let rows = self.run(async move {
            sqlx::query(r#"SELECT transaction FROM onchain_wallet_raw_txs WHERE wallet_id = $1"#)
                .bind(wallet_id)
                .fetch_all(&pool)
                .await
        })?;
        rows.into_iter()
            .map(|row| decode_transaction(&row.get::<Vec<u8>, _>("transaction")))
            .collect()
    }

    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let rows = self.run(async move {
            if include_raw {
                sqlx::query(
                    r#"SELECT t.txid, t.received, t.sent, t.fee, t.confirmation_height,
                              t.confirmation_timestamp, r.transaction AS raw_tx
                       FROM onchain_wallet_txs t
                       LEFT JOIN onchain_wallet_raw_txs r
                         ON r.wallet_id = t.wallet_id AND r.txid = t.txid
                       WHERE t.wallet_id = $1"#,
                )
                .bind(wallet_id)
                .fetch_all(&pool)
                .await
            } else {
                sqlx::query(
                    r#"SELECT txid, received, sent, fee, confirmation_height, confirmation_timestamp
                       FROM onchain_wallet_txs WHERE wallet_id = $1"#,
                )
                .bind(wallet_id)
                .fetch_all(&pool)
                .await
            }
        })?;

        rows.into_iter()
            .map(|row| {
                let txid = decode_txid(&row.get::<Vec<u8>, _>("txid"))?;
                let received: i64 = row.get("received");
                let sent: i64 = row.get("sent");
                let fee: Option<i64> = row.get("fee");
                let height: Option<i32> = row.try_get("confirmation_height").ok();
                let ts: Option<i64> = row.try_get("confirmation_timestamp").ok();
                let confirmation_time = match (height, ts) {
                    (Some(h), Some(t)) => Some(BlockTime {
                        height: h as u32,
                        timestamp: t as u64,
                    }),
                    _ => None,
                };
                let transaction = if include_raw {
                    row.try_get::<Vec<u8>, _>("raw_tx")
                        .ok()
                        .map(|bytes| decode_transaction(&bytes))
                        .transpose()?
                } else {
                    None
                };
                Ok(TransactionDetails {
                    transaction,
                    txid,
                    received: received as u64,
                    sent: sent as u64,
                    fee: fee.map(|v| v as u64),
                    confirmation_time,
                })
            })
            .collect()
    }

    fn get_script_pubkey_from_path(
        &self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<ScriptBuf>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_code = encode_keychain(keychain);
        let child_index = child as i32;
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT script FROM onchain_wallet_scripts
                   WHERE wallet_id = $1 AND keychain = $2 AND child_index = $3"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .bind(child_index)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(row) = row {
            Ok(Some(decode_script(&row.get::<Vec<u8>, _>("script"))?))
        } else {
            Ok(None)
        }
    }

    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let script_bytes = encode_script(script);
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT keychain, child_index FROM onchain_wallet_scripts
                   WHERE wallet_id = $1 AND script = $2"#,
            )
            .bind(wallet_id)
            .bind(script_bytes)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(r) = row {
            let keychain = decode_keychain(r.get("keychain"))?;
            let child = r.get::<i32, _>("child_index") as u32;
            Ok(Some((keychain, child)))
        } else {
            Ok(None)
        }
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid = encode_txid(&outpoint.txid);
        let vout = outpoint.vout as i32;
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT script, value, keychain, is_spent
                   FROM onchain_wallet_utxos
                   WHERE wallet_id = $1 AND txid = $2 AND vout = $3"#,
            )
            .bind(wallet_id)
            .bind(txid)
            .bind(vout)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(r) = row {
            let txout = decode_txout(r.get("script"), r.get("value"))?;
            let keychain = decode_keychain(r.get("keychain"))?;
            let is_spent: bool = r.get("is_spent");
            Ok(Some(LocalUtxo {
                outpoint: *outpoint,
                txout,
                keychain,
                is_spent,
            }))
        } else {
            Ok(None)
        }
    }

    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid_bytes = encode_txid(txid);
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT transaction FROM onchain_wallet_raw_txs
                   WHERE wallet_id = $1 AND txid = $2"#,
            )
            .bind(wallet_id)
            .bind(txid_bytes)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(r) = row {
            Ok(Some(decode_transaction(
                &r.get::<Vec<u8>, _>("transaction"),
            )?))
        } else {
            Ok(None)
        }
    }

    fn get_tx(
        &self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let txid_bytes = encode_txid(txid);
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT received, sent, fee, confirmation_height, confirmation_timestamp
                   FROM onchain_wallet_txs
                   WHERE wallet_id = $1 AND txid = $2"#,
            )
            .bind(wallet_id)
            .bind(txid_bytes)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(r) = row {
            let received: i64 = r.get("received");
            let sent: i64 = r.get("sent");
            let fee: Option<i64> = r.get("fee");
            let height: Option<i32> = r.get("confirmation_height");
            let timestamp: Option<i64> = r.get("confirmation_timestamp");
            let confirmation_time = match (height, timestamp) {
                (Some(h), Some(ts)) => Some(BlockTime {
                    height: h as u32,
                    timestamp: ts as u64,
                }),
                _ => None,
            };
            let transaction = if include_raw {
                self.get_raw_tx(txid)?
            } else {
                None
            };
            Ok(Some(TransactionDetails {
                transaction,
                txid: *txid,
                received: received as u64,
                sent: sent as u64,
                fee: fee.map(|v| v as u64),
                confirmation_time,
            }))
        } else {
            Ok(None)
        }
    }

    fn get_last_index(&self, keychain: KeychainKind) -> Result<Option<u32>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_code = encode_keychain(keychain);
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT value FROM onchain_wallet_last_indices
                   WHERE wallet_id = $1 AND keychain = $2"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(r) = row {
            Ok(Some(r.get::<i32, _>("value") as u32))
        } else {
            Ok(None)
        }
    }

    fn get_sync_time(&self) -> Result<Option<SyncTime>, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let row = self.run(async move {
            sqlx::query(
                r#"SELECT block_height, block_timestamp FROM onchain_wallet_sync_times
                   WHERE wallet_id = $1"#,
            )
            .bind(wallet_id)
            .fetch_optional(&pool)
            .await
        })?;
        if let Some(r) = row {
            let height: i32 = r.get("block_height");
            let timestamp: i64 = r.get("block_timestamp");
            Ok(Some(decode_block_time(height, timestamp)))
        } else {
            Ok(None)
        }
    }

    fn increment_last_index(&mut self, keychain: KeychainKind) -> Result<u32, BdkError> {
        let pool = self.pool.clone();
        let wallet_id = self.wallet_id;
        let keychain_code = encode_keychain(keychain);
        let value: i32 = self.run(async move {
            sqlx::query_scalar(
                r#"WITH upsert AS (
                        INSERT INTO onchain_wallet_last_indices (wallet_id, keychain, value)
                        VALUES ($1, $2, 0)
                        ON CONFLICT (wallet_id, keychain)
                        DO UPDATE SET value = onchain_wallet_last_indices.value + 1,
                                      updated_at = NOW()
                        RETURNING value
                    )
                    SELECT value FROM upsert"#,
            )
            .bind(wallet_id)
            .bind(keychain_code)
            .fetch_one(&pool)
            .await
        })?;
        Ok(value as u32)
    }
}

impl BatchDatabase for PgWalletDatabase {
    type Batch = PgWalletBatch;

    fn begin_batch(&self) -> Self::Batch {
        PgWalletBatch::default()
    }

    fn commit_batch(&mut self, mut batch: Self::Batch) -> Result<(), BdkError> {
        for op in batch.ops.drain(..) {
            match op {
                BatchOp::SetScript {
                    script,
                    keychain,
                    child,
                } => {
                    self.set_script_pubkey(&script, keychain, child)?;
                }
                BatchOp::SetUtxo { utxo } => {
                    self.set_utxo(&utxo)?;
                }
                BatchOp::SetRawTx { tx } => {
                    self.set_raw_tx(&tx)?;
                }
                BatchOp::SetTx { details } => {
                    self.set_tx(&details)?;
                }
                BatchOp::SetLastIndex { keychain, value } => {
                    self.set_last_index(keychain, value)?;
                }
                BatchOp::SetSyncTime { sync } => {
                    self.set_sync_time(sync)?;
                }
                BatchOp::DelScriptByPath { keychain, child } => {
                    let _ = self.del_script_pubkey_from_path(keychain, child)?;
                }
                BatchOp::DelScriptByScript { script } => {
                    let _ = self.del_path_from_script_pubkey(&script)?;
                }
                BatchOp::DelUtxo { outpoint } => {
                    let _ = self.del_utxo(&outpoint)?;
                }
                BatchOp::DelRawTx { txid } => {
                    let _ = self.del_raw_tx(&txid)?;
                }
                BatchOp::DelTx { txid, include_raw } => {
                    let _ = self.del_tx(&txid, include_raw)?;
                }
                BatchOp::DelLastIndex { keychain } => {
                    let _ = self.del_last_index(keychain)?;
                }
                BatchOp::DelSyncTime => {
                    let _ = self.del_sync_time()?;
                }
            }
        }
        Ok(())
    }
}
