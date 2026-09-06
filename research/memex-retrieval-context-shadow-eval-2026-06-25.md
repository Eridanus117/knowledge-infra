---
description: "2026-06-25 memex ingestion-time retrieval_context shadow eval: GPT 生成上下文有弱正信号但不足以上生产"
keywords: [memex, retrieval-context, contextualized-content, contextual-retrieval, goldset, rag, bm25, semantic, shadow-eval, gpt]
kind: research
code: ["fixtures/users/anonymous/workspace/sources/personal/orrery/memex/src/memex/indexing/compile.py", "fixtures/users/anonymous/workspace/sources/personal/orrery/memex/src/memex/compiled.py", "fixtures/users/anonymous/workspace/sources/personal/orrery/memex/src/memex/lexical.py"]
---

# Memex Retrieval Context Shadow Eval 2026-06-25

## Conclusion

Do not add production `contextualized_content` / `retrieval_context` yet.

The local shadow tests show a weak positive signal for GPT-generated ingestion-time retrieval context, mainly small MRR/rank improvements. The signal is not strong enough for a production schema/index change: it did not improve `gold@10`, did not fix no-hit cases, and introduced small regressions in some slices.

Query-time `--preview` remains a display aid, not a retrieval-quality feature. Keep default `crux recall` pointer-first; only use preview when a human or answer-synthesis path needs snippets.

## Current Memex State

As of 2026-06-25, memex does not have a `contextualized_content` field.

Current compiled/index path:

- C4 compiled doc schema stores `body_text`, not contextualized text.
- Embedding text is `description + keywords + body_text`.
- Lexical `Doc.body` is also `description + keywords + body_text`.
- `--preview` is filled after retrieval from doc body and does not affect ranking.

This means memex already has a weak human-authored retrieval context layer through `description` and `keywords`, but not Anthropic-style generated per-document retrieval context.

## Tests

All tests were read-only shadow evals. No source note, compiled artifact, Qdrant collection, or production index was changed. Temporary prompts/results lived under `/tmp` and are not durable evidence.

### Deterministic Metadata Context

Synthetic context: `repo/domain/kind/title/path/keywords` was prepended to body for lexical BM25.

Full goldset lexical, 474 queries:

| Metric | Baseline | Deterministic context |
|---|---:|---:|
| gold@5 | 0.8713 | 0.8734 |
| gold@10 | 0.9599 | 0.9599 |
| MRR | 0.7001 | 0.6978 |
| no_hit | 19 | 19 |

Diff: 4 improved, 5 regressed, 0 fixed no-hit.

Verdict: not useful enough; metadata-only context mostly adds noise.

### GPT Retrieval Context: source-delta

Generated GPT retrieval context for all 26 `source-delta` docs, then prepended it to body in a temporary lexical corpus. Evaluated 70 `source-delta` gold queries.

| Metric | Baseline | GPT context |
|---|---:|---:|
| gold@3 | 0.7714 | 0.7857 |
| gold@5 | 0.8714 | 0.8714 |
| gold@10 | 0.9714 | 0.9714 |
| MRR | 0.6363 | 0.6472 |
| no_hit | 2 | 2 |

Diff: 4 improved, 0 regressed, 0 fixed no-hit.

A semantic hard-subset cosine test over 9 hard queries was mixed:

| Metric | Semantic base | Semantic + GPT context |
|---|---:|---:|
| gold@5 | 0.6667 | 0.5556 |
| gold@10 | 0.7778 | 0.7778 |
| MRR | 0.3222 | 0.3370 |
| no_hit | 2 | 2 |

Diff: 1 improved, 1 regressed. Larger embedding batches hit HTTP 504 from the embedding gateway, so semantic results are only directional.

Generation cost: about 55k Codex/GPT tokens for 26 docs.

### GPT Retrieval Context: source-beta

Generated GPT retrieval context for all 17 `source-beta` docs. Evaluated 52 `source-beta` gold queries.

| Metric | Baseline | GPT context |
|---|---:|---:|
| gold@3 | 0.8846 | 0.8846 |
| gold@5 | 0.9231 | 0.9231 |
| gold@10 | 1.0000 | 1.0000 |
| MRR | 0.7978 | 0.8000 |
| no_hit | 0 | 0 |

Diff: 1 improved, 1 regressed.

### GPT Retrieval Context: source-alpha Hard Subset

Generated GPT retrieval context for 38 `source-alpha` candidate docs selected from 6 hard source-alpha queries: each query's gold doc plus current top-10 confuser docs. Evaluation still used the full 125-doc `source-alpha` lexical competition set.

All source-alpha gold queries, 82 queries:

| Metric | Baseline | GPT context on 38 docs |
|---|---:|---:|
| gold@3 | 0.6829 | 0.6707 |
| gold@5 | 0.7561 | 0.7561 |
| gold@10 | 0.9146 | 0.9146 |
| MRR | 0.5852 | 0.5864 |
| no_hit | 7 | 7 |

Diff: 2 improved, 1 regressed, 0 fixed no-hit.

Selected hard6 only:

| Metric | Baseline | GPT context on 38 docs |
|---|---:|---:|
| gold@3 | 0.0000 | 0.0000 |
| gold@5 | 0.0000 | 0.0000 |
| gold@10 | 0.8333 | 0.8333 |
| MRR | 0.1111 | 0.1141 |
| no_hit | 1 | 1 |

Diff: 1 improved, 0 regressed.

Generation cost: about 80k Codex/GPT tokens for 55 docs in the second small run.

## Interpretation

GPT-generated retrieval context is not useless: it consistently nudged a few ranks upward and produced small MRR gains in multiple slices. However, the observed benefit is too small to justify a production schema/index change now.

The main failure mode remains harder than missing generic context: no-hit cases were not fixed, and `gold@10` did not move. For current memex notes, hand-authored `description` and `keywords` already provide much of the cheap context that contextual retrieval would otherwise add.

## Recommended Gate Before Any Implementation

If this is revisited, do not write generated context into source Markdown. Treat it as a derived compiled/index artifact, for example:

- `retrieval_context`
- `retrieval_context_model`
- `retrieval_context_hash`
- `retrieval_context_source_hash` or compiled-hash binding

Only generate incrementally by `compiled_hash`, and prefer hard/miss documents first.

Promotion gate should include:

- full or representative goldset shadow run;
- lexical, semantic, and hybrid metrics;
- `gold@10`, `gold@5`, MRR, and no-hit fixed count;
- per-slice regression checks for `zh_low_anchor`, `lexical_dependent`, and repo slices;
- cost ceiling for generation and re-embedding;
- no production flip unless improvements are larger than noise and regressions are zero or explicitly accepted.

Current status: keep as a candidate research direction, not a planned production change.
