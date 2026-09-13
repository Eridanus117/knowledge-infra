---
description: 公开、合成 fixture 专用的 CJK 分词选型：优先使用宿主全文引擎的原生搜索分词器，并保护精确标识字段。
keywords: [CJK-tokenizer, lexical-retrieval, analyzer-selection, synthetic-fixture, exact-identifier]
links: []
kind: research
---

# CJK 分词选型：先选原生搜索分析器，再用公开 fixture 证明收益

> **公开替代文档。** 本文不保留真实查询、文档正文、来源路径、依赖锁定版本、运行环境或私有评测分数。`source-alpha`、`source-beta`、`source-gamma`、`source-delta` 均为匿名合成身份。

## 结论

当全文引擎的默认分析器主要面向空格分词语言时，CJK 文本会因切分不足而失去 lexical recall。首选方案是宿主全文引擎支持的**原生 CJK 搜索分词器**：它拥有最小的集成面、可复用的位置与词频语义，并避免维护自定义适配层。

选择并不等于把所有字段都送去分词。自然语言字段使用 CJK 搜索分析器；身份、对象键和范围字段保持精确匹配。混合查询的评估必须与纯 CJK 查询一起运行，避免为一个类别修复问题而破坏标识符检索。

## 公开 fixture 契约

fixture 只保留输入形状和匿名预期身份，不保存真实文本：

| fixture ID | 输入形状 | 预期相关身份 | 证明的边界 |
|---|---|---|---|
| `fixture-cjk-compound` | 无空格 CJK 词组形状 | `source-alpha:object-01` | 自然语言字段获得可检索 token |
| `fixture-cjk-with-identifier` | CJK 字符与 ASCII 标识符并存 | `source-beta:object-01` | 分词与精确标识符共存 |
| `fixture-ascii-identifier` | 仅 ASCII 标识符 | `source-gamma:object-01` | 原有精确检索不退化 |
| `fixture-scope-exact` | 精确范围/对象键条件 | `source-delta:object-01` | 不把结构化字段误当自然语言 |

manifest 必须明确列出每个匿名身份。缺少 manifest、身份未注册或期望身份与注册表不一致时，公开 evaluator 必须在计分前失败，而不是回退到任何私有输入或历史分数。

## 三类候选

| 候选 | 优点 | 风险 | 采用条件 |
|---|---|---|---|
| **宿主引擎原生 CJK 搜索分词器** | 集成面小；查询与索引语义一致；通常保留词频和位置 | 可能需要受控升级全文引擎 | 首选，前提是公开 fixture 满足门槛 |
| 字典导向的外部分词器 | 专名和词典可调 | 依赖、初始化与运维面更大 | 原生方案在公开 fixture 上不能满足边界时再比较 |
| 本地维护的适配器 | 可避免升级 | 长期要维护 tokenizer 协议与兼容性 | 仅在升级不可行且有明确维护所有者时使用 |

不要因为短期升级阻力直接选择本地适配器；它把一次性的兼容成本变成持续的协议维护成本。也不要把外部词典当作默认更好：它必须通过同一套公开 fixture 和运行约束。

## 字段与查询配置

1. **自然语言字段**：标题和正文使用选定的 CJK 搜索分析器，并保留词频与位置信息，以支持短语、近邻或排序功能。
2. **结构化字段**：匿名身份、对象键和范围字段使用 `raw`/exact 分析器；它们不经 CJK 切分。
3. **查询解析**：自然语言字段可有相对 boost，精确字段的 boost 保持单独可配置。所有权重从公开 fixture 校准，不写死私有评分结论。
4. **歧义切分**：先以保守设置上线候选 profile。是否启用未知词推断或词典扩展，必须是一次独立、可复现的公开 fixture 实验，而不是隐式开关。

这样可以把“自然语言召回”和“精确标识符命中”分开观测，避免分析器配置吞掉结构化 token。

## 接入与回退顺序

1. 建立新的 analyzer/profile 标识，保留当前 baseline 可复现。
2. 在公开 fixture 上记录 token 流、命中匿名身份、排序边界和耗时；token 流只能来自合成输入。
3. 若原生分词器通过约定门槛，采用它并完成受控升级。
4. 若升级阻塞或未通过，记录具体不通过的 fixture，再比较字典方案；不要用未说明的局部补丁掩盖结果。
5. 只有当两种受支持方案均不可用，才评估本地适配器，并同时明确版本兼容责任与退出条件。

回退必须回到之前可复现的 profile，而不是保留半升级、半适配的混合状态。

## 评估门槛

门槛应表达为公开可观察的行为，而不是绑定真实语料：

- `fixture-cjk-compound` 的预期匿名身份进入请求结果窗口；
- CJK 与 ASCII/mixed fixture 均不越过预先批准的回归界限；
- `fixture-scope-exact` 仍以精确字段命中；
- profile trace 能说明所用 analyzer、字段、token 数量和耗时，且不输出输入正文；
- 所有 fixture、注册表和阈值均显式传入 evaluator。

公开 evaluator 禁止在输入缺失时选择默认目录、默认语料或存档得分。任何 identity mismatch 都是配置错误，不是零分样本。

## 非目标

- 查询改写、翻译或语义扩写；
- 将对象键、身份或范围字段迁移成模糊自然语言匹配；
- 以私有 corpus 的 aggregate metric 替代公开 fixture 结果；
- 将真实词典、项目专名、私有文本或本机依赖快照复制到公开仓。
