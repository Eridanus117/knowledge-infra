---
description: 公开、合成 fixture 专用的双语 hybrid 检索设计：区分候选缺失与融合降权，并以显式身份注册表约束评估。
keywords: [hybrid-retrieval, bilingual, CJK, candidate-depth, weighted-RRF, synthetic-fixture]
links: []
kind: research
---

# 双语 Hybrid 检索：候选池与融合是两个独立问题

> **公开替代文档。** 本文只使用合成 fixture 和匿名身份 `source-alpha`、`source-beta`、`source-gamma`、`source-delta`。不包含真实查询、文档正文、来源路径、索引名称、运行实例或私有评测输入。

## 结论

低锚点的 CJK 查询在跨语种检索中可能同时遇到两种失败：相关对象没有进入 semantic 候选池，或对象已经进入候选池却在融合后落到结果窗口之外。两种失败必须分别观测、分别修复；只提高候选深度或只改融合权重都不足以证明问题已解决。

推荐顺序是：先确认 semantic lane 在更深的候选窗口能看见相关匿名对象，再在隔离的 opt-in profile 中比较等权和逐级提高 semantic 权重的 RRF。选择满足公开评估门槛的最低增益档；若仍不能满足门槛，转向 reranking 质量，而不是无限提高权重。

## 合成 fixture 边界

评估只允许引用 fixture manifest 中声明的匿名身份。示例 fixture 只描述输入形状和预期身份，不保存查询文本或对象正文：

| fixture ID | 输入形状 | 预期相关身份 | 目的 |
|---|---|---|---|
| `fixture-cjk-no-anchor` | 仅含 CJK 字符、没有 ASCII 标识符 | `source-alpha:object-01`、`source-beta:object-02` | 暴露跨语种的低锚点召回问题 |
| `fixture-mixed-anchor` | CJK 字符与至少一个 ASCII 标识符并存 | `source-gamma:object-01` | 验证混合查询不被过度放大 |
| `fixture-ascii-anchor` | 不含 CJK 字符，含 ASCII 标识符 | `source-delta:object-01` | 保护已有 ASCII 查询表现 |
| `fixture-unclassified` | 不满足前三种形状 | `source-alpha:object-03` | 验证安全的常规预算回退 |

这些标签是测试构造，不表示真实来源、路径或文档内容。

## 先区分失败位置

每次 profile 评估都应记录两层结果：

1. **候选可见性**：每个预期身份是否进入 lexical lane 或 semantic lane 的候选池，以及各 lane 中的 rank。
2. **最终可见性**：该身份在融合和截断后的最终 rank 是否仍在请求窗口内。

若对象不在任何候选池，融合公式没有可操作的输入；应调节 candidate budget、索引覆盖或 semantic 质量。若对象已经在 semantic lane 中且最终被压出窗口，才是融合策略问题。报告不得只给最终分数而省略第一层证据。

## 确定性语言提示

分类器只读取查询字符特征，绝不改写、翻译或注入别名。令：

- `cjk_char_count` 为 CJK 字符数；
- `ascii_identifier_count` 为匹配 `[A-Za-z_][A-Za-z0-9_:-]*` 的标识符数。

| `query_language_hint` | 判定 | semantic budget |
|---|---|---|
| `cjk_only_low_anchor` | `cjk_char_count > 0` 且 `ascii_identifier_count == 0` | 使用经公开 fixture 校准的较深预算上限 |
| `mixed` | 两个计数都大于零 | 使用常规预算 |
| `ascii_only` | CJK 计数为零且标识符计数大于零 | 使用常规预算 |
| `unknown` | 两个计数都为零 | 使用常规预算 |

`scope_filter` 是正交条件，不是语言类别；它可以增加最低候选预算，但必须仍受同一上限保护。分类结果应进入 trace，且分类枚举值、profile 标识和 fixture 名称各自采用稳定、受限的命名规则。

## Candidate budget 策略

以请求窗口 `limit`、倍数 `m`、最低预算 `floor` 和 profile 上限 `cap` 表示常规策略：

```text
base_budget = max(limit * m, floor)
semantic_budget = min(cap, adjusted_budget)
```

低锚点类别可以直接选择 `cap`；其余类别从 `base_budget` 开始。具体数值必须由公开合成 fixture 的延迟和排序门槛确定，不能从私有默认配置、私有评测分数或本机配置文件隐式读取。

预算调整后仍要保留延迟保护：记录每条 lane 的耗时、候选数和截断位置；当 profile 的预算或尾延迟越过批准边界时，拒绝把它设为默认值。

## 加权 RRF 融合

对文档 `d`，可用下式表达两条 lane 的融合：

```text
score(d) = lexical_weight / (k + lexical_rank(d))
         + semantic_weight / (k + semantic_rank(d))
```

缺失的 lane 不贡献项。评估至少包含：

- 等权 cell，作为可复现基线；
- 两个 semantic 权重逐级增强的 cell；
- 对低锚点类别的 opt-in 限定，不改变默认 profile；
- 对 mixed 和 ASCII fixture 的回归检查。

选择最低通过门槛的 semantic 权重。若最强预先批准的 cell 仍失败，应检查 reranker、语义模型或判断集质量；继续叠加权重会掩盖候选或权威性问题。

## 可审计 trace

每次检索至少产出以下不含正文的字段：

```text
query_language_hint
query_language_features.{cjk_char_count, ascii_identifier_count}
semantic_budget_policy
semantic_budget_reason
semantic_budget_cap
fusion_algorithm
fusion_policy
fusion_weights
per_lane_stats.<lane>.{candidate_count, elapsed_ms}
```

trace 可以包含匿名对象身份和 rank，不能包含原始查询、对象正文、来源路径、连接信息或运行实例标识。

## 公开评估契约

公开 evaluator 必须显式接收 fixture manifest 和身份注册表；不得默认查找私有目录，也不得把既存私有得分当作 baseline。缺少输入、读取失败、未注册身份或 fixture 与注册表的 identity mismatch 都必须在计分前失败并返回非零状态。

评分应只比较匿名身份、排序和公开定义的阈值。非主要命中可作为诊断信息出现，但不得被悄悄加入 expected set 来掩盖主要对象缺失。

## 非目标

- 查询改写、翻译、生成式扩写或按 fixture 硬编码别名；
- 仅为语言差异复制一套索引；
- 在公开评估通过前替换默认检索 profile；
- 将任何历史分数、真实 qrels、查询文本或语料内容带入本文件。
