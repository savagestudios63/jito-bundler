//! Demonstrates custom retry policy with tip bumping.
//!
//! The bundle below uses an aggressive 5-attempt policy: start at p50, bump
//! the tip by 30 % between retries, and cap the total wall clock with a
//! 1-second initial backoff doubling to 8 seconds.

use std::env;
use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
use jito_bundler::{JitoEngine, RetryPolicy, TipStrategy};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jito_bundler=debug".into()),
        )
        .init();

    let rpc = env::var("RPC_URL").context("set RPC_URL")?;
    let payer = load_keypair(&env::var("PAYER_JSON").context("set PAYER_JSON")?)?;
    let tip_payer = load_keypair(&env::var("PAYER_JSON")?)?;

    let rpc = RpcClient::new(rpc);
    let bh = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), 1)],
        Some(&payer.pubkey()),
        &[&payer],
        bh,
    );

    let policy = RetryPolicy::new()
        .max_attempts(5)
        .initial_backoff(Duration::from_secs(1))
        .backoff_multiplier(2.0)
        .max_backoff(Duration::from_secs(8))
        .tip_bump_bps(3000); // +30 %

    let engine = JitoEngine::builder().tip_payer(tip_payer).build().await?;

    match engine
        .bundle()
        .add_tx(tx)
        .tip(TipStrategy::Fixed(5_000))
        .retry_policy(policy)
        .submit()
        .await
    {
        Ok(out) => println!(
            "landed in slot {} after {} attempt(s); paid {} lamports",
            out.slot, out.attempts, out.landed_tip_lamports
        ),
        Err(jito_bundler::Error::RetriesExhausted { attempts, last }) => {
            eprintln!("gave up after {attempts} attempts; last error: {last}");
        }
        Err(e) => eprintln!("unexpected error: {e}"),
    }
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
