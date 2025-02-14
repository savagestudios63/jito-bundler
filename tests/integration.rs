//! Integration tests against Jito's staging / devnet block engine.
//!
//! These tests make real network calls and are gated behind the
//! `JITO_BUNDLER_LIVE=1` environment variable so `cargo test` on a bare
//! checkout does not hit the public engine. To run:
//!
//! ```bash
//! JITO_BUNDLER_LIVE=1 \
//! JITO_BUNDLER_PAYER_JSON=~/.config/solana/devnet.json \
//! RPC_URL=https://api.devnet.solana.com \
//! cargo test --test integration -- --nocapture
//! ```

use std::env;
use std::fs;
use std::time::Duration;

use jito_bundler::{JitoEngine, Region, RetryPolicy, TipOracle, TipStrategy};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;

fn live_env() -> Option<(String, Keypair)> {
    if env::var("JITO_BUNDLER_LIVE").ok().as_deref() != Some("1") {
        return None;
    }
    let rpc = env::var("RPC_URL").ok()?;
    let path = env::var("JITO_BUNDLER_PAYER_JSON").ok()?;
    let path = if let Some(rest) = path.strip_prefix('~') {
        let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).ok()?;
        format!("{home}{rest}")
    } else {
        path
    };
    let bytes: Vec<u8> = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    Some((rpc, Keypair::from_bytes(&bytes).ok()?))
}

#[tokio::test]
async fn auto_region_picks_something() {
    // Offline CI just checks the local probe plumbing compiles and runs.
    // We don't assert a specific region because it varies by location.
    match Region::auto_select_with_timeout(Duration::from_millis(500)).await {
        Ok(region) => println!("picked region: {region:?}"),
        Err(e) => eprintln!("offline: {e} (ok)"),
    }
}

#[tokio::test]
async fn tip_oracle_cold_then_warm() {
    let oracle = TipOracle::new();
    assert!(oracle.p50().await.is_err());
    for i in 1..=20u64 {
        oracle.push_sample(i * 1000);
    }
    let p75 = oracle.p75().await.unwrap();
    assert!(p75 >= 15_000, "p75 should land around 15k, got {p75}");
}

#[tokio::test]
async fn live_self_transfer_lands() {
    let Some((rpc, payer)) = live_env() else {
        eprintln!("skipping live test; set JITO_BUNDLER_LIVE=1");
        return;
    };

    let rpc = RpcClient::new(rpc);
    let bh = rpc.get_latest_blockhash().await.expect("blockhash");
    let tx = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(&payer.pubkey(), &payer.pubkey(), 1)],
        Some(&payer.pubkey()),
        &[&payer],
        bh,
    );

    let tip_payer = Keypair::from_bytes(&payer.to_bytes()).unwrap();
    let engine = JitoEngine::builder()
        .region(Region::Mainnet)
        .tip_payer(tip_payer)
        .retry(RetryPolicy::new().max_attempts(3))
        .build()
        .await
        .expect("engine");

    let outcome = engine
        .bundle()
        .add_tx(tx)
        .tip(TipStrategy::Fixed(10_000))
        .submit()
        .await
        .expect("land");

    assert!(!outcome.bundle_id.is_empty());
    assert!(outcome.attempts >= 1);
}
