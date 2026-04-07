---
layout: default
title: Validation Suite
parent: Development
nav_order: 3
---

# Allux Validation Suite (Autonomous CLI)
{: .no_toc }

This suite validates the full autonomous workflow against a real sample project:
- Analyze existing code
- Apply file changes
- Run checks/tests
- Request follow-up updates
- Process multiple prompts sequentially

<details open markdown="block">
<summary>Table of contents</summary>
{: .text-delta }
1. TOC
{:toc}
</details>

---

## 1. Sample Project

Use: `sandbox/sample-rust-app`

The sample intentionally includes:
- A small Rust binary
- An inefficient/verbose implementation in `src/lib.rs`
- Tests in `src/lib.rs`

---

## 2. Single Prompt Validation

Run one autonomous prompt:

```bash
node --experimental-strip-types scripts/allux-cli.ts ask "Work only inside sandbox/sample-rust-app. Improve the implementation in src/lib.rs for clarity, keep behavior, then run cargo test there. Respond in English." --autonomous --max-rounds 8 --verbose
```

**Expected:**
- Tool calls include `read_file`, `replace_in_file` or `write_file`, and `bash`
- Output confirms what changed
- `cargo test` succeeds

---

## 3. Multi-Input Sequential Validation

Use batch file: `validation/prompts-sequential.txt`

```bash
node --experimental-strip-types scripts/allux-cli.ts ask --batch-file validation/prompts-sequential.txt --autonomous --max-rounds 8 --verbose
```

**Behavior:**
- Prompts are executed one by one
- Each prompt waits for completion before next starts
- Errors are reported and execution continues by default (use `--stop-on-error` to halt)

---

## 4. Master Supervisor Validation

Run managed cycles with ready token:

```bash
node --experimental-strip-types scripts/master-automejora.ts --cycles 2 --interval-ms 1000 --prompt "Work inside sandbox/sample-rust-app only. Apply one safe improvement and run cargo test. Include CLI_READY_TO_RESTART only when done." --max-rounds 8
```

**Expected:**
- Supervisor waits for child completion
- If child returns non-zero, it restarts
- If `scripts/allux-cli.ts` changed, next cycle uses the new version
- Exits early when ready token is detected

---

## 5. Regression Checks

After autonomous runs:

```bash
cargo check
git status --short
```

**Review:**
- Ensure intended files changed
- No accidental large edits
- No broken build

---

## 6. Token Compression Technologies — Research & Feasibility Analysis

This section surveys current token compression technologies (2024–2026) relevant to Allux as a Rust-based CLI agent that interacts with LLM APIs. Each technique is evaluated for technical feasibility of integration.

---

### 6.1 Prompt Compression Techniques

#### 6.1.1 LLMLingua (Microsoft)

Uses a small language model (GPT-2, LLaMA-7B) to identify and remove non-essential tokens based on perplexity scoring. Employs budget controller, iterative token-level compression, and distribution alignment.

