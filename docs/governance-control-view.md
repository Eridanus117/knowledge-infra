---
description: "memex 引擎治理控制视图:把读/写两路径的核心不变量与写安全守卫 catalog 成一处,每条标强制机制(源文件:符号)+ 重验口。不变量(读写同源/point_id 不变性/可索引闸门/identity 派生/中央隔离/telemetry 无副作用)与守卫(prune 三守卫/unit-mode 断言/退出码/dry-run)分两张表。指针式不复制源码正文。"
keywords: [memex, governance-control, invariant, guard, prune-guard, point-id, indexability-gate, exit-code, current-state-model, snapshot@2026-06-30, detail]
kind: reference
links: [architecture]
code: [src/memex/indexing/sync.py, src/memex/config.py, src/memex/semantic.py, src/memex/indexing/compile.py, src/memex/indexing/cli.py]
---

# memex Governance/Control 视图

> **Governing viewpoint**: Governance/Control(Software/System 资产层)——读/写两路径里哪些约束破坏即引擎不可信(不变量)、写路径靠哪些守卫不误删/不混写(guard)。
> **Purpose / Concerns**: 把散在 `config.py` / `semantic.py` / `indexing/sync.py` / `indexing/compile.py` 里的控制面**汇成一处可查 catalog**,回答「改检索/写路径会撞到哪些约束、怎么重验它没被破坏」。
> **Primary Audience**: 改引擎读写逻辑的人/agent、onboarding、审计。
> **Notation**: 不变量 catalog + 守卫/门 catalog 两张表(行号为 snapshot 时刻锚,符号更稳)。
> **真相源 & correspondence**: 强制真相在**各源文件本体**;本视图是**指针式 catalog**,不复制源码正文,每条链到 enforcing 符号 + 可跑重验口。路径相对 memex 仓根,源指针锚见 frontmatter `code:`。
> **Scope**: memex 引擎(读路径顶层 + `indexing/` 写路径);上游 authoring 契约(frontmatter/kind/域)归 [[rhizome:docs:governance-control-view]]。

## 入口表

