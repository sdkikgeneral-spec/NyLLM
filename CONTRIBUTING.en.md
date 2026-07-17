# Contributing Guide

**English** | [日本語](./CONTRIBUTING.md)

## 1. Introduction

**Status: 🚧 Design phase complete / PoC (S1: single-node minimal loop) in progress** — there is no live network yet.

At this stage of the project, the most valuable contribution is **discussion**. Before you start writing code, please share your idea in an Issue / Discussion first. A large PR dropped without warning takes a long time to reconcile against design intent, and the work may go to waste.

Themes we especially welcome:

- Embedding model selection and empirical measurement of the shared threshold τ (0.9 is a floor — see §6)
- Initial construction of the predicate ontology (volatility classes) and multilingual support
- Parameter design for local EigenTrust
- Choice of DHT implementation (viability of reusing Kademlia-family designs)

Small PRs — typo fixes, documentation clarifications, minor bug fixes — are of course welcome without prior discussion.

## 2. The design documents are the source of truth

**In this project `docs/` is authoritative, and the code in `poc/` is a deliberately *reduced* implementation of those decisions.** When code and documentation appear to disagree, assume the documentation is right. The reductions in `poc/` are intentional, and they are recorded in the PoC's own design notes.

Reading order:

| # | Document | Role |
|---|---|---|
| 1 | [Architecture design](./docs/Winny_Type_Semantic_Cache_Architecture.md) | The clean implementation spec. **Start here** |
| 2 | [Competitive analysis & reliability design notes](./docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md) | Primary decisions on threat model, Sybil defense, volatility, copyright (§1–9) |
| 3 | [Original concept](./docs/Winny_Type_Semantic_Cache_AI_Concept.md) | Philosophy and prototype. Closer to a historical document |
| 4 | [PoC minimal loop design notes](./docs/PoC_Minimal_Loop.md) | The PoC's own design and its intentional simplifications |

(The design documents are written in Japanese.)

Conventions to observe:

- **Do not break the section-number citation chain.** Code comments in the PoC cite the reliability design notes by section number in the form `設計メモ §4`, and the architecture document cross-references by § number as well. If you add, remove, or renumber a section, search `poc/src/` and all of `docs/` and update the references to follow.
- **Deprecated documents are moved to `docs/archives/`, never deleted.** When moving one, add a note at the top of the moved file explaining why it was deprecated and what supersedes it.
- **If you find a contradiction, do not unilaterally delete one side.** Determine which is the upstream decision, and raise an Issue.

## 3. Development environment

