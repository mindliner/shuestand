//! Offline-owner maintenance: explicit M5 proof sets only, never replay sagas.
use anyhow::{Context, Result, bail, ensure};
use backend::wallet::{WalletConfig, open_existing_wallet_no_recovery};
use cdk::nuts::nut00::KnownMethod;
use cdk::{
    Amount,
    amount::SplitTarget,
    nuts::{
        Id, MeltQuoteState, MintQuoteState, PaymentMethod, Proof, Proofs, ProofsMethods, PublicKey,
    },
};
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

const M5: &str = "https://mint.mountainlake.io";
const M7: &str = "https://m7.mountainlake.io";
const FOREIGN: &str = "foreign/https___mint_mountainlake_io";

#[derive(Parser)]
#[command(
    about = "Targeted M5 retirement. Stop backend before execute; never runs bulk saga recovery."
)]
struct Cli {
    #[arg(long)]
    wallet_root: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// SQLite read-only + mint state/restore checks; does not open a CDK wallet.
    Plan {
        #[arg(long)]
        output: PathBuf,
    },
    /// Explicit proofs, bounded loss, journal and backups. No customer payout.
    Execute {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        recovery_dir: PathBuf,
        #[arg(long)]
        max_fee_sats: u64,
        /// Attest ALL backend/CLI processes using these stores are stopped.
        #[arg(long)]
        backend_stopped: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    y: String,
    amount: u64,
    local_state: String,
    mint_state: String,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Plan {
    version: u32,
    source: String,
    destination: String,
    fingerprint: String,
    proofs: Vec<Entry>,
    live_sats: u64,
    stale_sats: u64,
}
fn ro(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(Duration::from_secs(5))?;
    Ok(c)
}
fn private_write(path: &Path, data: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}
fn event(dir: &Path, value: Value) -> Result<()> {
    let mut f = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(dir.join("journal.jsonl"))?;
    serde_json::to_writer(&mut f, &value)?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}
fn fingerprint(c: &Connection) -> Result<String> {
    let mut h = Sha256::new();
    for (table, order) in [
        ("proof", "hex(y)"),
        ("wallet_sagas", "id"),
        ("melt_quote", "id"),
    ] {
        h.update(table.as_bytes());
        let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))?;
        let n = s.column_count();
        let mut rows = s.query([])?;
        while let Some(row) = rows.next()? {
            for i in 0..n {
                // Length-delimited, typed values; only the digest leaves the process.
                let bytes = format!("{:?}", row.get_ref(i)?).into_bytes();
                h.update((bytes.len() as u64).to_le_bytes());
                h.update(bytes);
            }
        }
    }
    Ok(hex::encode(h.finalize()))
}
fn inventory(c: &Connection) -> Result<Vec<Entry>> {
    let mut s = c.prepare(
        "SELECT hex(y),amount,state,mint_url,unit,spending_condition FROM proof ORDER BY hex(y)",
    )?;
    let mut rows = s.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let amount: i64 = r.get(1)?;
        ensure!(amount > 0, "invalid proof amount");
        ensure!(
            r.get::<_, String>(3)? == M5 && r.get::<_, String>(4)? == "sat",
            "unexpected mint/unit"
        );
        ensure!(
            r.get::<_, Option<String>>(5)?.is_none(),
            "conditioned proofs require separate recovery"
        );
        out.push(Entry {
            y: r.get::<_, String>(0)?.to_lowercase(),
            amount: amount as u64,
            local_state: r.get(2)?,
            mint_state: String::new(),
        });
    }
    ensure!(!out.is_empty(), "no M5 proof inventory");
    Ok(out)
}
async fn states(client: &reqwest::Client, ys: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for batch in ys.chunks(100) {
        let v: Value = client
            .post(format!("{M5}/v1/checkstate"))
            .json(&json!({"Ys":batch}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let states = v["states"].as_array().context("missing mint states")?;
        ensure!(states.len() == batch.len(), "incomplete mint reply");
        for s in states {
            let y = s["Y"].as_str().context("missing Y")?.to_lowercase();
            ensure!(
                batch.contains(&y) && !out.contains_key(&y),
                "unexpected/duplicate mint Y"
            );
            let state = s["state"].as_str().context("missing state")?;
            ensure!(
                ["SPENT", "UNSPENT"].contains(&state),
                "pending/unknown proof state; stop"
            );
            out.insert(y, state.to_owned());
        }
    }
    ensure!(out.len() == ys.len(), "mint state coverage mismatch");
    Ok(out)
}
async fn check_old_operations(c: &Connection, client: &reqwest::Client) -> Result<()> {
    let mut s = c.prepare("SELECT kind,state,data FROM wallet_sagas ORDER BY id")?;
    let rows = s
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (kind, state, data) in rows {
        let state: Value = serde_json::from_str(&state)?;
        let data: Value = serde_json::from_str(&data)?;
        let data = &data["data"];
        match kind.as_str() {
            "melt" => {
                ensure!(
                    state["state"] == "proofs_reserved",
                    "unexpected old melt phase"
                );
                let id = data["quote_id"]
                    .as_str()
                    .context("old melt missing quote")?;
                let v: Value = client
                    .get(format!("{M5}/v1/melt/quote/bolt11/{id}"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                ensure!(
                    v["state"] == "UNPAID",
                    "old melt is not UNPAID; reconcile first"
                );
                ensure!(
                    data["change_amount"].is_null(),
                    "old melt has change evidence"
                );
            }
            "swap" => {
                ensure!(
                    state["state"] == "swap_requested",
                    "unexpected old swap phase"
                );
                let outputs = data["blinded_messages"]
                    .as_array()
                    .context("old swap missing outputs")?;
                ensure!(!outputs.is_empty(), "cannot establish old swap outcome");
                let v: Value = client
                    .post(format!("{M5}/v1/restore"))
                    .json(&json!({"outputs":outputs}))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                ensure!(
                    v["outputs"]
                        .as_array()
                        .context("missing restore outputs")?
                        .is_empty(),
                    "old swap issued outputs; stop"
                );
                let sig = v
                    .get("signatures")
                    .or_else(|| v.get("promises"))
                    .context("missing restore signatures")?;
                ensure!(
                    sig.as_array().context("bad restore response")?.is_empty(),
                    "old swap signatures exist; stop"
                );
            }
            _ => bail!("unsupported old saga kind; separate investigation required"),
        }
    }
    Ok(())
}
fn classify(entries: &[Entry]) -> Result<(u64, u64)> {
    let mut live = 0u64;
    let mut stale = 0u64;
    for p in entries {
        match (p.local_state.as_str(), p.mint_state.as_str()) {
            ("UNSPENT" | "RESERVED", "UNSPENT") => {
                live = live.checked_add(p.amount).context("amount overflow")?
            }
            ("UNSPENT" | "RESERVED", "SPENT") => {
                stale = stale.checked_add(p.amount).context("amount overflow")?
            }
            ("SPENT", "SPENT") => {}
            _ => bail!("ambiguous local/mint state; stop"),
        }
    }
    Ok((live, stale))
}
async fn make_plan(root: &Path, client: &reqwest::Client) -> Result<Plan> {
    let c = ro(&root.join(FOREIGN).join("wallet.sqlite"))?;
    c.execute_batch("BEGIN")?; // one read snapshot, including WAL
    let fingerprint = fingerprint(&c)?;
    let mut entries = inventory(&c)?;
    let mint = states(
        client,
        &entries.iter().map(|p| p.y.clone()).collect::<Vec<_>>(),
    )
    .await?;
    for e in &mut entries {
        e.mint_state = mint[&e.y].clone();
    }
    let (live_sats, stale_sats) = classify(&entries)?;
    check_old_operations(&c, client).await?;
    c.execute_batch("ROLLBACK")?;
    Ok(Plan {
        version: 1,
        source: M5.into(),
        destination: M7.into(),
        fingerprint,
        proofs: entries,
        live_sats,
        stale_sats,
    })
}
fn load_live(root: &Path, plan: &Plan) -> Result<Proofs> {
    let c = ro(&root.join(FOREIGN).join("wallet.sqlite"))?;
    let wanted: BTreeSet<_> = plan
        .proofs
        .iter()
        .filter(|p| p.mint_state == "UNSPENT")
        .map(|p| p.y.clone())
        .collect();
    let mut s = c.prepare(
        "SELECT hex(y), amount,keyset_id,secret,hex(c),witness,p2pk_e FROM proof ORDER BY hex(y)",
    )?;
    let mut rows = s.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let y = r.get::<_, String>(0)?.to_lowercase();
        if !wanted.contains(&y) {
            continue;
        }
        ensure!(
            r.get::<_, Option<String>>(5)?.is_none() && r.get::<_, Option<Vec<u8>>>(6)?.is_none(),
            "unsupported witness/key"
        );
        let p = Proof::new(
            Amount::from(r.get::<_, i64>(1)? as u64),
            Id::from_str(&r.get::<_, String>(2)?)?,
            cdk::secret::Secret::new(r.get::<_, String>(3)?),
            PublicKey::from_str(&r.get::<_, String>(4)?)?,
        );
        ensure!(p.y()?.to_hex() == y, "stored proof Y mismatch");
        out.push(p);
    }
    ensure!(
        out.len() == wanted.len() && out.total_amount()?.to_u64() == plan.live_sats,
        "proof inventory changed"
    );
    Ok(out)
}
fn backup(root: &Path, dir: &Path) -> Result<()> {
    for (name, base) in [("m5", root.join(FOREIGN)), ("m7", root.to_path_buf())] {
        let database_path = dir.join(format!("{name}-wallet.sqlite"));
        private_write(&database_path, b"")?;
        let mut dst = Connection::open(&database_path)?;
        let src = ro(&base.join("wallet.sqlite"))?;
        let b = rusqlite::backup::Backup::new(&src, &mut dst)?;
        b.run_to_completion(128, Duration::from_millis(10), None)?;
        drop(b);
        drop(dst);
        fs::set_permissions(
            dir.join(format!("{name}-wallet.sqlite")),
            fs::Permissions::from_mode(0o600),
        )?;
        let seed = fs::read(base.join("seed"))?;
        ensure!(seed.len() == 64, "existing seed invalid");
        private_write(&dir.join(format!("{name}-seed")), &seed)?;
    }
    Ok(())
}
fn payment_target(total: u64, input_fee: u64, reserve: u64) -> Option<u64> {
    total
        .checked_sub(input_fee)?
        .checked_sub(reserve)
        .filter(|a| *a > 0)
}
fn enforce_budget(initial: u64, credited: u64, remaining: u64, max_fee: u64) -> Result<()> {
    let accounted = credited.checked_add(remaining).context("amount overflow")?;
    ensure!(
        accounted <= initial && initial - accounted <= max_fee,
        "fee/loss budget exceeded"
    );
    Ok(())
}

async fn execute(
    root: &Path,
    plan: &Plan,
    dir: &Path,
    max_fee: u64,
    client: &reqwest::Client,
) -> Result<()> {
    ensure!(
        max_fee > 0 && max_fee <= plan.live_sats / 20,
        "fee ceiling must be positive and at most 5% of live value"
    );
    ensure!(
        &make_plan(root, client).await? == plan,
        "plan is stale or altered; create a new plan"
    );
    let mut inputs = load_live(root, plan)?;
    // Exclusive create is also the rerun guard. An interrupted execution needs investigation,
    // never a new payment using the old plan or a reused journal directory.
    fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .context("new recovery directory required")?;
    private_write(&dir.join("plan.json"), &serde_json::to_vec_pretty(plan)?)?;
    backup(root, dir)?;
    ensure!(
        &make_plan(root, client).await? == plan,
        "wallet changed during backup; stop"
    );
    event(
        dir,
        json!({"phase":"backed_up","initial_sats":plan.live_sats,"max_fee_sats":max_fee}),
    )?;
    let old_ids: Vec<String> = {
        let c = ro(&root.join(FOREIGN).join("wallet.sqlite"))?;
        let mut s = c.prepare("SELECT id FROM wallet_sagas ORDER BY id")?;
        s.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let source =
        open_existing_wallet_no_recovery(&WalletConfig::new(M5, Some(root.join(FOREIGN)))).await?;
    let destination =
        open_existing_wallet_no_recovery(&WalletConfig::new(M7, Some(root.to_path_buf()))).await?;
    let mut credited = 0u64;
    for round in 0..8 {
        if inputs.is_empty() {
            break;
        }
        let total = inputs.total_amount()?.to_u64();
        let ys = inputs.ys()?.iter().map(|y| y.to_hex()).collect::<Vec<_>>();
        ensure!(
            states(client, &ys).await?.values().all(|s| s == "UNSPENT"),
            "recovery input no longer unspent"
        );
        let input_fee = source
            .lock()
            .await
            .get_proofs_fee(&inputs)
            .await?
            .total
            .to_u64();
        let Some(mut target) = payment_target(total, input_fee, 0) else {
            break;
        };
        let mut chosen = None;
        for _ in 0..16 {
            let quote = destination
                .lock()
                .await
                .mint_quote(
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    Some(Amount::from(target)),
                    None,
                    None,
                )
                .await?;
            event(
                dir,
                json!({"phase":"canonical_quote","round":round,"id":quote.id,"amount":target}),
            )?;
            let melt = source
                .lock()
                .await
                .melt_quote(
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    &quote.request,
                    None,
                    None,
                )
                .await?;
            event(
                dir,
                json!({"phase":"foreign_quote","round":round,"canonical_quote":quote.id,"melt_quote":melt.id,"amount":target,"reserve":melt.fee_reserve.to_u64(),"input_fee":input_fee}),
            )?;
            let Some(next) = payment_target(total, input_fee, melt.fee_reserve.to_u64()) else {
                break;
            };
            if next < target {
                target = next;
                continue;
            }
            // Bound the worst-case loss before sending. Returned change is not assumed.
            enforce_budget(
                plan.live_sats,
                credited.checked_add(target).context("overflow")?,
                0,
                max_fee,
            )?;
            chosen = Some((quote, melt));
            break;
        }
        let Some((quote, melt)) = chosen else {
            break;
        };
        event(
            dir,
            json!({"phase":"before_melt","round":round,"canonical_quote":quote.id,"melt_quote":melt.id,"input_sats":total,"target_sats":target,"ys":ys}),
        )?;
        let guard = source.lock().await;
        let prepared = guard
            .prepare_melt_proofs(&melt.id, inputs.clone(), HashMap::new())
            .await?;
        let finalized = tokio::time::timeout(Duration::from_secs(120), prepared.confirm())
            .await
            .context("melt timeout; inspect journal/quotes, DO NOT rerun")??;
        ensure!(
            finalized.state() == MeltQuoteState::Paid,
            "melt not PAID; reconcile, DO NOT rerun"
        );
        event(
            dir,
            json!({"phase":"melt_paid","round":round,"melt_quote":melt.id,"reported_fee":finalized.fee_paid().to_u64()}),
        )?;
        inputs = finalized.into_change().unwrap_or_default();
        drop(guard);
        let remaining = inputs.total_amount()?.to_u64();
        let mut paid = false;
        for _ in 0..30 {
            let status = destination
                .lock()
                .await
                .check_mint_quote_status(&quote.id)
                .await?;
            match status.state {
                MintQuoteState::Paid => {
                    paid = true;
                    break;
                }
                MintQuoteState::Issued => bail!(
                    "canonical quote already issued unexpectedly; inspect stored proofs/journal"
                ),
                _ => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
        ensure!(
            paid,
            "canonical quote not paid within 60s; reconcile, DO NOT rerun"
        );
        event(
            dir,
            json!({"phase":"before_canonical_mint","round":round,"quote":quote.id}),
        )?;
        let minted = destination
            .lock()
            .await
            .mint(&quote.id, SplitTarget::default(), None)
            .await?;
        ensure!(
            minted.total_amount()?.to_u64() == target,
            "canonical issued amount mismatch"
        );
        // Direct NUT-07 for destination, without mutating its unrelated pending proofs.
        let mint_ys = minted.ys()?.iter().map(|y| y.to_hex()).collect::<Vec<_>>();
        let v: Value = client
            .post(format!("{M7}/v1/checkstate"))
            .json(&json!({"Ys":mint_ys}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        verify_state_response(&mint_ys, &v, "UNSPENT")?;
        credited = credited.checked_add(target).context("credit overflow")?;
        enforce_budget(plan.live_sats, credited, remaining, max_fee)?;
        event(
            dir,
            json!({"phase":"round_complete","round":round,"credited_sats":credited,"remaining_m5_sats":remaining,"canonical_ys":mint_ys}),
        )?;
    }
    let remaining = inputs.total_amount()?.to_u64();
    // Never erase unspent value to report zero. All original and change proofs are
    // rechecked before touching the exact old reservations and saga references.
    let current = make_cleanup_states(root, plan, client).await?;
    {
        let audit = ro(&root.join(FOREIGN).join("wallet.sqlite"))?;
        check_old_operations(&audit, client).await?;
    }
    event(
        dir,
        json!({"phase":"before_old_operation_cleanup","ids":old_ids}),
    )?;
    let c = Connection::open(root.join(FOREIGN).join("wallet.sqlite"))?;
    c.execute_batch("BEGIN IMMEDIATE")?;
    for e in &plan.proofs {
        if e.local_state == "SPENT" {
            continue;
        }
        let state = current
            .get(&e.y)
            .context("original proof disappeared; reconcile")?;
        // Missing old rows can be legitimate CDK cleanup after a paid melt: UPDATE
        // affects zero rows, and mint state still proves whether value was consumed.
        c.execute(
            "UPDATE proof SET state=?1,used_by_operation=NULL WHERE y=?2",
            rusqlite::params![state, hex::decode(&e.y)?],
        )?;
    }
    for id in &old_ids {
        c.execute(
            "UPDATE melt_quote SET used_by_operation=NULL WHERE used_by_operation=?1",
            [id],
        )?;
        c.execute("DELETE FROM wallet_sagas WHERE id=?1", [id])?;
    }
    c.execute_batch("COMMIT")?;
    drop(c);
    event(
        dir,
        json!({"phase":"old_operations_archived","ids":old_ids}),
    )?;
    let verify = ro(&root.join(FOREIGN).join("wallet.sqlite"))?;
    let active: i64 = verify.query_row(
        "SELECT coalesce(sum(amount),0) FROM proof WHERE state!='SPENT'",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        active >= 0 && active as u64 == remaining,
        "final M5 value differs from tracked change; investigate"
    );
    enforce_budget(plan.live_sats, credited, remaining, max_fee)?;
    event(
        dir,
        json!({"phase":"complete","m7_credited_sats":credited,"m5_remaining_sats":remaining,"loss_sats":plan.live_sats-credited-remaining}),
    )?;
    println!(
        "M7 credited: {credited} sats; M5 remaining: {remaining} sats; fees/value difference: {} sats.",
        plan.live_sats - credited - remaining
    );
    if remaining > 0 {
        println!(
            "M5 remainder retained, NOT deleted. Do not claim zero balance or rerun the old plan."
        );
    }
    Ok(())
}
fn verify_state_response(ys: &[String], v: &Value, expected: &str) -> Result<()> {
    let states = v["states"].as_array().context("missing proof states")?;
    let mut seen = BTreeSet::new();
    ensure!(states.len() == ys.len(), "state coverage mismatch");
    for s in states {
        let y = s["Y"].as_str().context("missing Y")?.to_lowercase();
        ensure!(
            ys.contains(&y) && seen.insert(y) && s["state"] == expected,
            "unexpected proof state"
        );
    }
    Ok(())
}
async fn make_cleanup_states(
    root: &Path,
    plan: &Plan,
    client: &reqwest::Client,
) -> Result<BTreeMap<String, String>> {
    let c = ro(&root.join(FOREIGN).join("wallet.sqlite"))?;
    let entries = inventory(&c)?;
    let ys = entries
        .into_iter()
        .map(|e| e.y)
        .chain(plan.proofs.iter().map(|e| e.y.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    states(client, &ys).await
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = fs::canonicalize(&cli.wallet_root)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    match cli.command {
        Command::Plan { output } => {
            let p = make_plan(&root, &client).await?;
            println!(
                "M5 recoverable gross: {} sats; stale active face value: {} sats. Destination: M7. No wallet changes.",
                p.live_sats, p.stale_sats
            );
            private_write(&output, &serde_json::to_vec_pretty(&p)?)?;
        }
        Command::Execute {
            plan,
            recovery_dir,
            max_fee_sats,
            backend_stopped,
        } => {
            ensure!(
                backend_stopped,
                "stop all backend/wallet processes and pass --backend-stopped"
            );
            let p: Plan = serde_json::from_slice(&fs::read(plan)?)?;
            ensure!(
                p.version == 1 && p.source == M5 && p.destination == M7,
                "unsupported recovery plan"
            );
            ensure!(!recovery_dir.exists(), "new recovery directory required");
            let lock = root.join(".m5-recovery.lock");
            private_write(&lock, recovery_dir.to_string_lossy().as_bytes()).context(
                "recovery lock exists or cannot be created; reconcile before proceeding",
            )?;
            execute(&root, &p, &recovery_dir, max_fee_sats, &client)
                .await
                .context(
                    "recovery stopped; retain journal/backups; do NOT blindly retry or restore",
                )?;
            fs::remove_file(lock)?;
            fs::File::open(&root)?.sync_all()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(local: &str, mint: &str, n: u64) -> Entry {
        Entry {
            y: "synthetic".into(),
            amount: n,
            local_state: local.into(),
            mint_state: mint.into(),
        }
    }
    #[test]
    fn inventory_separates_dead_reservations_from_real_value() {
        assert_eq!(
            classify(&[
                entry("RESERVED", "SPENT", 26653),
                entry("RESERVED", "UNSPENT", 114981),
                entry("UNSPENT", "UNSPENT", 992),
                entry("SPENT", "SPENT", 137676)
            ])
            .unwrap(),
            (115973, 26653)
        );
        for e in [
            entry("PENDING_SPENT", "UNSPENT", 1),
            entry("SPENT", "UNSPENT", 1),
            entry("RESERVED", "PENDING", 1),
        ] {
            assert!(classify(&[e]).is_err());
        }
    }
    #[test]
    fn target_keeps_input_fee_and_reserve_and_never_underflows() {
        assert_eq!(payment_target(115973, 1, 3480), Some(112492));
        assert_eq!(payment_target(1, 1, 0), None);
        assert_eq!(payment_target(10, 2, 9), None);
        assert_eq!(payment_target(100, u64::MAX, 1), None);
    }
    #[test]
    fn loss_ceiling_counts_change_without_treating_it_as_a_fee() {
        assert!(enforce_budget(115973, 112492, 3480, 1).is_ok());
        assert!(enforce_budget(115973, 112492, 0, 1000).is_err());
        assert!(enforce_budget(115973, 115973, 1, 5000).is_err());
        assert!(enforce_budget(u64::MAX, u64::MAX, 1, 1).is_err());
    }
    #[test]
    fn mint_response_requires_exact_unspent_set() {
        let ys = vec!["aa".into(), "bb".into()];
        assert!(
            verify_state_response(
                &ys,
                &json!({"states":[{"Y":"aa","state":"UNSPENT"},{"Y":"bb","state":"UNSPENT"}]}),
                "UNSPENT"
            )
            .is_ok()
        );
        for v in [
            json!({"states":[]}),
            json!({"states":[{"Y":"aa","state":"UNSPENT"},{"Y":"aa","state":"UNSPENT"}]}),
            json!({"states":[{"Y":"aa","state":"UNSPENT"},{"Y":"cc","state":"UNSPENT"}]}),
            json!({"states":[{"Y":"aa","state":"UNSPENT"},{"Y":"bb","state":"SPENT"}]}),
            json!({"states":[{"Y":"aa","state":"UNSPENT"},{"Y":"bb","state":"PENDING"}]}),
        ] {
            assert!(verify_state_response(&ys, &v, "UNSPENT").is_err());
        }
    }
    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("m5-recovery-test-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        p
    }
    fn fixture(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE proof(y BLOB,state TEXT); INSERT INTO proof VALUES(x'01','RESERVED'); CREATE TABLE wallet_sagas(id TEXT,state TEXT); INSERT INTO wallet_sagas VALUES('old','swap_requested'); CREATE TABLE melt_quote(id TEXT,state TEXT); INSERT INTO melt_quote VALUES('quote','UNPAID');").unwrap();
    }
    #[test]
    fn recovery_lock_cannot_be_overwritten_and_journal_is_append_only() {
        let dir = tempdir();
        let lock = dir.join(".m5-recovery.lock");
        private_write(&lock, b"first-run").unwrap();
        assert!(private_write(&lock, b"another-run").is_err());
        assert_eq!(fs::read(&lock).unwrap(), b"first-run");
        event(&dir, json!({"phase":"before_melt"})).unwrap();
        event(&dir, json!({"phase":"melt_paid"})).unwrap();
        let journal = fs::read_to_string(dir.join("journal.jsonl")).unwrap();
        assert_eq!(journal.lines().count(), 2);
        assert!(
            journal
                .lines()
                .all(|line| serde_json::from_str::<Value>(line).is_ok())
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn snapshot_digest_refuses_proof_saga_or_quote_drift() {
        let dir = tempdir();
        let path = dir.join("wallet.sqlite");
        fixture(&path);
        let c = ro(&path).unwrap();
        let initial = fingerprint(&c).unwrap();
        assert_eq!(initial, fingerprint(&c).unwrap());
        assert!(c.execute("DELETE FROM proof", []).is_err());
        drop(c);
        let w = Connection::open(&path).unwrap();
        for sql in [
            "UPDATE proof SET state='SPENT'",
            "UPDATE wallet_sagas SET state='changed'",
            "UPDATE melt_quote SET state='PAID'",
        ] {
            let before = fingerprint(&w).unwrap();
            w.execute(sql, []).unwrap();
            assert_ne!(before, fingerprint(&w).unwrap());
        }
        drop(w);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn backup_is_complete_private_and_never_overwrites_seed() {
        let dir = tempdir();
        let root = dir.join("wallets");
        let m5 = root.join(FOREIGN);
        fs::create_dir_all(&m5).unwrap();
        for base in [&root, &m5] {
            fixture(&base.join("wallet.sqlite"));
            fs::write(base.join("seed"), [7u8; 64]).unwrap();
        }
        let archive = dir.join("archive");
        fs::create_dir(&archive).unwrap();
        backup(&root, &archive).unwrap();
        for name in ["m5", "m7"] {
            let c = ro(&archive.join(format!("{name}-wallet.sqlite"))).unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM wallet_sagas", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let seed = archive.join(format!("{name}-seed"));
            assert_eq!(fs::read(&seed).unwrap(), vec![7u8; 64]);
            assert_eq!(
                fs::metadata(seed).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(backup(&root, &archive).is_err());
        assert_eq!(fs::read(archive.join("m5-seed")).unwrap(), vec![7u8; 64]);
        fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn maintenance_open_refuses_missing_or_invalid_seed_without_generating_one() {
        let dir = tempdir();
        let config = WalletConfig::new(M5, Some(dir.clone()));
        assert!(open_existing_wallet_no_recovery(&config).await.is_err());
        fixture(&dir.join("wallet.sqlite"));
        assert!(open_existing_wallet_no_recovery(&config).await.is_err());
        assert!(!dir.join("seed").exists());
        fs::write(dir.join("seed"), [1u8; 3]).unwrap();
        assert!(open_existing_wallet_no_recovery(&config).await.is_err());
        assert_eq!(fs::read(dir.join("seed")).unwrap().len(), 3);
        fs::remove_dir_all(dir).unwrap();
    }
}