| 想查 | 看哪 |
|---|---|
| 哪些约束破坏即引擎读写不可信 | [§1 不变量](#1-不变量invariants) |
| 写路径靠什么不误删/不混写/不静默 | [§2 守卫/门](#2-守卫guard) |
| 改检索默认值的放行门 | [§3 检索默认值 = 跨仓 eval gate](#3-检索默认值--跨仓-eval-gate) |

## 1. 不变量(Invariants)

破坏即引擎读写口径不可信。`强制机制`列即 enforcement map(机制 ↔ 不变量)。

| 不变量 | 破坏的后果 | 强制机制(源文件:符号) | 验证/重验口 |
|---|---|---|---|
| **读写 profile 常量同源**:`CENTRAL_INDEX_PROFILE`/`CENTRAL_POINT_KIND`/`EMBEDDING_PROFILE_ID` 读侧定义、写侧 import 同一份 | 读写 filter 口径漂移,写进的点读不出来 | 定义 `src/memex/semantic.py`(`CENTRAL_INDEX_PROFILE`/`CENTRAL_POINT_KIND`/`EMBEDDING_PROFILE_ID`,~L29-33);写侧 `src/memex/indexing/sync.py` `from memex.semantic import ...`(~L22-32) | `uv run pytest tests/test_sync.py tests/test_semantic.py` |
| **point_id 含 unit-mode 且 NAMESPACE 稳定**:`point_id = uuid5(POINT_NAMESPACE, identity+":"+unit_mode)`,改 NAMESPACE = 全库 re-key | 误改 = 整库 re-key;切 chunk 混跑 = 增量错乱 | `src/memex/indexing/sync.py:POINT_NAMESPACE`(uuid5 字面量,~L39)·`point_id`(~L83) | `uv run pytest tests/test_sync.py::test_point_id_deterministic_and_mode_distinct` |
| **可索引性闸门**:有 frontmatter ⟺ 可索引,无 → loud-skip 进报告不静默 | 静默漏索引 = 现状盲区 | 硬门 `src/memex/indexing/compile.py:compile_note`(无 fm 返回 `doc=None`);记录 `src/memex/indexing/pipeline.py`(`SkipEntry`);loud `src/memex/indexing/integrity.py:findings_for_report`(升级 `DOMAIN_SKIP` + exit 3) | `uv run pytest tests/test_indexing.py tests/test_integrity.py` |
| **identity 位置派生**:`<repo>:<domain>:<slug>`,无 uuid/手写 key;移动文件 = 换 identity | 手写 key/物理目录名入 identity → 同 note 在 worktree 分叉 | 派生 `src/memex/indexing/scan.py:derive_identity`;worktree 防护 = registry **逻辑名**作 `name` 入参固定(`pipeline.py` `repo = name`,ADR-035),**不取物理目录名**;旧 `scan.py:repo_name()` helper 现**无调用点**(死代码,勿据 architecture 旧描述当主路径) | `uv run pytest tests/test_indexing.py`;`grep -rn "repo_name(" src/memex/`(确认无调用) |
| **写路径只碰中央 collection**:`sync_repo` 唯一 collection = `s.central_collection`,绝不碰 legacy per-root | 误写 legacy 产物 / 污染迁移基线 | `src/memex/indexing/sync.py`(`coll = s.central_collection`,~L288);`src/memex/indexing/qdrant.py`(collection-agnostic 薄封装,调用方保证传中央名) | `uv run pytest tests/test_sync.py` |
| **`read_from_central` 默认中央**:读路径默认从中央 compiled/collection 读,翻它 = cutover 级 | 误翻 = lexical 读源与 semantic 检索目标全变 | `src/memex/config.py:Settings.read_from_central`(默认 `True`,~L28);lexical `engine.py` / semantic `semantic.py` 据同 flag 切换;env `KB_SEARCH_READ_FROM_CENTRAL` 可覆盖 | `uv run pytest tests/test_config.py tests/test_central_read.py` |
| **embed 超时设长防队列雪崩**:`embed_timeout_secs` 默认 600s | 短超时遗弃请求 → 单线程服务端磨弃请求 → 队列雪崩 | `src/memex/config.py:Settings.embed_timeout_secs`(默认 `600.0`,~L35);用于 `semantic.py` `_post_json` timeout | `grep -n "embed_timeout_secs" src/memex/config.py src/memex/semantic.py` |
| **telemetry 不影响命令**:best-effort 落 ledger,吞所有异常,不改退出码 | telemetry 失败拖垮/改写命令退出码 | 实现迁 `gnomon`:`gnomon/src/gnomon/telemetry.py:record`(末尾 `except Exception: return`)·`run_instrumented`(返回 app 原始 exit code);env `MEMEX_TELEMETRY_OFF`/`DO_NOT_TRACK` 关 | `DO_NOT_TRACK=1 memex-sync sync-all; echo $?`;`uv run pytest tests/test_telemetry.py` |

## 2. 守卫(Guard)

写路径(`memex-sync`)的拦截点,挡误删/混写/静默失败。

| 守卫/门 | 触发时机 | 拦什么 | 失败行为 |
|---|---|---|---|
| **prune 守卫① per-repo 收窄** | sync prune diff | 候选删点用 identity 前缀 `repo:` 客户端过滤,绝不跨仓判删 | `src/memex/indexing/sync.py:_scroll_repo_points`(~L229);跨仓点不进删候选 |
| **prune 守卫② >50% 拒绝** | sync prune diff | 单仓待删 > 50% 视为异常,拒绝 | `_MASS_PRUNE_RATIO=0.5`(~L48);`len(stale) > repo_points * ratio and not force` → 拒绝,需显式 `--force`(exit 2) |
| **prune 守卫③ 默认 dry-run** | 每次 sync | 默认不动 qdrant | `_DRY_RUN`(`apply=False`,~L60);`sync_repo(mode=_DRY_RUN)` 默认,`--apply` 才写 |
| **unit-mode 一致断言** | apply 前读集合 | collection 内 chunk/whole mode 不一致 = 拒绝,禁增量混跑 | `src/memex/indexing/sync.py:_assert_unit_mode`(~L211);mode mismatch → `report.error` |
| **退出码语义 0/1/2/3** | `memex-sync` 收尾 | 0 全绿 / 1 硬失败 / 2 prune 守卫拒绝(需 `--force`)/ 3 内容完整性发现(0-doc 仓/域内静默 skip) | `src/memex/indexing/cli.py`(`EXIT_FAILURE=1`/`EXIT_NEEDS_FORCE=2`/`EXIT_INTEGRITY=3`,~L37);`sync_all_cmd` 按 1>2>3 优先级 `raise typer.Exit` |

重验口(守卫整体):`uv run pytest tests/test_sync.py tests/test_cli.py`;手动 `memex-sync sync-all; echo $?`(dry-run 全绿应为 0)。

## 3. 检索默认值 = eval gate(procedural,非自动拦截)

排序/融合/lane 默认参数(`hybrid.py` 的 `RRF_K`/`_weights`、planner 锚定阈值、`lexical.py` 的 `FIELD_BOOST`)的**默认值**变更,放行门 = memex **eval gate**:`eval/run_goldset.py` 可跑、含冻结基线常量(`BASELINE`/`HYBRID_BASELINE`),lexical 闸 gold@10 ≥ BASELINE per slice、hybrid 闸 ≥ max(lexical,semantic) + HYBRID_BASELINE 绝对地板。

> **诚实口径(对抗核实)**:这是 **procedural gate——无 commit hook 自动拦截**,靠合并前人工跑 + 约定执行,违反无自动阻断。别 overclaim 成代码强制门。
> **重验口**:`uv run python eval/run_goldset.py`(lexical 闸)·`uv run python eval/run_goldset.py --lane hybrid --protect`(hybrid 绝对地板闸)。

决策见 [[rhizome:decisions:adr-023-kb-retrieval-index-decisions]];KB 系统侧同条不变量见 [[rhizome:docs:governance-control-view]] §1。

> **correspondence 注记**:本视图每条强制机制应是 `src/memex/` 下 tracked 符号,缺失即漂移。行号随代码演进会漂,以**符号**为准;snapshot 之后改了写路径/config,重跑上述 pytest + `rhizome check` 后更新 `snapshot@` 日期。
</content>
</invoke>
