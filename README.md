# jito-bundler

[![Crates.io](https://img.shields.io/crates/v/jito-bundler.svg)](https://crates.io/crates/jito-bundler)
[![Docs.rs](https://docs.rs/jito-bundler/badge.svg)](https://docs.rs/jito-bundler)
[![CI](https://github.com/jito-bundler/jito-bundler/actions/workflows/ci.yml/badge.svg)](https://github.com/jito-bundler/jito-bundler/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

An ergonomic Rust client for [Jito](https://jito.wtf) block-engine bundles.
It hides the tedious bits of bundle submission — region selection, tip
injection, retries, status tracking — behind a single fluent builder.

```rust
use jito_bundler::{JitoEngine, TipStrategy};

let engine = JitoEngine::auto_region().await?.with_tip_payer(my_keypair);

let tip = engine.tip_oracle().p75().await?;

let outcome = engine
    .bundle()
    .add_tx(signed_tx)
    .tip(TipStrategy::Fixed(tip))
    .max_attempts(3)
    .submit()
    .await?;

println!("landed at slot {}", outcome.slot);
```

## Why this crate?

Jito ships [`jito-json-rpc-client`](https://crates.io/crates/jito-json-rpc-client)
as a thin JSON-RPC wrapper. It's the right primitive, but every bot ends up
reinventing the same layer on top. `jito-bundler` *is* that layer.

| | `jito-json-rpc-client` | **`jito-bundler`** |
|---|---|---|
| JSON-RPC transport | ✅ | ✅ |
| gRPC transport     | ❌ | ✅ (feature `grpc`) |
| Fluent builder API | ❌ | ✅ |
| Automatic region selection | ❌ | ✅ |
| Automatic tip injection    | ❌ | ✅ |
| Streaming tip oracle (p50/p75/p95) | ❌ | ✅ (feature `tip-stream`) |
| Retry + tip bumping on drops | ❌ | ✅ |
| Typed bundle results (`Landed` / `Dropped` / `Rejected`) | ❌ | ✅ |
| Prometheus metrics | ❌ | ✅ (feature `metrics`) |
| Examples + integration tests | partial | ✅ |

## Installation

```toml
[dependencies]
jito-bundler = "0.1"
```

Feature flags:

| flag | default | enables |
|---|---|---|
| `tip-stream` | ✅ | WebSocket tip-stream oracle |
| `grpc`       | ❌ | gRPC transport via `tonic` + `prost` |
| `metrics`    | ❌ | Prometheus metrics under `jito_bundler_*` |

## Examples

Runnable examples live in [`examples/`](examples):

- [`simple_bundle.rs`](examples/simple_bundle.rs) — one tx, fixed tip, default retries
- [`with_retries.rs`](examples/with_retries.rs) — custom retry policy + tip bumping
- [`with_tip_oracle.rs`](examples/with_tip_oracle.rs) — live tip-stream percentile queries

```bash
RPC_URL=https://api.mainnet-beta.solana.com \
PAYER_JSON=~/.config/solana/id.json \
cargo run --example simple_bundle
```

## Minimum supported Rust version

The crate targets **Rust 1.75**. The CI matrix builds on stable and beta.

## Integration tests

The test in `tests/integration.rs` hits Jito's public block engine and is
gated behind `JITO_BUNDLER_LIVE=1`:

```bash
JITO_BUNDLER_LIVE=1 \
JITO_BUNDLER_PAYER_JSON=~/.config/solana/devnet.json \
RPC_URL=https://api.devnet.solana.com \
cargo test --test integration -- --nocapture
```

## License

MIT. See [LICENSE](LICENSE).
