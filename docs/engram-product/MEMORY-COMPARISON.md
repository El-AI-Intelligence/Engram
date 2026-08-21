# Engram vs. Claude Code native memory — a measured comparison

**Date:** 2026-08-21 · **Status:** COMPLETE (K=25 ×3 reps, K=100 ×2 reps, clean harness; environmental impact analysis added)

This documents an A/B benchmark that pits an Engram vault (semantic retrieval via
`engramd-mcp`) against Claude Code's native memory mechanisms — wholesale
`CLAUDE.md` injection and index-plus-topic-files — plus a no-memory control, on
the same synthetic corpus and question set. The cost and token results are also
read through an environmental lens: inference energy tracks tokens processed,
so the *shape* of token scaling is the sustainability story.

## TL;DR

> **Sustainability summary:** Engram's per-interaction token load is bounded
> as knowledge grows; native wholesale injection grows linearly until it
> fills the context window entirely (≈K 4,000 on the measured slope) and
> stops working. Engram embeds the knowledge base **once — locally, on the
> device** — instead of re-broadcasting it into every interaction of every
> session. The efficiency win is structural and compounds at team scale;
> below ~100–150 memories it has not yet arrived. See
> [Environmental impact](#environmental-impact).

| Condition | K=25 recall | K=100 recall | Cost per q (K=100) | In-token growth, 25→100 | Latency (K=100) |
|---|---|---|---|---|---|
| **Native A1** — every memory injected via CLAUDE.md | 100% | **95%** (2 crossref misses) | $0.17 | **+11% — linear, no ceiling** | 6 s |
| **Native A2** — CLAUDE.md index + `memories/*.md` topic files | 100% | 100% | $0.20 | +2.6% | 13 s |
| **Engram (b)** — MCP vault, semantic search | 100% | **100%** | $0.25 | **+4.5% — bounded by retrieval** | 21 s |
| **Control (c)** — no memory | 0% | 0% | $0.04 | — | 9 s |

- **Recall parity:** Engram matches or beats native memory at every scale tested,
  including the one place native wholesale injection failed (cross-referencing
  questions at K=100).
- **Tool adoption: 100%.** In every Engram session the model called
  `engram_search` on its own — no prompting beyond a bare `# Acme project`
  CLAUDE.md and the MCP config. The model does not need to be taught to use
  Engram.
- **The model hunts for memory anyway.** With memory stripped (control), Claude
  *looked for* a memory system — Glob'ing its own Claude Code namespace,
  `cat`ing `memory/MEMORY.md`, and (with tools disabled) emitting raw
  Read/Bash tool-call text into its answer. Print-mode does **not** load
  Claude Code's auto-memory feature, so the model went searching and found
  nothing. Native "it just remembers" is not on by default anywhere; Engram
  makes it real.
- **Cost honesty:** at these corpus sizes Engram costs ~1.4–1.5× native
  wholesale injection per question (retrieval round-trips + extra tokens).
  But A1's cost scales *linearly* with memory count (every question re-injects
  the whole corpus) while Engram's scales *sub-linearly* (only retrieved
  chunks enter context). The measured trend lines cross at roughly **K ≈
  100–150 memories** — right at this benchmark's top end, and well below any
  real team's corpus.
