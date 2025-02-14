//! Simple bundle: connect, build one transaction, submit.
//!
//! Run with:
//! ```bash
//! RPC_URL=https://api.mainnet-beta.solana.com \
//! PAYER_JSON=~/.config/solana/id.json \
//! cargo run --example simple_bundle
//! ```

use std::env;
use std::fs;

use anyhow::{Context, Result};
use jito_bundler::{JitoEngine, TipStrategy};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jito_bundler=info".into()),
        )
        .init();

    let rpc = env::var("RPC_URL").context("set RPC_URL")?;
    let payer_path = env::var("PAYER_JSON").context("set PAYER_JSON")?;
    let payer = load_keypair(&payer_path)?;

    // Build a trivial self-transfer so the example has a signed transaction
    // to submit. Swap this out for your real transaction.
    let rpc = RpcClient::new(rpc);
    let blockhash = rpc.get_latest_blockhash().await?;
    let ix = system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), 1);
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[&payer],
        blockhash,
    );

    let engine = JitoEngine::builder()
        .tip_payer(load_keypair(&payer_path)?)
        .build()
        .await?;

    println!("using region {:?}", engine.region());

    let outcome = engine
        .bundle()
        .add_tx(tx)
        .tip(TipStrategy::Fixed(10_000)) // 10k lamports = 0.00001 SOL
        .submit()
        .await?;

    println!(
        "landed {} in slot {} after {} attempt(s)",
        outcome.bundle_id, outcome.slot, outcome.attempts
    );
    Ok(())
}

fn load_keypair(path: &str) -> Result<Keypair> {
    let expanded = shellexpand::tilde(path).to_string();
    let bytes: Vec<u8> =
        serde_json::from_str(&fs::read_to_string(expanded).context("read keypair file")?)?;
    Ok(Keypair::from_bytes(&bytes).context("decode keypair")?)
}

// Tiny inline helper so the example doesn't pull an extra dependency.
mod shellexpand {
    pub fn tilde(s: &str) -> std::borrow::Cow<'_, str> {
        if let Some(rest) = s.strip_prefix('~') {
            if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
                return std::borrow::Cow::Owned(format!("{home}{rest}"));
            }
        }
        std::borrow::Cow::Borrowed(s)
    }
}
