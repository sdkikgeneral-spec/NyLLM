# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

**NyLLM / Winny型 Semantic Cache** — a design for a Winny-style P2P network that shares a *semantic answer cache* across users at human scale. The core idea: semantic-search a shared cache first; only run LLM inference on a miss, then register the signed result. This repo is **design-heavy and early-stage** — the authoritative artifact is the design set in `docs/`, and `poc/` is a single-node Rust proof of concept of the core loop (no P2P/reputation/revocation yet).

## Design docs are the source of truth — read before changing `poc/`

The PoC deliberately implements a *reduced* version of decisions made in the docs. Before modifying cache/volatility/signing logic, read the relevant section — code comments cite them by number (e.g. `設計メモ §4`):

- `docs/Winny_Type_Semantic_Cache_Architecture.md` — the clean **implementation spec** (v1.0). Start here.
- `docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md` — competitive analysis + reliability design (§1–9: threats, Sybil defense, volatility tags, copyright).
- `docs/Winny_Type_Semantic_Cache_AI_Concept.md` — original concept/philosophy.
- `docs/PoC_Minimal_Loop.md` — the PoC's own design notes.
- `docs/Roadmap.md` — the implementation roadmap (stages S1–S7, gates, and current status), extracted from Architecture.md §13.2. This is the single place tracking implementation progress — update its status column when a stage's state changes, not Architecture.md.

Deprecated docs go in `docs/archives/` (do not delete them).

## Implementation workflow rules

1. **Present the design in Plan Mode and get the user's review and approval before starting.** Do not start writing code without approval.
2. **After approval, always invoke the `project-leader` agent so it can assign the work to the appropriate specialized agent(s).** Claude does not implement directly — a specialized agent implements under the project-leader's direction.
3. **Once a goal is set, approval decisions are handled by `project-leader`.** Escalate to the user only when the project-leader itself is unsure.
4. **Whenever a component is implemented, tests must be implemented too.** Create `src/tests/test_<component>.rs` and confirm the tests pass before considering the work complete.

## Build & run (PoC)

Build system is **Cargo**. External-dependency-free at the mock/default level (deps are small pure-Rust crates: `serde`, `serde_json`, `sha2`, `hex`, `rand`, `chrono`).

```sh
cd poc
cargo build
cargo run                    # add -- --keep to preserve the existing cache_store/
```

Optional feature flags (both default off; the full loop runs with them off):

```sh
cargo build --features ed25519   # real Ed25519 signatures via ed25519-dalek (else DummySigner MAC)
cargo build --features onnx       # real-embedding extension point (currently an unwired stub, else MockEmbedder)
```

There is no test suite yet. `src/main.rs` **is** the end-to-end demo (6 questions → hit/miss → register → tamper-detection); running the binary is how you verify behavior. Runtime artifacts `poc/cache_store/` (entry JSON) and `poc/keys/node.key` (node key) are generated in the CWD and git-ignored.

## PoC architecture

Trait-driven with a factory function per swappable concern, so mock and real implementations share one call path. `main.rs` wires them together:

- `Embedder` trait / `create_embedder()` (`embedder.rs`, `onnx_embedder.rs`) — question → normalized vector. Default `MockEmbedder` is a char-n-gram hash: exact matches hit at sim=1.0 but **paraphrases do not** (real semantic matching needs a real embedding model — this limitation is intentional and visible in the demo). `OnnxEmbedder` (`feature = "onnx"`) is currently an unwired stub (no `ort` crate dependency yet), not a working ONNX path.
- `Signer` trait / `create_signer()` (`signer.rs`) — default `DummySigner` is a keyed MAC (not publicly verifiable, placeholder only); `Ed25519Signer` (`signer/ed25519_signer.rs`, `feature = "ed25519"`) is real Ed25519 via `ed25519-dalek`.
- `Agent` trait / `create_agent()` (`agent.rs`) — the LLM. PoC uses `MockAgent` (canned answers); real-LLM path plugs into this trait.
- `SemanticCache` (`cache.rs`) — brute-force cosine search (O(n), fine at PoC scale), JSON persistence via `serde`/`serde_json`, verify-on-load.
- volatility + share gate (`volatility.rs`) — L0 lexical rules only.

`poc/python_prototype/` is a retired reference implementation kept because it shows a *real* sentence-transformers embedder and a real Claude agent; it is not the build target.

### Invariants to preserve

These encode the security model — changing them silently breaks the design:

- **`entry_id = sha256(signed_payload)`** and is the on-disk filename. `signed_payload()` covers question+answer+created+volatility, serialized via `serde_json::json!` (the default `Map` backing is a `BTreeMap`, so keys are sorted → canonical; do not enable the `preserve_order` cargo feature on `serde_json`, which would break this). Change what's signed → change both the id and `verify()` together.
- **Verify on load**: `SemanticCache::load()` recomputes the hash and checks `author_sig`; entries failing either are dropped. This is the tamper-detection the demo exercises. Hash = *detection*; signature = *forgery prevention* — keep them distinct (see 設計メモ §4).
- **Conservative share gate**: `share_gate()` is an AND of context-independence, factual (non-subjective, non-personal), and non-volatile. Default is *not shareable*. Bias is toward slow/volatile ("疑わしきは共有しない") because a bad shared entry pollutes the network at scale.
- **Thresholds**: `LOCAL_THRESHOLD = 0.80`, `SHARED_THRESHOLD = 0.90` (shared is stricter — precision over recall).

## Coding conventions

- **File naming**: project-internal files use the `nyllm_` prefix. Probe/one-off scripts in particular go under `scripts/nyllm_probe*.py`.
- **Real implementation layout**: for the real (non-PoC) implementation, the core layer lives under `src/core/` and is written in Rust; the UI layer lives under `src/ui/` and is written in Blazor (C#). This is distinct from the `poc/` prototype, which keeps its own existing flat layout (`poc/src/*.rs`) — do not retrofit this layout onto `poc/`.
- **Comments**: write code comments in Japanese.
- **Brace style (Allman)**: the opening brace `{` always starts on its own new line, e.g.:
  ```rs
  fn abc()
  {
      // ...
  }
  ```
  Caveat: stable `rustfmt` cannot auto-enforce this for Rust — `brace_style` is a nightly-only/unstable rustfmt option, so running `cargo fmt` on the stable toolchain will reformat braces back to the same line. For C#/Blazor this is `dotnet format`'s default and needs no extra config. Until the Rust side adopts nightly rustfmt with `unstable_features` (or an equivalent CI check), Allman-style Rust braces must be kept correct by hand and `cargo fmt` should not be run indiscriminately over brace placement.

## Project-wide tech decisions

- **Core = Rust** (performance on the search/verify hot path, plus memory safety on the paths that will parse untrusted data from adversarial peers once P2P/DHT lands — this was a C++ decision originally, revised to Rust before S2). **UI = C# Blazor** (planned, not started); the Rust core and Blazor will need interop (leading candidate: run the Rust core as a local daemon behind HTTP/gRPC, e.g. `axum`/`tonic`).
- **License = AGPL-3.0 now**, with a planned migration to **Apache-2.0** once mature. All contributions require the CLA (`CLA.md`) — this is what makes the future relicense possible. Do not add code under conflicting license terms.
- Default git branch is `main`.