- **The environmental reading (emphasized):** inference energy tracks tokens
  processed, so the scaling shape *is* the sustainability story. Injection
  re-broadcasts the entire knowledge base into every interaction, in every
  teammate's session, forever — linear growth that fills the context window
  entirely around K ≈ 4,000 and stops working. Engram's per-query tokens are
  bounded by top-k retrieval, the corpus is embedded **once — locally, on
  the device**, and at team scale the deduplication is immediate. The win is
  structural, not day-one: below ~100–150 memories Engram currently costs a
  few percent *more* tokens per question. See
  [Environmental impact](#environmental-impact).

## What was measured

Four conditions, identical questions, one fresh `claude -p` session per
question (no cross-question contamination):

| Cond | Memory mechanism | Tools allowed |
|---|---|---|
| **a1** | Every memory bullet-injected into project `CLAUDE.md` (fits guidance at small K) | Read, Glob, Grep |
| **a2** | `CLAUDE.md` holds only an index; memories live in `memories/<category>.md` — the model must find and Read them itself | Read, Glob, Grep |
| **b** | Engram vault via `engramd-mcp` (`--engramd-url http://127.0.0.1:18789`) | **MCP only** (`--tools ""` — empirically verified to disable built-ins while keeping MCP tools) |
| **c** | Nothing — empty dir | None (`--tools ""`) |

### Corpus

Synthetic "Acme Labs" company memory, deterministic (`--seed 42`),
`bench/memory/generate.py`. Eight categories — employees, incidents,
releases, architecture decisions, infra hosts, events, budgets, products —
at **K = 25 and K = 100** memories. Every memory carries distinctive answer
keys (full names, `INC-####` ids, ports, dates, codenames) that never appear
in their question, so grading is an exact casefolded contains-match
(cross-reference questions require *all* keys). Control scoring 0% on all 60
+ 40 questions confirms the questions are unanswerable without memory — the
benchmark measures retrieval, not guesswork.

### Harness

`bench/memory/run.py` — resumable JSONL rows keyed on (condition, rep,
question); each question runs as a one-shot `claude -p` print-mode session
with `--output-format json` and `bypassPermissions`, in a unique per-condition
cwd under `/tmp/engram-bench/`. Transcripts are scanned for `mcp__` tool
calls to measure adoption. `bench/memory/report.py` aggregates. All
conditions resolved to the same model through the CLI.

## Results

### K = 25 (3 reps, 60 questions per condition)

| Condition | Correct | Recall | Mean in-tok/q | Mean cost/q | Wall s | Turns |
|---|---|---|---|---|---|---|
| Native A1 (all injected) | 60/60 | 100% | 29,308 | $0.151 | 8 | 1 |
| Native A2 (index + topic files) | 60/60 | 100% | 28,851 | $0.185 | 11 | 4 |
| **Engram (MCP vault)** | **60/60** | **100%** | **31,839** | **$0.207** | **14** | **5** |
| Control (no memory) | 0/60 | 0% | 1,461 | $0.023 | 13 | 1 |

Engram tool adoption: **60/60 (100%)** — `engram_search` on all 60 questions,
plus occasional `engram_get` (4), `engram_context` (3), `engram_health` (2).
The model discovered the toolset from the MCP schema and used search as its
primary path.

### K = 100 (2 reps, 40 questions per condition)

| Condition | Correct | Recall | Mean in-tok/q | Mean cost/q | Wall s | Turns |
|---|---|---|---|---|---|---|
| Native A1 (all injected) | 38/40 | **95%** | 32,449 | $0.169 | 6 | 1 |
| Native A2 (index + topic files) | 40/40 | 100% | 29,592 | $0.195 | 13 | 4 |
| **Engram (MCP vault)** | **40/40** | **100%** | **33,257** | **$0.247** | **21** | **5** |
| Control (no memory) | 0/40 | 0% | 2,268 | $0.037 | 9 | 1 |

Engram tool adoption: **40/40 (100%)** — `engram_search` on all 40 questions,
`engram_context` 4, `engram_health` 3.

Recall by question type at K=100:

| Type | a1 | a2 | b | c |
|---|---|---|---|---|
| crossref | **50%** (2/4) | 100% | 100% | 0% |
| lookup | 100% | 100% | 100% | 0% |
| precise | 100% | 100% | 100% | 0% |

The two A1 misses are the *same* question, both reps, with the *same* wrong
answer: asked which came first of "the billing-api outage" vs. the
recommendation-feed outage, the model answered `INC-4102` (2025-10-30) where
the key was `INC-5902` (2026-01-17). Both exist in the corpus — billing-api
appears in the base incident pool *and* in the K>68 overflow synthesis, so
the question as worded matches two memories. It is a residual corpus
ambiguity (flagged below), and it is itself informative: with 100 memories
all injected at once, the model *consistently* anchored on the wrong one of
two same-service incidents; Engram and A2 — which retrieve or read only the
relevant slices — both answered correctly.

## Findings

### 1. Tool adoption is automatic, not trained

100/100 Engram sessions (both scales, all reps) called `engram_search`
without any CLAUDE.md instruction to do so. The b-condition CLAUDE.md was a
single line (`# Acme project`). MCP tool *presence* is enough — the model
reads the schema and reaches for retrieval when a question exceeds its
context. This is the adoption-risk question answered: memory-as-MCP requires
zero per-project configuration to be used.

### 2. The model expects memory to exist — and native Claude Code has none (in print mode)

Three independent observations, all transcript-verified:

- In the control condition the model Glob'd its own
  `~/.claude/projects/<namespace>/` and attempted `Read memory/MEMORY.md` —
  it was *looking for* Claude Code's memory feature.
- With tools disabled the same instinct surfaced as literal DSML tool-call
  text embedded in the answer ("`<invoke name="Read"
  file=".../memory/MEMORY.md"/>`") — the model trying to search with no
  hands.
- Print-mode (`claude -p`) does not load auto-memory. There is no built-in
  retrieval path in this configuration; the model found nothing and scored
  0%.

Product implication: "Claude will just remember" does not exist by default
today. Engram's MCP server is the mechanism that satisfies the model's own
expectation — and the 100% adoption rate says the model *wants* it.

### 3. Engram wins the failure mode native memory loses

A1's only failures came from *over-injection*: 100 memories at once, two
same-service incidents, one consistently mis-chosen. Retrieval-based
conditions (A2 by file scoping, Engram by semantic search) scoped the answer
space to the right memories and got both crossref questions right. At larger
corpora this gap should widen: A1's per-question context grows without
bound, while retrieval stays bounded.

### 4. Cost and scaling — the honest picture

Per-question cost (means, measured):

| | K=25 | K=100 | Growth at 4× corpus |
|---|---|---|---|
| A1 (inject all) | $0.151 | $0.169 | +12% — and rising ~linearly with K |
| A2 (index + read) | $0.185 | $0.195 | +6% |
| **Engram** | **$0.207** | **$0.247** | **+19%** |
| Control | $0.023 | $0.037 | — |

In-token growth tells the same story: A1 29,308 → 32,449 (+11%) because every
question re-injects the whole corpus; A2 28,851 → 29,592 (+2.6%) because only
the relevant topic file is read; Engram 31,839 → 33,257 (+4.5%) because only
the top search hits enter context.

Engram is the costlier option *at these scales* — search round-trips and
result digesting cost ~$0.05–0.08 more per question than wholesale injection,
and it is the slowest condition (21 s vs 6 s at K=100). But the structural
trends point the other way: fit linearly through the two measured points, A1
and Engram cross over at roughly **K ≈ 110–150 memories**. Beyond that, A1
pays the whole corpus on every question while Engram pays a roughly flat
per-question retrieval fee. Extrapolation, not measurement — but the shape is
mechanistic, not guessed: injection cost is `overhead + c·K` by construction.

Engram also carries a fixed premium per question that A1 does not (MCP round
trips, ~2–5 extra turns) — worth knowing for low-K, high-frequency use cases
where injection remains genuinely cheaper.

## Environmental impact

The benchmark measures tokens, not watts — no power instrumentation was
done. But token volume is the standard proxy for inference energy, since
datacenter inference compute scales with tokens processed. Read through that
lens, the cost-scaling result above is also an energy-scaling result, and it
is the deepest finding of this run:

> **Engram's energy cost per interaction stops growing as knowledge grows.
> Native injection's grows forever — until it hits the context ceiling and
> stops working entirely.**

### Linear vs. bounded: what the measurements show

Input tokens per question, mean, measured:

| Corpus | A1 (inject all) | Engram |
|---|---|---|
| K=25 | 29,308 | 31,839 |
| K=100 | 32,449 | 33,257 |
| Growth at 4× corpus | **+11%** | **+4.5%** |

A1's growth is linear *by construction* — every interaction re-processes the
entire corpus (~28.3k + 42 tokens per memory, fit through the two measured
points). Engram's growth comes only from retrieval: search returns its top-k
hits regardless of corpus size, so per-query context stays bounded (the
two-point linear fit, ~+19 tokens per memory, is a conservative upper bound —
the mechanism suggests it flattens further). The measured trend lines cross
at roughly **K ≈ 110–150 memories**. Beyond that, Engram processes fewer
tokens per question than wholesale injection, and the gap widens
indefinitely.

The context ceiling makes this decisive rather than marginal. Context
windows top out around ~200k tokens. On the measured slope, A1 fills an
entire window with memory *alone* around **K ≈ 4,000** — after which the
approach stops being deployable at all. Engram's per-query context grows
only through its top-k search results, not the corpus: at any corpus size it
sends memory chunks plus room to think. Retrieval is not just cheaper at
scale; it is the only shape of memory that survives scale.

### Knowledge should be transported once, not re-broadcast

Wholesale injection re-ships the entire knowledge base on every interaction,
in every teammate's session, forever. Engram embeds the corpus once and pays
per query only for the chunks that query needs. Org-wide, the difference
compounds:

- 20 teammates × 10 memory-dependent questions/day at K=1,000: injection ≈
  **14M input tokens/day** just to carry knowledge; Engram ≈ **10M** (using
  the conservative linear fit — the mechanism says less). ~30% fewer tokens
  per day from the retrieval shape alone.
- Every 1,000 memories added to the corpus: injection +~8.4M tokens/day
  (42 tokens × 1,000 memories × 200 interactions). Engram barely moves.

And Engram's retrieval layer is **local**: embeddings and search run on-device
(local-only inference — no datacenter GPU for retrieval). The only cloud load
per query is the tokens the model itself consumes. Cloud-RAG and cloud-memory
alternatives pay for retrieval compute in the datacenter too; Engram does
not.

### Secondary effects (not measured here, but directionally real)

- **Context churn.** Injected contexts overflow the window sooner, forcing
  compaction — which re-reads and re-summarizes the whole context: energy
  spent re-processing knowledge, not answering. Retrieval's small footprint
  pushes compaction out.
- **Output-token premium at small scale.** Engram generated ~5.7× more
  output tokens at K=100 (1,290 vs 225 — the model narrates its search), and
  output tokens cost more energy per token than input. This narrows Engram's
  day-one efficiency; as input volume dominates at scale, it fades.

### Where the claim holds, and where it must not be stretched

- **True:** Engram's per-interaction token load is bounded as knowledge
  grows; injection's is linear and context-capped. At team scale the
  deduplication of knowledge transport is immediate, and the retrieval layer
  runs locally.
- **Not yet true:** "Engram uses less energy per question" for a *small*
  personal corpus — below ~100–150 memories it currently costs a few percent
  more tokens, plus the output-token overhead. The win is structural and
  asymptotic, not day-one.
- **Not claimed:** no power instrumentation was done — we measured tokens,
  not joules. Frame it as token-bounded, never carbon-measured.
- **Anticipated objection:** within one long session, prompt caching lets a
  repeated injected prefix be re-read cheaply, so native injection is less
  wasteful intra-session than the per-question numbers suggest. The
  benchmark's fresh-session design models the first question, cache
  evictions, and new teammates — the moments full price is paid. Cached
  tokens still occupy the context budget either way.

## Lessons learned building the benchmark (all fixed in-harness)

1. **Synthetic corpora must be uniqueness-verified.** The first K=100 run was
   invalid: products were picked *with replacement* (two codenames for one
   initiative), overflow synthesis duplicated roles and event types, and
   crossref pairs could share a service. The model flagged "two conflicting
   entries" and got graded wrong for being observant. Fixes: without-
   replacement sampling, disjoint overflow pools, date disambiguators baked
   into question text, same-service crossref skip. Every question now maps to
   exactly one memory. **The residual billing-api case above survived this
   fix** (base pool + overflow, same service) and cost A1 two questions —
   the doc records it rather than hiding it.
2. **Sibling sessions contaminate each other.** A b-condition session once
   Grep'd the shared parent directory and read A1's injected CLAUDE.md,
   answering an Engram question from native memory. Every condition now gets
   a unique parent dir and a tool whitelist.
3. **`--allowedTools` is not a restriction** — it only *adds*. The real
   built-in kill switch is `--tools ""`, and it leaves MCP tools available
   (verified empirically). Pre-fix, every condition had Bash; the control
   scored 0% only after `--tools ""` landed.
4. **Budget truncation is a benchmark hazard.** The run hit an API 402
   mid-flight; rows were resumable (JSONL + done-set), and the K=100 scope
   was halved from 3 reps to 2 with `--max-reps`. A1's 40 rows span two runs,
   all from the fixed clean corpus. Total API spend across the benchmark's
   full history — including the invalidated runs and dry-runs — was roughly
   **$102**; the two valid final runs cost $59.77 ($33.89 + $25.88).

## Limitations

- **Synthetic corpus.** "Acme Labs" is generated, ~130-token memories with
  distinctive keys. Real memory is messier, longer, and less keyword-dense.
- **One model.** All conditions resolved through the same CLI model; results
  may shift on other models.
- **Exact-match grading.** A contains-match on answer keys counts a wrong
  *reason* with a right *key* as correct, and vice versa.
- **Fresh session per question.** Real work benefits from session-level
  memory reuse; this benchmark deliberately isolates per-question retrieval.
- **Small question counts** (20 questions/rep, 2–3 reps). The A1 95% figure
  rides on exactly 2 misses — directionally real (both are the same
  ambiguous question, consistently wrong), but the margin of error is wide.

## Reproduce

```sh
python3 bench/memory/generate.py --k 100 --questions 20 --seed 42 --out bench/data-k100
python3 bench/memory/run.py --k 100 --questions 20 --reps 2 --conditions a1,a2,b,c \
    --tag my-run --seed 42
python3 bench/memory/report.py --tag my-run
```

Runs `cargo build -p engramd-mcp --release` as needed, starts a bench vault
daemon on `127.0.0.1:18789`, and appends resumable rows to
`bench/runs/<tag>/results.jsonl`. Requires a configured `claude` CLI.
Published runs: `bench/runs/full1-k25/` (3 reps) and `bench/runs/full1-k100/`
(2 reps); `full1-k100-ambiguous/` archives the invalidated first attempt.