All you need is [Rust](https://www.rust-lang.org/tools/install) (Cargo). The default build depends only on small pure-Rust crates (`serde` / `serde_json` / `sha2` / `hex` / `rand` / `chrono`) — no external models or runtimes required.

```sh
cd poc
cargo build
cargo run

# run while preserving the existing cache
cargo run -- --keep
```

Cargo features (both default off — **the minimal loop runs end-to-end with them off**):

| feature | When ON |
|---|---|
| `ed25519` | Signing switches from `DummySigner` to real Ed25519 via `ed25519-dalek` |
| `onnx` | The **extension point** for a real embedder. `OnnxEmbedder` is currently an **unwired stub**, not a working ONNX path |

```sh
cargo build --features ed25519
```

**Understand the limitations of the default configuration** (both are deliberate simplifications, and both are visible in the demo):

- **The default embedder is `MockEmbedder`** — a char-n-gram hash. **Exact matches hit at sim=1.0, but paraphrases do not.** Real semantic matching requires a real embedding model.
- **The default signer is `DummySigner`** — a keyed MAC, i.e. a **placeholder that is not publicly verifiable**. If you need public verifiability, use `--features ed25519`.

Running the binary generates `cache_store/` (entry JSON) and `keys/node.key` (node key) in the current directory. **These are git-ignored runtime artifacts — do not commit them.**

See [`poc/README.md`](./poc/README.md) for details (it is the source for the build instructions).

## 4. Coding conventions

- **Write code comments in Japanese.**
- **Allman brace style** — the opening brace `{` always starts on its own new line.

  ```rs
  fn abc()
  {
      // ...
  }
  ```

  **Caution: do not run `cargo fmt` across the tree for brace formatting.** rustfmt's `brace_style` option, which controls brace placement, is nightly-only (unstable), so running `cargo fmt` on the stable toolchain will reformat braces back onto the same line. Until the Rust side adopts nightly rustfmt, Allman braces must be maintained by hand. On the C#/Blazor side, Allman is `dotnet format`'s default and needs no extra configuration.
- **`nyllm_` filename prefix** — project-internal files carry the `nyllm_` prefix. Throwaway probe scripts go under `scripts/nyllm_probe*.py`.
- **Layout of the real implementation** — in the production implementation, the core layer lives in `src/core/` (Rust) and the UI layer in `src/ui/` (Blazor / C#). However, **`poc/` keeps its existing flat layout (`poc/src/*.rs`)**. Do not retrofit this layout onto `poc/`.

## 5. Tests

When you implement a component, **you must implement tests along with it**. Create `src/tests/test_<component>.rs`, and do not consider the work complete until you have confirmed the tests pass.

To be honest about the current state: the PoC has a unit test suite under `poc/src/tests/` (cache / volatility / signer, plus an `#[ignore]`d search benchmark), runnable with `cargo test`. In addition, `poc/src/main.rs` **is** the end-to-end demo (6 questions → hit/miss → register → tamper detection), and running the binary via `cargo run` remains a valid way to verify behavior end to end. Contributions that expand the tests are welcome.

## 6. Invariants to read before you touch anything

The following *are* the security model. **Changing them silently breaks the design.** The rationale lives in the [reliability design notes](./docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md) and the [architecture design](./docs/Winny_Type_Semantic_Cache_Architecture.md).

- **`entry_id = sha256(signed_payload)`**, and this is the on-disk filename. `signed_payload()` covers question + answer + created + volatility, serialized via `serde_json::json!` (the default `Map` backing is a `BTreeMap`, so keys are sorted → canonical). **Do not enable the `preserve_order` cargo feature on `serde_json`** — it breaks key ordering and destroys canonicality. If you change what is signed, change the id and `verify()` **together**.
- **Verify on load** — `SemanticCache::load()` recomputes the hash and checks `author_sig`; entries failing either are dropped. **Hash = *tamper detection* (it is a public function, so an attacker can recompute it too) / signature = *forgery prevention*** — keep the two distinct (see 設計メモ §4). Note that `DummySigner::verify` rejecting entries signed with a key other than the node's own is **intended behavior stemming from the limits of a MAC, not a bug**. If you loosen `verify` while writing the P2P load path, forgery prevention disappears. **The fix is `--features ed25519`, not relaxing `verify`.**
- **Conservative share gate** — `share_gate()` is an AND of context-independence, factual (non-subjective, non-personal), and non-volatile, and the default is **not shareable**. The bias is toward "when in doubt, do not share," because at human scale a single bad shared entry pollutes the network. **The PoC's gate is a reduction implementing L0 (lexical) only**; the real gate also includes the L2 "answerable standalone" judgment and the regurgitation filter ([Architecture §7](./docs/Winny_Type_Semantic_Cache_Architecture.md)). **You may add AND conditions, but you must not remove them.**
- **Store facts as triples only (R1)** — `answer: String` (plaintext answer) in `poc/src/cache.rs` is a **PoC reduction**. The real rule is R1: **store only fact triples + provenance metadata, re-synthesize the answer on the receiving side, and never register content that cannot be expressed as triples into Public** ([Architecture §11 R1 / T-H](./docs/Winny_Type_Semantic_Cache_Architecture.md)). When extending the PoC toward sharing, do not put `answer` on the network as-is.
- **Thresholds** — `LOCAL_THRESHOLD = 0.80` / `SHARED_THRESHOLD = 0.90`. The shared side is stricter because precision is prioritized over recall. **The shared τ ≥ 0.9 is a requirement (a floor)** ([Architecture §5.1 / T-A](./docs/Winny_Type_Semantic_Cache_Architecture.md)), not merely a tuning value. Measurement may **raise** it, but it **cannot be lowered**. Lowering it requires changing Architecture §5.1 / T-A first.

> PRs touching cache / signing / volatility / the share gate are subject to threat-model review. Please expect review to take time.

## 7. Sending a PR

- The default branch is `main`. **Do not commit directly to `main` — create a branch.**
- **One PR, one concern.** Do not mix in unrelated changes.
- **Changes that involve a design decision need an Issue first.** As stated above, `docs/` is authoritative, so anything affecting the design needs agreement on the documentation side first.
- Confirm `cargo build`, `cargo test`, and `cargo run` pass before committing (and mind the caution in §4 about running `cargo fmt` across the tree).

## 8. CLA and license

**Before sending any Contribution — including code and documentation — agreement to [CLA.md](./CLA.md) is required.** The reason is to make the license migration from AGPL-3.0 to Apache-2.0 (once the project matures) possible — that migration is impossible unless every contributor's rights are clear. **For how to agree, see the [同意方法 (How to agree) section of CLA.md](./CLA.md#同意方法)**; it is not duplicated in this guide.

The current license is [GNU AGPL-3.0](./LICENSE). The [License section of the README](./README.en.md#license) is the single source of truth for the full policy.

Do not bring in code under conflicting license terms (for the representation regarding material that carries third-party license terms, see [CLA.md §4(c)](./CLA.md)).

> Note: this guide, and its statements about licensing and the CLA, are not legal advice.
