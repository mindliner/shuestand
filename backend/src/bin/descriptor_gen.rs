use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use bdk::bitcoin::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use bdk::bitcoin::secp256k1::Secp256k1;
use bdk::bitcoin::Network;
use bdk::keys::bip39::{Language, Mnemonic};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "shuestand-descriptor-gen",
    about = "Generate Bitcoin descriptors from a BIP39 seed (without passing seed as CLI arg)"
)]
struct Cli {
    /// bitcoin | testnet | signet | regtest
    #[arg(long, default_value = "bitcoin")]
    network: String,

    /// Optional file containing BIP39 seed words (single line)
    #[arg(long)]
    seed_file: Option<PathBuf>,

    /// Optional file containing BIP39 passphrase
    #[arg(long)]
    passphrase_file: Option<PathBuf>,

    /// Optional output file for env snippet
    #[arg(long)]
    output_env: Option<PathBuf>,

    /// Emit a commented .env template instead of raw KEY=VALUE lines
    #[arg(long, default_value_t = false)]
    template: bool,
}

struct SeedDerivedDescriptors {
    public_descriptor: String,
    spend_descriptor: String,
    change_descriptor: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let network = parse_network(&cli.network)?;

    let seed_phrase = read_seed_phrase(cli.seed_file.as_ref())?;
    let passphrase = read_passphrase(cli.passphrase_file.as_ref())?;

    let descriptors = derive_descriptors_from_seed(&seed_phrase, passphrase.as_deref(), network)?;

    let env_snippet = if cli.template {
        render_env_template(&descriptors, network)
    } else {
        format!(
            "BITCOIN_DESCRIPTOR={}\nBITCOIN_SPEND_DESCRIPTOR={}\nBITCOIN_CHANGE_DESCRIPTOR={}\n",
            env_single_quote(&descriptors.public_descriptor),
            env_single_quote(&descriptors.spend_descriptor),
            env_single_quote(&descriptors.change_descriptor)
        )
    };

    if let Some(path) = cli.output_env {
        fs::write(&path, &env_snippet)
            .with_context(|| format!("failed writing {}", path.display()))?;
        println!("Wrote descriptor env snippet to {}", path.display());
    } else {
        print!("{env_snippet}");
        io::stdout().flush().ok();
    }

    Ok(())
}

fn parse_network(raw: &str) -> Result<Network> {
    match raw.to_lowercase().as_str() {
        "bitcoin" | "mainnet" => Ok(Network::Bitcoin),
        "testnet" => Ok(Network::Testnet),
        "signet" => Ok(Network::Signet),
        "regtest" => Ok(Network::Regtest),
        _ => Err(anyhow!("unsupported network: {raw}")),
    }
}

fn read_seed_phrase(seed_file: Option<&PathBuf>) -> Result<String> {
    if let Some(path) = seed_file {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed reading seed file {}", path.display()))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("seed file is empty"));
        }
        return Ok(normalize_seed_phrase(trimmed));
    }

    eprint!("Enter BIP39 seed words: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed reading seed phrase from stdin")?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("seed phrase is empty"));
    }
    Ok(normalize_seed_phrase(trimmed))
}

