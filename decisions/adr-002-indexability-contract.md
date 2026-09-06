---
description: 钉死"为什么 curated 知识检索不回来",补 ADR-001 没闭合的缝。真因是三层静默失败(全部 query-time + 读码 + --diagnose 实证):①写→索引:94 个 domain-map 里 74 个无 frontmatter 被静默 skip(门是 frontmatter 有无、不是路径);②索引→召回:连已索引的 20 个,其最高分语义结果被 eligibility gate 以 compiled_hash_mismatch 静默丢弃 32 条(qdrant 向量 vs 当前 artifact 不同步,binary dirty+behind);③诊断本身说谎:doctor 报 semantic configured=false 实为 root-agnostic 探针误报(语义其实通)。架构没坏(ADR-001 成立),坏的是栈到处静默失败 + 诊断误导,导致本会话连环误诊 4 次 + 历史 issue-example。决定:数据补 v3 frontmatter + 重建索引同步 hash + 让三层失败从静默变可见。
keywords: [检索静默失败, compiled_hash_mismatch, frontmatter门, domain-map不可索引, doctor探针误报, 索引新鲜度]
links: [adr-001-kb-consumption-layer]
supersedes: ~
kind: decision
---

# ADR-002:检索栈静默失败(可索引性 / 新鲜度 / 可观测性)

- **状态**:Proposed(待 principal 拍 D1 schema + accept;D2/D3 改核心待批)
- **日期**:2026-06-06
- **决策**:principal 拍板,agent 落实
- **关联**:refine [[adr-001-kb-consumption-layer]];勘误 issue-example(语义降级误诊)、记忆 `wx-recall-drops-domain-map`(已证伪)

## 背景(为什么要做)

主诉:"最值钱的 KB(domain-map)检索不回来"。诊断此事时连环误诊 **4 次**(详见过程教训),靠对抗 critic + query-time 实测 + 读检索核心代码 + `--diagnose` 才钉死。真相不是架构问题,是**栈在三个独立层各自静默失败,且诊断工具主动误导** —— 这恰恰解释了"为什么这库一直有问题却说不清"。

**实证(2026-06-06,全部亲手复核)**:

1. **写→索引:无 frontmatter 被静默 skip。门是 frontmatter 有无,不是路径。**
   `docs/source-notes/domain-map/` 共 **94** 个 `.md`:**20 个带 v3 frontmatter → 已索引、可语义召回**(实测命中 `example-domain-charge/topology.md` 等);**74 个无 frontmatter → 静默 skip**。铁证:同一目录、同一路径,带的进、不带的不进 → **决定可索引性的是 frontmatter,不是路径**。
   机制(读码):source-alpha 的 manifest **无 `repo_kind: kb`**,是代码仓,走 `scan_markdown`(扫描含 `docs/`,domain-map 全在 `notes_scanned=282` 内);无 frontmatter 文件在 `build_artifacts` 的 `parse_markdown_note` 处 `skip;continue`,**不报错、不阻塞、不提示**。doctor:`notes_scanned=282 skip=124(44%)`。
   推论:ADR-001 的 v5 kb-lane(path-gated `docs/kb/` + `is_kb_path`)**对本仓不生效**——那是给 `repo_kind: kb` 仓的。本仓的 lane 是老 source-note schema。

