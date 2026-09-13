---
description: 公开、合成 fixture 专用的语义同步批量清理诊断：识别 ID 模式切换残留，并在显式批准后执行大规模 prune。
keywords: [semantic-sync, mass-prune, id-scheme, chunking, orphan-detection, synthetic-fixture]
links: []
kind: research
---

# 语义同步中的批量清理：先确认 ID 模式切换，再处理 orphan

> **公开替代文档。** 本文仅使用匿名身份 `source-alpha`、`source-beta`、`source-gamma`、`source-delta` 与合成 ID 形状。没有真实集合名、来源路径、环境变量、端点、运行实例或对象正文。

## 结论

当同步器拒绝一次大比例 prune 时，不能立刻把原因归为“范围配置错误”。常见的另一种原因是：同一个目标索引曾用两种 unit-ID 模式写入，当前同步只按其中一种模式计算 expected IDs，于是把另一种模式留下的点全部视为 orphan。

大比例 prune guard 仍然是正确的安全边界；问题在于诊断信息必须区分“真正的范围错误”和“ID 模式混合”。只有模式、身份清单和待删 ID 集合都被证明一致后，才能有条件地放行一次大清理。

## 两种合成 ID 形状

本文把模式称为 `whole` 与 `chunked`：

| 模式 | 单位 | 合成 ID 形状 |
|---|---|---|
| `whole` | 每个源对象一个向量单位 | `source-gamma:object-01` |
| `chunked` | 每个源对象的多个片段各一个向量单位 | `derive(source-gamma:object-01, chunk-index)` |

`derive(...)` 仅表示稳定、可重复的派生函数；它不绑定任何实际算法、命名空间或底层服务。两种形状可以各自正确，但一个目标索引在任一时刻必须有一个唯一、显式记录的 ID 模式。

## 失败机理

1. 某次写入按 `chunked` 模式产生多个派生 ID。
2. 后续同步按 `whole` 模式从当前身份注册表计算 expected IDs。
3. 这些派生 ID 与裸对象 ID 不匹配，因此被归为 orphan。
4. orphan 比例超过 guard，清理被拒绝；新对象也可能因同步整体中止而迟迟不可见。

反方向的模式切换同样会发生。只调高 prune 阈值会把模式不一致伪装成可接受的删除风险。

## 最小诊断流程

诊断不需要显示对象正文，也不应调用或记录任何运行端点。

1. **冻结写入者。** 暂停自动与人工同步，避免观察过程中继续混入新模式。
2. **读取 manifest。** 检查目标索引记录的 `unit_id_scheme`、生成代号、匿名 source identity 集合和最后一次成功同步的模式。
3. **抽样元数据。** 从受限样本读取 point ID、匿名 `source_identity` 与 `unit_index`；不读取正文或来源路径。裸身份形状与派生形状并存即是混合模式证据。
4. **比较有效配置。** 对每个可能的写入入口导出已解析的 `unit_id_scheme`。任何入口不一致都是根因候选，不能只检查其中一个启动方式。
5. **重算 expected IDs。** 用 manifest 中同一份匿名注册表和同一模式计算；将结果与 observed ID 形状对比。

诊断记录应包括 `configured_id_scheme`、`observed_id_scheme_counts`、`expected_count`、`orphan_count`、`manifest_generation` 和 `identity_registry_digest`，但不包含真实 identity 值、路径或正文。

## 修复顺序

1. **统一模式来源。** 所有写入入口必须从同一个受控配置解析 `unit_id_scheme`；将有效模式写入 manifest，禁止依赖漂浮的进程环境。
2. **选择恢复方式。** 若 manifest 和匿名对象清单可信，生成只包含 inactive-mode ID 的删除计划；若不可证明清单完整，删除并从可信输入重建整个目标索引更安全。
3. **执行受控清理。** 大比例 prune 必须要求一次性显式批准，并在执行前持久化 manifest 快照、目标模式、删除 ID 摘要和理由。该批准不得成为永久默认开关。
4. **正常同步。** 清理后用同一模式运行一次完整同步，并确认 observed ID 形状只剩已批准模式。

禁止按“看起来像旧点”的内容猜测删除，也禁止因为 guard 阻塞就把任何模式混合直接全量放行。

## 防止再次发生

- 把 `unit_id_scheme` 作为索引的持久化契约，而不是每次运行的临时选项；
- 在首次写入前拒绝“已有索引但没有 manifest”的情况，除非显式走迁移流程；
- 每次同步在写入前比较配置模式与 manifest 模式，不一致即失败；
- 在预演中报告候选删除比例和两种 ID 形状的计数，超过 guard 时显示下一步诊断而不是模糊报错；
- 对模式迁移使用新索引代际或明确的重建步骤，避免两种 ID 算法混写。

## 公开 fixture 与 evaluator

可用一个只含 `source-gamma:object-01` 和 `source-delta:object-01` 的合成 manifest 演练 whole/chunked 切换。fixture 仅保存匿名 ID 形状与期望状态，不含文档内容或真实存储资料。

公开 evaluator 必须显式提供 fixture manifest 与身份注册表；输入缺失、digest 不匹配、未注册 identity 或 manifest 声明与观察模式矛盾时必须 fail loud，并在尝试 prune 前退出。它不得读取私有默认位置，也不得以任何私有 score 或历史运行结果判定成功。

## 非目标

- 通过提高大 prune 阈值来掩盖模式不一致；
- 依赖调用入口、进程环境或人工记忆来决定 ID 模式；
- 输出或复用真实对象、集合、来源路径、服务连接或历史运行日志。
