# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

**NyLLM / Winny型 Semantic Cache** — a design for a Winny-style P2P network that shares a *semantic answer cache* across users at human scale. The core idea: semantic-search a shared cache first; only run LLM inference on a miss, then register the signed result. This repo is **design-heavy and early-stage** — the authoritative artifact is the design set in `docs/`, and `poc/` is a single-node C++ proof of concept of the core loop (no P2P/reputation/revocation yet).

## Design docs are the source of truth — read before changing `poc/`

The PoC deliberately implements a *reduced* version of decisions made in the docs. Before modifying cache/volatility/signing logic, read the relevant section — code comments cite them by number (e.g. `設計メモ §4`):

- `docs/Winny_Type_Semantic_Cache_Architecture.md` — the clean **implementation spec** (v1.0). Start here.
- `docs/Winny_Type_Semantic_Cache_信頼性設計メモ.md` — competitive analysis + reliability design (§1–9: threats, Sybil defense, volatility tags, copyright).
- `docs/Winny_Type_Semantic_Cache_AI_Concept.md` — original concept/philosophy.
- `docs/PoC_Minimal_Loop.md` — the PoC's own design notes.

Deprecated docs go in `docs/archives/` (do not delete them).

## Build & run (PoC)

Build system is **Meson** (not CMake — CMake was removed). External-dependency-free by default.

```sh
cd poc
meson setup build
meson compile -C build
./build/semantic_cache_poc          # add --keep to preserve the existing cache_store/
```

If `meson`/`ninja` aren't on PATH, the pip modules work without a global install:

```sh
python -m pip install meson ninja
python -m mesonbuild.mesonmain setup build
python -m mesonbuild.mesonmain compile -C build
```

On Windows, Meson auto-detects MSVC and `meson compile` auto-activates the compiler environment (no Developer Prompt needed).

Optional feature flags (both default `false`; the full loop runs with them off):

```sh
meson setup build -Duse_sodium=true   # real Ed25519 signatures via libsodium (else DummySigner MAC)
meson setup build -Duse_onnx=true     # real embeddings via ONNX Runtime (else MockEmbedder)
# reconfigure an existing build dir:
meson configure build -Duse_sodium=true
```

These map to the compile defines `POC_USE_SODIUM` / `POC_USE_ONNX`.

There is no test suite yet. `src/main.cpp` **is** the end-to-end demo (6 questions → hit/miss → register → tamper-detection); running the binary is how you verify behavior. Runtime artifacts `poc/cache_store/` (entry JSON) and `poc/keys/node.key` (node key) are generated in the CWD and git-ignored.

## PoC architecture

Interface-driven with a factory per swappable concern, so mock and real implementations share one call path. `main.cpp` wires them together:

- `IEmbedder` / `create_embedder()` (`embedder.hpp`, `onnx_embedder.hpp`) — question → normalized vector. Default `MockEmbedder` is a char-n-gram hash: exact matches hit at sim=1.0 but **paraphrases do not** (real semantic matching needs the ONNX path — this limitation is intentional and visible in the demo).
- `ISigner` / `create_signer()` (`signer.hpp`) — default `DummySigner` is a keyed MAC (not publicly verifiable, placeholder only); `SodiumSigner` is real Ed25519.
- `IAgent` / `create_agent()` (`agent.hpp`) — the LLM. PoC uses `MockAgent` (canned answers); real-LLM path plugs into this interface.
- `SemanticCache` (`cache.hpp`) — brute-force cosine search (O(n), fine at PoC scale), JSON persistence, verify-on-load.
- volatility + share gate (`volatility.hpp`) — L0 lexical rules only.
- `vendor/` — bundled single-header deps (`nlohmann/json.hpp`, self-written `sha256.hpp`).

`poc/python_prototype/` is a retired reference implementation kept because it shows a *real* sentence-transformers embedder and a real Claude agent; it is not the build target.

### Invariants to preserve

These encode the security model — changing them silently breaks the design:

- **`entry_id = sha256(signed_payload)`** and is the on-disk filename. `signed_payload()` covers question+answer+created+volatility, serialized via nlohmann::json (keys are sorted → canonical). Change what's signed → change both the id and `verify()` together.
- **Verify on load**: `SemanticCache::load()` recomputes the hash and checks `author_sig`; entries failing either are dropped. This is the tamper-detection the demo exercises. Hash = *detection*; signature = *forgery prevention* — keep them distinct (see 設計メモ §4).
- **Conservative share gate**: `share_gate()` is an AND of context-independence, factual (non-subjective, non-personal), and non-volatile. Default is *not shareable*. Bias is toward slow/volatile ("疑わしきは共有しない") because a bad shared entry pollutes the network at scale.
- **Thresholds**: `kLocalThreshold = 0.80`, `kSharedThreshold = 0.90` (shared is stricter — precision over recall).

## Project-wide tech decisions

- **Core = C++** (performance on the search/verify hot path). **UI = C# Blazor** (planned, not started); the C++ core and Blazor will need interop (leading candidate: run the C++ core as a local daemon behind HTTP/gRPC).
- **License = AGPL-3.0 now**, with a planned migration to **Apache-2.0** once mature. All contributions require the CLA (`CLA.md`) — this is what makes the future relicense possible. Do not add code under conflicting license terms.
- Default git branch is `main`.