fn normalize_seed_phrase(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn read_passphrase(passphrase_file: Option<&PathBuf>) -> Result<Option<String>> {
    if let Some(path) = passphrase_file {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed reading passphrase file {}", path.display()))?;
        return Ok(Some(raw.trim_end_matches(['\n', '\r']).to_string()));
    }

    Ok(None)
}

fn env_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn render_env_template(descriptors: &SeedDerivedDescriptors, network: Network) -> String {
    let generated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    format!(
        "# ------------------------------------------------------------------------------\n# Shuestand on-chain descriptor template\n# Network: {:?}\n# Generated (unix epoch): {}\n#\n# SECURITY:\n# - Keep BITCOIN_SPEND_DESCRIPTOR and BITCOIN_CHANGE_DESCRIPTOR secret.\n# - Prefer storing this file with strict permissions (chmod 600).\n# - For checksum validation run:\n#   bitcoin-cli getdescriptorinfo \"<descriptor>\"\n# ------------------------------------------------------------------------------\nBITCOIN_DESCRIPTOR={}\nBITCOIN_SPEND_DESCRIPTOR={}\nBITCOIN_CHANGE_DESCRIPTOR={}\n",
        network,
        generated_at,
        env_single_quote(&descriptors.public_descriptor),
        env_single_quote(&descriptors.spend_descriptor),
        env_single_quote(&descriptors.change_descriptor)
    )
}

fn derive_descriptors_from_seed(
    seed_phrase: &str,
    passphrase: Option<&str>,
    network: Network,
) -> Result<SeedDerivedDescriptors, anyhow::Error> {
    maybe_return_checksum_candidates(seed_phrase, Language::English)?;

    let mnemonic =
        Mnemonic::parse_in(Language::English, seed_phrase).context("invalid BIP39 mnemonic")?;
    let seed = mnemonic.to_seed(passphrase.unwrap_or(""));
    let secp = Secp256k1::new();
    let master = ExtendedPrivKey::new_master(network, &seed)
        .context("failed to derive master xprv from seed")?;
    let coin_type = match network {
        Network::Bitcoin => 0,
        _ => 1,
    };
    let account_path = DerivationPath::from_str(&format!("m/84'/{}'/0'", coin_type))
        .map_err(|_| anyhow!("invalid derivation path"))?;
    let account_xprv = master
        .derive_priv(&secp, &account_path)
        .context("failed to derive account xprv")?;
    let account_xpub = ExtendedPubKey::from_priv(&secp, &account_xprv);
    let fingerprint = master.fingerprint(&secp);
    let origin = format!("[{}/84h/{}h/0h]", fingerprint, coin_type);

    Ok(SeedDerivedDescriptors {
        public_descriptor: format!("wpkh({}{}/0/*)", origin, account_xpub),
        spend_descriptor: format!("wpkh({}{}/0/*)", origin, account_xprv),
        change_descriptor: format!("wpkh({}{}/1/*)", origin, account_xprv),
    })
}

fn maybe_return_checksum_candidates(seed_phrase: &str, language: Language) -> Result<()> {
    let words = seed_phrase
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>();

    let word_count = words.len();
    if word_count != 11 && word_count != 23 {
        return Ok(());
    }

    for (index, word) in words.iter().enumerate() {
        if language.find_word(word).is_none() {
            return Err(anyhow!(
                "invalid BIP39 word at position {}: '{}'",
                index + 1,
                word
            ));
        }
    }

    let prefix = words.join(" ");
    let mut candidates = Vec::new();
    for candidate in language.word_list() {
        let phrase = format!("{prefix} {candidate}");
        if Mnemonic::parse_in(language, &phrase).is_ok() {
            candidates.push(*candidate);
        }
    }

    if candidates.is_empty() {
        return Err(anyhow!(
            "incomplete BIP39 mnemonic ({} words), but no checksum-valid final words were found",
            word_count
        ));
    }

    Err(anyhow!(
        "incomplete BIP39 mnemonic: got {} words.\nPossible checksum-valid final words ({}):\n{}\n\nPick one final word, append it to your phrase, and run this command again.",
        word_count,
        candidates.len(),
        format_word_candidates(&candidates)
    ))
}

fn format_word_candidates(words: &[&str]) -> String {
    const PER_LINE: usize = 8;
    words
        .chunks(PER_LINE)
        .map(|chunk| chunk.join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eleven_word_phrase_lists_128_candidates() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        let err = maybe_return_checksum_candidates(phrase, Language::English)
            .expect_err("11 words should return candidates");
        let msg = err.to_string();
        assert!(msg.contains("got 11 words"));
        assert!(msg.contains("(128)"));
        assert!(msg.contains("about"));
    }

    #[test]
    fn twenty_three_word_phrase_lists_8_candidates() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        let err = maybe_return_checksum_candidates(phrase, Language::English)
            .expect_err("23 words should return candidates");
        let msg = err.to_string();
        assert!(msg.contains("got 23 words"));
        assert!(msg.contains("(8)"));
    }

    #[test]
    fn complete_phrase_passes_candidate_gate() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        maybe_return_checksum_candidates(phrase, Language::English)
            .expect("12 words should skip candidate mode");
    }
}
