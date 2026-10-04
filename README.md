# axmg — Universal Merkle DAG Associative Memory Engine

[![CI](https://github.com/dchrnv/axmg/actions/workflows/ci.yml/badge.svg)](https://github.com/dchrnv/axmg/actions/workflows/ci.yml)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org)
[![Status: Stable v0.1.0](https://img.shields.io/badge/status-stable%20v0.1.0-green.svg)](https://github.com/dchrnv/axmg)

> **Embedded, deterministic, zero-vocabulary associative memory engine for Rust.**

`axmg` converts raw byte streams (text, logs, code, sensor telemetry) into a cryptographic **Merkle DAG (SHA-256)**, dynamically maintains active working memory through temporal noise eviction ($K=3$ epochs), calculates online information-theoretic surprise, and enables instant semantic recall via co-occurrence graphs (PPMI).

No vector databases. No embedding models. No dictionaries. 100% deterministic and byte-exact.

---

## ⚡ Key Highlights

- **Zero-Vocabulary Merkle DAG:** Operates purely on raw bytes. Level 0 consists of 256 factory bytes. All higher-level concepts, words, and phrases emerge bottom-up from co-occurrence frequency.
- **Dynamic Forgetting & Working Memory ($K=3$):** Generational GC without sweep. Active working memory is bounded to reachable roots from the last $K=3$ epochs, pruned by 25% lowest PPMI-weight borderlines.
- **Online Surprise Metric ($S_{\text{static}}, S_{\text{growth}}$):** Real-time measurement of compression efficiency and vocabulary growth without store cloning. Classifies inputs into `Familiar`, `NovelGrowth`, or `Noise`.
- **Sub-graph Associative Recall:** Instant recall of related concepts via sparse PPMI co-occurrence matrices and random walks.
- **ACID Persistence via `redb`:** Single-transaction atomic commits for token bodies and eviction logs. Resilient against crashes and power loss.
- **High Token Efficiency:** Proven **96.89% reduction** in LLM prompt context size (32.2x compression factor) when feeding active focus context.

---

## 🚀 Quickstart

Add `shelf` (the core engine crate) to your `Cargo.toml`:

```toml
[dependencies]
shelf = { path = "shelf" }
# Or via git:
# axmg = { package = "shelf", git = "https://github.com/dchrnv/axmg", branch = "main" }
```

### Basic Usage

```rust
use shelf::Axmg;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize an in-memory engine (or use Axmg::open("path.redb") for ACID disk storage)
    let mut memory = Axmg::in_memory();

    // 2. Ingest raw text or byte streams (turns the wheel, computes surprise, updates focus)
    let report = memory.ingest("Rust and Merkle DAG combine to build axmg associative memory")?;
    println!("Revolution: {}, Surprise Score: {:.3}, Verdict: {:?}", 
        report.revolution, report.surprise_score, report.verdict);

    // 3. Extract active focus context for LLM system prompt (K=3 working memory)
    println!("Prompt Context: {}", memory.prompt_context());

    // 4. Sub-graph associative recall
    let recall = memory.recall("Merkle", Some(5));
    println!("Associations: {}", recall.context_string);

    // 5. Read-only hypothesis checking (check surprise without modifying state)
    let surprise = memory.check_surprise("Rust systems programming");
    println!("S-static: {:.3} | Verdict: {:?}", surprise.s_static, surprise.verdict);

    Ok(())
}
```

Run the built-in quickstart example:
```bash
cargo run -p shelf --example quickstart
```

---

## 📖 Architecture & Data Flow

```
                      Raw Byte Stream (text, logs, code, sensors)
                                         │
                                         ▼
                     ┌───────────────────────────────────────┐
                     │          StreamingRecognizer          │
                     │  $O(N)$ Shift-Reduce Greedy Folding   │
                     └───────────────────┬───────────────────┘
                                         │
                                         ▼
                     ┌───────────────────────────────────────┐
                     │       Store (Merkle DAG Arena)        │
                     │  SoA Token Arena • SHA-256 Interning  │
                     └───────────────────┬───────────────────┘
                                         │
                   ┌─────────────────────┴─────────────────────┐
                   ▼                                           ▼
       ┌───────────────────────┐                   ┌───────────────────────┐
       │      WheelState       │                   │    DeathLog (K=3)     │
       │ Revolutions & Epochs  │                   │ Root Reachability GC  │
       │ S-static / S-growth   │                   │ 25% PPMI Weight Cut   │
       └───────────┬───────────┘                   └───────────┬───────────┘
                   │                                           │
                   └─────────────────────┬─────────────────────┘
                                         │
                                         ▼
                     ┌───────────────────────────────────────┐
                     │          High-Level Axmg API          │
                     │   ingest() • recall() • focus()       │
                     │   check_surprise() • stats() • save() │
                     └───────────────────────────────────────┘
```

---

## 🛠 API Cheat Sheet

| Method | Description |
|---|---|
| `Axmg::in_memory()` | Create an isolated in-memory engine instance. |
| `Axmg::open("path.redb")` | Open or create an ACID-persisted engine with automated timeline tracking. |
| `AxmgBuilder::new()` | Configurable builder (`birth_threshold`, `tie_break`, `focus_window`, etc.). |
| `memory.ingest(text)` | Ingest string, advance wheel, calculate surprise, update focus, and auto-persist. |
| `memory.ingest_bytes(bytes)` | Ingest arbitrary raw byte slices (logs, telemetry, binary streams). |
| `memory.recall(query, limit)` | Associative subgraph search returning related concepts and LLM prompt context string. |
| `memory.focus()` | Instant lock-free `Arc<FocusSet>` access to active working memory. |
| `memory.prompt_context()` | Formatted context string of active top concepts ready for LLM system prompts. |
| `memory.check_surprise(text)` | Non-mutating surprise evaluation on frozen state. |
| `memory.stats()` | Engine statistics (token count, compression ratio, active focus ratio). |
| `memory.save()` | Explicit transactional flush to disk. |

---

## 📚 Documentation & Integration

- [Detailed Integration Guide](docs/GUIDE.md) — embedding recipes for LLM agent loops, sensory streaming daemons, and vocalizers.
- [Fundamental Invariants](INVARIANTS.md) — architectural and cryptographic constraints of the Merkle storage engine.
- [Development Guide](DEV_GUIDE.md) — engineering rules and verification standards.
- **Local Rustdoc HTML Wiki:**
  ```bash
  cargo doc -p shelf --no-deps --open
  ```

---

## 🧪 Testing & Verification

Run the comprehensive test suite (100 unit & doc tests):
```bash
cargo test
```

---

## 📄 License

Licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0).