- **Paper:** [arXiv:2310.05736](https://arxiv.org/abs/2310.05736) (EMNLP 2023)
- **GitHub:** [microsoft/LLMLingua](https://github.com/microsoft/LLMLingua)
- **Website:** [llmlingua.com](https://llmlingua.com/)
- **Metrics:** Up to 20x compression with minimal performance loss across QA, summarization, and reasoning tasks.
- **Status:** Production-ready Python library, actively maintained by Microsoft Research.
- **Rust feasibility:** Medium. Requires running a small LM for perplexity scoring — would need a Python subprocess, porting via `candle`/`llama.cpp` bindings, or external API call.

#### 6.1.2 LLMLingua-2 (Microsoft)

Reformulates prompt compression as a **token classification** problem. Uses GPT-4-distilled supervision to train a small Transformer encoder that classifies each token as keep/drop. Extractive approach (preserves original tokens).

- **Paper:** [arXiv:2403.12968](https://arxiv.org/abs/2403.12968) (ACL 2024 Findings)
- **GitHub:** [microsoft/LLMLingua](https://github.com/microsoft/LLMLingua) (same repo)
- **Metrics:** 2x–5x compression, 3x–6x faster than LLMLingua-1. End-to-end latency improvement of 1.6x–2.9x.
- **Status:** Production-ready, part of the same Microsoft library.
- **Rust feasibility:** More feasible than v1. The classifier model is BERT-sized and could run via ONNX Runtime in Rust or via `candle`.

#### 6.1.3 LongLLMLingua (Microsoft)

Extends LLMLingua for long-context scenarios with question-aware coarse-to-fine compression, document reordering, and dynamic compression ratios. Designed for RAG and long-document QA.

- **Paper:** [arXiv:2310.06839](https://arxiv.org/abs/2310.06839)
- **GitHub:** [microsoft/LLMLingua](https://github.com/microsoft/LLMLingua)
- **Metrics:** Up to 17.1x compression on long contexts.
- **Status:** Production-ready (same library).
- **Rust feasibility:** Same as LLMLingua-1 (Medium).

#### 6.1.4 Selective Context

Prunes low self-information tokens from prompts using a base causal language model (e.g., GPT-2).

- **Paper:** [arXiv:2310.06201](https://arxiv.org/abs/2310.06201)
- **GitHub:** [liyucheng09/Selective_Context](https://github.com/liyucheng09/Selective_Context)
- **Metrics:** ~2x content processing capacity, ~40% memory/GPU time savings.
- **Status:** Research with working code. Simpler than LLMLingua but less flexible.
- **Rust feasibility:** Medium. Same dependency on running a small LM.

#### 6.1.5 SCOPE (Generative Compression)

Uses chunking-and-summarization: splits prompts into semantically coherent chunks, rewrites each chunk concisely, then reconstructs. Includes outlier chunk handling, dynamic compression ratio, and keyword preservation.

- **Paper:** [arXiv:2508.15813](https://arxiv.org/abs/2508.15813) (Aug 2025)
- **Metrics:** Better compression quality and stability than extractive methods under high compression ratios.
- **Status:** Research (2025).
- **Rust feasibility:** Low-Medium. Requires an LLM for generative rewriting — adds latency and cost.

#### 6.1.6 CompactPrompt

Unified end-to-end pipeline merging hard prompt compression with file-level data compression. Prunes low-information tokens via self-information scoring and dependency-based phrase grouping. Also applies n-gram abbreviation and uniform quantization for numerical data.

- **Paper:** [arXiv:2510.18043](https://arxiv.org/abs/2510.18043) (ACM ICAIF '25)
- **Metrics:** Up to 60% token reduction on TAT-QA and FinQA with <5% accuracy drop.
- **Status:** Research with working implementation.
- **Rust feasibility:** Medium-High. N-gram abbreviation and quantization are pure algorithmic (easy to port). Self-information scoring still needs a small LM.

#### 6.1.7 Gist Tokens (Soft Prompt Compression)

Trains a language model to compress prompts into smaller sets of virtual "gist" tokens via modified attention masks during instruction finetuning.

- **Paper:** [arXiv:2304.08467](https://arxiv.org/abs/2304.08467) (NeurIPS 2023)
- **GitHub:** [jayelm/gisting](https://github.com/jayelm/gisting)
- **Metrics:** Up to 26x compression, ~40% FLOPs reduction.
- **Status:** Research. Requires model fine-tuning — **not applicable to API-based usage**.
- **Rust feasibility:** Not applicable for API-based CLI agents.

---

### 6.2 KV-Cache Compression

> These techniques are relevant for **self-hosted models only**, not API-based usage. Included for completeness.

#### 6.2.1 StreamingLLM (MIT)

Exploits the "attention sink" phenomenon: maintains a small set of initial "sink" tokens plus a rolling window of recent tokens for stable infinite-length inference.

- **Paper:** [arXiv:2309.17453](https://arxiv.org/abs/2309.17453) (ICLR 2024)
- **GitHub:** [mit-han-lab/streaming-llm](https://github.com/mit-han-lab/streaming-llm)
- **Metrics:** Stable inference with 4M+ tokens, up to 22.2x speedup. Works on Llama-2, MPT, Falcon without finetuning.
- **Status:** Production-ready for self-hosted models.

#### 6.2.2 SnapKV

Compresses KV caches by selecting/clustering significant KV positions based on attention scores within instruction token windows.

- **Paper:** [arXiv:2404.14469](https://arxiv.org/abs/2404.14469)
- **Metrics:** 92% compression at 1024 tokens, 68% at 4096 tokens, negligible accuracy drops.
- **Status:** Research with implementations.

#### 6.2.3 PyramidKV

Allocates different KV cache sizes across layers: lower layers get larger caches (dense attention), higher layers get smaller (sparse), forming a pyramid.

- **Paper:** [arXiv:2406.02069](https://arxiv.org/abs/2406.02069)
- **GitHub:** [Zefan-Cai/KVCache-Factory](https://github.com/Zefan-Cai/KVCache-Factory) (unified implementation of multiple methods)

#### 6.2.4 KVTC (KV Cache Transform Coding)

Lightweight transform coder combining PCA-based feature decorrelation, adaptive quantization, and entropy coding.

- **Paper:** [arXiv:2511.01815](https://arxiv.org/abs/2511.01815) (ICLR 2026)
- **Metrics:** Up to 20x compression maintaining reasoning accuracy; 40x+ for specific use cases.
- **Status:** State-of-the-art research, accepted at ICLR 2026.

#### 6.2.5 CacheGen / CacheBlend (LMCache)

CacheGen encodes KV caches into compact bitstreams. CacheBlend enables reusing KV caches for non-prefix texts in RAG scenarios.

- **Papers:** [CacheGen arXiv:2310.07240](https://arxiv.org/abs/2310.07240) (SIGCOMM 2024), CacheBlend (Best Paper, EuroSys 2025)
- **GitHub:** [LMCache/LMCache](https://github.com/LMCache/LMCache)
- **Metrics:** 3.5–4.3x KV cache reduction, 3.2–3.7x delay reduction (CacheGen). 2.2–3.3x TTFT reduction (CacheBlend).

---

### 6.3 Context Distillation & Summarization

#### 6.3.1 ACON (Agent Context Optimization)

Unified framework for systematic and adaptive context compression for long-horizon LLM agents. Uses "compression guideline optimization" in natural language. Gradient-free — works with closed-source API models.

- **Paper:** [arXiv:2510.00615](https://arxiv.org/abs/2510.00615) (Oct 2025)
- **Metrics:** 26–54% memory reduction (peak tokens). Enables distillation to smaller models preserving 95% accuracy. Improves small LM agent performance by 20–46%.
- **Status:** Research. Gradient-free and API-compatible.
- **Rust feasibility:** **HIGH**. Works via natural language guidelines + API calls. A Rust CLI agent can detect growing context, apply ACON-style summarization rules, and compress conversation history.

#### 6.3.2 Progressive Summarization

Iteratively summarizes older parts of conversation/context as the session progresses, keeping recent content intact and compressing older content into summaries.

- **Status:** Well-established production pattern used in ChatGPT's memory, various agent frameworks.
- **Rust feasibility:** **HIGH**. Simple to implement: maintain a rolling window of recent messages and periodically summarize older ones via LLM API call.

---

### 6.4 Tokenizer-Level Compression

#### 6.4.1 TOON (Token-Oriented Object Notation)

Data serialization format designed for LLM token efficiency. Replaces JSON with a more compact notation using indentation instead of braces, field declarations, and minimal quoting.

- **Website:** [toonformat.dev](https://toonformat.dev/)
- **GitHub:** [toon-format/toon](https://github.com/toon-format/toon) (TypeScript SDK, specs, benchmarks)
- **MCP Server (Rust):** [WithTOON/toon-mcp-server](https://github.com/WithTOON/toon-mcp-server)
- **Rust crate:** `toon-format` on [docs.rs](https://docs.rs/toon-format)
- **Metrics:** ~40% fewer tokens than JSON, 74% accuracy vs JSON's 70% in mixed-structure benchmarks.
- **Status:** Production-ready with native Rust crate.
- **Rust feasibility:** **HIGH**. `cargo add toon-format` — drop-in replacement for JSON serialization.

#### 6.4.2 MultiTok (Variable-Length Tokenization)

Each token represents a variable number of sub-words, adapted from LZW compression.

- **GitHub:** [noelkelias/multitok](https://github.com/noelkelias/multitok)
- **Metrics:** ~33% data compression, ~3x faster training.
- **Status:** Research. Requires retraining models — **not applicable for API-based usage**.

---

### 6.5 Production-Oriented Tools & Services

#### 6.5.1 compression-prompt (Rust crate)

Pure Rust statistical filtering for prompt compression. Uses IDF scoring to identify and remove low-importance content while preserving keywords and entities. **No external model dependencies.**

- **Crate:** [compression-prompt on crates.io](https://crates.io/crates/compression-prompt)
- **Docs:** [docs.rs/compression-prompt](https://docs.rs/compression-prompt)
- **Metrics:** 50% token reduction, 91% quality retention (Claude Sonnet), <1ms compression time, 10.58 MB/s throughput. 100% keyword retention, 91.8% entity retention.
- **Status:** Production-ready.
- **Rust feasibility:** **HIGHEST**. `cargo add compression-prompt` — immediate integration.

#### 6.5.2 RTK (Rust Token Killer)

High-performance CLI proxy written in Rust that reduces LLM token consumption by filtering and compressing command outputs before they reach the LLM context. Single binary, zero dependencies, supports 100+ commands.

- **GitHub:** [rtk-ai/rtk](https://github.com/rtk-ai/rtk)
- **Metrics:** 60–90% token reduction on common dev commands, <10ms overhead.
- **Status:** Production-ready (v0.13.1). Works with Claude Code, Cursor, Gemini CLI, Aider, Codex.
- **Rust feasibility:** **HIGHEST**. Already Rust. Can be used as-is, forked, or studied.

#### 6.5.3 trimcp (Rust MCP Proxy)

MCP proxy built in Rust that reduces token costs with lossless strategies: ANSI stripping, JSON compaction, deduplication, code minification. Supports semantic tree-based indexing.

- **GitHub:** [rustkit-ai/trimcp](https://github.com/rustkit-ai/trimcp)
- **Status:** Production-ready, MCP-compatible.
- **Rust feasibility:** **HIGHEST**. Native Rust, MCP-compatible.

#### 6.5.4 llm-token-saver-rs (Rust crate)

Rust library for LLM token optimization with intelligent prompt compression, caching, selective truncation, and context engineering. Has tiered compression (tiers 2–5 can call an LLM to summarize).

- **GitHub:** [snailer-team/llm-token-saver-rs](https://github.com/snailer-team/llm-token-saver-rs)
- **Metrics:** 30–40% average token reduction in production (6K+ downloads).
- **Status:** Alpha. API may change.
- **Rust feasibility:** **HIGH**. Native Rust library.

#### 6.5.5 Headroom

Context optimization layer that auto-detects content types (JSON, code, logs, text) and routes each to the best compressor. AST-aware code compression supports Rust, Python, JS, Go, Java, C++.

- **GitHub:** [chopratejas/headroom](https://github.com/chopratejas/headroom)
- **Status:** Production-ready open-source (Python). Usable as proxy.
- **Rust feasibility:** **HIGH**. Run as local proxy, or reimplement compression strategies in Rust.

#### 6.5.6 The Token Company (SaaS)

Drop-in API middleware for prompt compression using custom "bear-1.1" models. Works with GPT, Claude, and all major LLMs.

- **Website:** [thetokencompany.com](https://thetokencompany.com)
- **Metrics:** Up to 66% compression while improving accuracy by up to 1.1%. Up to 37% faster latency.
- **Pricing:** $0.05/1M tokens.
- **Status:** Production SaaS (YC W26).
- **Rust feasibility:** **HIGH**. Simple API call before sending prompts to the main LLM.

#### 6.5.7 Lattice Proxy

Drop-in semantic compression proxy. Sits between your app and any LLM API. Long conversations (8k+ tokens) are compressed by summarizing middle history using a cheap model.

- **Website:** [latticeproxy.io](https://latticeproxy.io/)
- **Metrics:** Up to 93% token reduction. No code changes needed.
- **Status:** Production-ready (Python/FastAPI).
- **Rust feasibility:** **HIGH**. Just change the target API URL.

#### 6.5.8 Provider Prompt Caching (Anthropic / OpenAI)

LLM providers cache prompt prefixes across requests. Cached portions cost 50–90% less. Anthropic offers a 90% read discount on cached prefixes.

- **Status:** Built into APIs. No library needed.
- **Rust feasibility:** **HIGH**. Structure prompts so static content comes first (system prompt, instructions, examples) and dynamic content last.

#### 6.5.9 Semantic Caching (Redis LangCache)

Stores query vector embeddings and LLM responses in memory. Retrieves cached answers for semantically similar queries.

- **Metrics:** ~73% cost reduction in high-repetition workloads.
- **Status:** Production-ready (Redis product).
- **Rust feasibility:** **HIGH**. Redis has excellent Rust client libraries.

---

### 6.6 Feasibility Matrix for Allux Integration

Ranked by implementation effort and impact:

| Priority | Technique | Effort | Token Reduction | Notes |
|----------|-----------|--------|-----------------|-------|
| 1 | **Provider prompt caching** (prefix ordering) | Trivial | 50–90% cost on repeated prefixes | Just reorder prompts: static first |
| 2 | **compression-prompt** crate | Low | ~50% | `cargo add`, pure Rust, <1ms |
| 3 | **RTK** patterns / **trimcp** | Low | 60–90% on tool outputs | Already Rust; use or study |
| 4 | **TOON** format for structured data | Low | ~40% on JSON payloads | Rust crate available |
| 5 | **Progressive summarization** of history | Medium | 26–54% memory reduction | ACON-like rolling compression |
| 6 | **llm-token-saver-rs** | Medium | 30–40% | Native Rust, alpha quality |
| 7 | **Headroom** / **Lattice Proxy** | Low | Up to 93% | Run as local proxy |
| 8 | **The Token Company API** | Low | Up to 66% + accuracy gains | SaaS dependency, $0.05/1M tokens |
| 9 | **LLMLingua-2** (porting) | High | 2–5x compression | Needs small ML model in Rust |

---

### 6.7 Recommended Implementation Strategy for Allux

#### Phase 1 — Quick Wins (1–2 days)
1. **Prompt prefix ordering** for provider cache hits (Anthropic 90% discount)
2. **`compression-prompt`** crate for statistical prompt compression
3. **TOON format** for structured tool output serialization

#### Phase 2 — Tool Output Compression (3–5 days)
4. Integrate **RTK-style** filtering for command outputs (or use RTK directly)
5. Implement **trimcp-style** lossless compression: ANSI stripping, JSON compaction, deduplication

#### Phase 3 — Conversation Management (1–2 weeks)
6. **Progressive summarization** of conversation history (ACON-inspired)
7. **Semantic caching** with Redis for repeated queries
8. Tiered compression using **llm-token-saver-rs** patterns

#### Phase 4 — Advanced (Optional)
9. AST-aware code compression (inspired by Headroom)
10. Port LLMLingua-2 token classifier to Rust via ONNX Runtime

---

### 6.8 Further Reading

- [Prompt Compression for LLMs: A Survey](https://arxiv.org/abs/2410.12388) — NAACL 2025 (Selected Oral) | [GitHub](https://github.com/ZongqianLi/Prompt-Compression-Survey)
- [A Survey of Token Compression for Efficient Multimodal LLMs](https://arxiv.org/html/2507.20198v5) — TMLR 2026
- [Awesome-LLM-Compression](https://github.com/HuangOwen/Awesome-LLM-Compression) — Curated list
- [JetBrains: Efficient Context Management Research](https://blog.jetbrains.com/research/2025/12/efficient-context-management/)