2. **索引→召回:已索引的好货,最高分语义结果被静默丢弃。**
   `legacy-retrieval search '示例领域计算 接口调用拓扑' --diagnose`:`dropped_candidate_traces: 32`。被丢的正是**最高分**语义候选——`semantic#1:0.731 diamond-switches`、`semantic#2:0.730 example-domain-template route-segments`、`semantic#4/#5 troubleshooting-sop`…全部 `rule=candidate_failed_eligibility_gate dropped_reason=compiled_hash_mismatch`。survive 到 final_rank 1-5 的反而是语义排名靠后(#3/#6/#7/#18/#23)但 lexical 强的。
   机制:qdrant 向量是旧 artifact 编译版本嵌入的,与当前编译产物 hash 对不上,eligibility gate 判定失配即丢。doctor 佐证:`binary 70c38a2bb599 (dirty) behind_build_scope=1`。**这是索引陈旧,不是配置、不是架构**——而且它连带 #1 已索引的 20 个一起伤。

3. **诊断本身说谎。**
   `doctor` 报 `semantic_runtime: configured=false / missing WEAVE_QWEN3_QDRANT_HYBRID_COLLECTION`,看着像"语义关了"。但代码(`legacy-retrieval-semantic-qwen3-qdrant/src/lib.rs:319-323`)证明 env 未设时**带 root 即按仓派生 collection**(`root_hybrid_collection`→`source-alpha_hybrid_qwen3_v0`,qdrant 内真实存在);报错只在 **root-agnostic** 探针路径触发,doctor 走的正是那条。query-time 实测:embedding server(:3002)在线,纯 paraphrase「接需求时怎么不漏掉风险点」正确命中目标 = **语义运行时是通的**。`configured=false` 是误报,**同款误报在 issue-example + 本会话各坑了一次**。

4. **召回路径分歧:`legacy-retrieval query`(wx recall 实际调的)比 `legacy-retrieval search` 差。**
   实测(2026-06-06)同 query「COD 子模板没有费率」:`legacy-retrieval search` 正确返回 `all-template-types.md` 等**已索引** domain-map;`legacy-retrieval query` / `wx recall` 返回无关 doc、domain-map 一个没有。即使 D1/D2 把内容补进索引、hash 同步好,**agent 走的 recall→query 路径本身仍劣于 search**——这是第四层静默失败(同仓多条检索路径,agent-facing 那条质量更低)。机制未定位(query vs search 在 Rust 层是不同检索/排序代码路径),**暂记待查项,纳入 D3 可观测 + 后续架构设计范围**。

**论题**:KB 的真问题不是"存储/架构设计错了",而是**检索栈在写入、召回、诊断、路径选择多层都静默失败**——好知识无声地不进索引、无声地被陈旧 gate 丢、agent 走的召回路径无声地更差、出问题时诊断还反着报。结果:既找不回,又诊断不出。

## 决策(决定了什么)

分两类:**立即的数据/运维修复(D1/D2)** + **根治的"让失败可见"(D3)**。

- **D1 — 给 74 个无 frontmatter 的 domain-map 文件补 frontmatter(数据,无 legacy-retrieval 改动)。**
  schema 用 **v3 老 schema**(对齐本仓已索引的 20 个:`schema_version/object_id/object_key/object_type/status/topic/...`),**不是 v5**——因 source-alpha 非 kb 仓,v5 极简 lane 对它不生效,强上可能 compile 不出 artifact。
  - **待 principal 拍**:(a) 全 74 个一律补 v3(机械、可脚本化);还是 (b) 先挑高价值子树(hsf-control-plane / storage-topology / error-patterns 等导航主文档)补,raw 子表后续。
  - 复用 ADR-001 "做需求就往里加" 的活样本:那 20 个就是 req-gate/示例领域治理顺手补的,证明补 frontmatter 是低成本已验证的路。

- **D2 — 重建 binary + 重嵌索引,清掉 `compiled_hash_mismatch`(运维,改核心待批)。**
  `cargo install --path crates/legacy-retrieval-cli --force`(消除 dirty/behind)+ 对 source-alpha 重跑 index,让 qdrant 向量 hash 与当前 artifact 同步。32 条被丢的高分语义即可通过 gate。**这一步的收益可能最大**——它直接放出已索引好货的最佳语义命中。

- **D3 — 让三层静默失败变可见(改 legacy-retrieval 核心,待批;这是本 ADR 的根治点)。**
  4 次误诊 + issue-example 证明:静默 + 说谎的诊断,比任何单个 bug 都贵。要求:
  - **写→索引**:`doctor`/build 必须列出 "skip N 篇(无 frontmatter):<路径>",不再静默吞。
  - **索引→召回**:`compiled_hash_mismatch` 丢弃要进**常规 doctor**(不只 `--diagnose`),报 "索引陈旧:N 条语义命中被丢,需 reindex"。
  - **诊断纠真**:`semantic_runtime` 探针走带-root 解析路径,`configured` 反映 query-time 真相,终结 `configured=false` 假警。

## 否决的替代方案

| 替代 | 否决理由(多数经实测证伪) |
|---|---|
| **搬 domain-map 到 `docs/kb/` / 扩 kb-lane path-gate** | 实测证伪:门是 frontmatter 不是路径;且 v5 kb-lane 对非-kb 仓不生效。搬过去照样无 frontmatter、照样 skip。 |
| **补 frontmatter 用 v5 极简 schema** | source-alpha 非 kb 仓走老 lane,v5 字段可能 compile 不出 artifact。对齐已索引的 20 个(v3)才稳。 |
| **去设 `WEAVE_QWEN3_QDRANT_HYBRID_COLLECTION` 修"语义关了"** | 基于误诊。语义本就通;设单值 env 反而让多仓退化成单 collection。 |
| **当成架构问题重设计 KB/frontmatter 契约** | 架构(ADR-001)经复核成立。真因是数据缺 frontmatter + 索引陈旧 + 诊断说谎,全是修不是重建。**别把运维问题升级成又一轮 schema 重构。** |
| **✅ 选:补 v3 + 重建索引 + 失败可见** | 关掉静默失败的三层,不动正确的架构/embedding,且让下次能自诊断。 |

## 后果(影响 + 风险)

**正面**:
- D1 放出 74 个 curated 文件 + D2 放出 32 条被丢的高分语义命中 = recall 终于返"好货"(主诉直接解)。
- D3 后,"为什么找不到 X" 一眼可诊断,不再每次靠读 Rust 代码反推(根除连环误诊)。

**负面 / 风险(诚实记)**:
- **D2/D3 改/重装检索核心 production 二进制** + reindex,属 ADR-001 明确教训里"该让 principal 过目、不 autopilot"的动作。改前备份二进制(同 ADR-001 做法)。**D1 是纯数据、可逆,风险最低,可先走。**
- D2 reindex 期间 :3002 embedding endpoint 单线程,会和其他用它的活互拖,挑空窗跑。
- D3 是改核心、有回归面,工作量不小——**不是"顺手"**,别低估。
- domain-map 用 v3 = 与 v5 KB-SKELETON 并存两套 schema(本仓 v3、kb 仓 v5);可接受(仓类型不同本就不同 lane),但要在 KB-SKELETON 注明,别再误以为"全宇宙 v5"。

## 过程教训(写给未来的我,本 ADR 最该留的东西)
- **本会话连环误诊 4 次**,全因信了表面信号没实测/没读码:① "recall.go 有 provenance filter"(读码:只滤 inferred)→ ② "legacy-retrieval search 是 workaround"(实测:也搜不到)→ ③ "语义全机器关了"(读码+实测:per-root 派生,通)→ ④ "门是路径 / domain-map 15 个全无 frontmatter / 真因是夹缝"(实测:94 个 / 20 个 v3 已索引 / 门是 frontmatter / 真因是 hash_mismatch+静默skip)。**第①版 ADR 整条 D1 因果链搭错,靠对抗 critic 才拦下。**
- **铁律**:检索/embedding 这类多层系统,任何"它坏了"的结论先 `--diagnose` + query-time 实测 + 读解析代码,再下口径。**`doctor` 的 config 探针不是运行时真相。** 这条铁律本身就是 D3 要把它变成工具默认行为的理由。
- 记忆 `wx-recall-drops-domain-map`("Go 层 provenance 过滤")已两重证伪,须改写为本 ADR 结论。
