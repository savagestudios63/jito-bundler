//! Demonstrates the streaming tip oracle.
//!
//! Connects to Jito's public tip stream, waits for a handful of samples to
//! arrive, then submits a bundle using the live p75.
//!
//! Requires the `tip-stream` feature (enabled by default).

use std::env;
use std::fs;
use std::time::Duration;

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

    let engine = JitoEngine::builder()
        .tip_payer(load_keypair(&payer_path)?)
        .build()
        .await?;

    println!("warming up tip oracle…");
    engine
        .tip_oracle()
        .warmup(5, Duration::from_secs(30))
        .await?;

    let p50 = engine.tip_oracle().p50().await?;
    let p75 = engine.tip_oracle().p75().await?;
    let p95 = engine.tip_oracle().p95().await?;
    println!("current distribution: p50={p50} p75={p75} p95={p95}");

    let rpc = RpcClient::new(rpc);
    let bh = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), 1)],
        Some(&payer.pubkey()),
        &[&payer],
        bh,
    );

    let outcome = engine
        .bundle()
        .add_tx(tx)
        .tip(TipStrategy::Percentile75)
        .max_attempts(3)
        .submit()
        .await?;

    println!(
        "landed at slot {} (paid {} lamports, attempt {})",
        outcome.slot, outcome.landed_tip_lamports, outcome.attempts
    );
    Ok(())
}

fn load_keypair(path: &str) -> Result<Keypair> {
    let expanded = if let Some(rest) = path.strip_prefix('~') {
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))?;
        format!("{home}{rest}")
    } else {
        path.to_owned()
    };
    let bytes: Vec<u8> = serde_json::from_str(&fs::read_to_string(expanded)?)?;
    Ok(Keypair::from_bytes(&bytes).context("decode keypair")?)
}
