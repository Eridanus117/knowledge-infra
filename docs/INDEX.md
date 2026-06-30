---
description: memex 引擎的长期开发文档域:读/写两条路径的架构(current-state model anchor)、治理控制视图、设计取舍与改动入口。
keywords: [memex, architecture, recall, indexing, hybrid, governance-control, current-state-model, snapshot@2026-06-30, anchor]
kind: index
links: [overview, architecture, governance-control-view]
---

# memex docs

memex 引擎自己的长期开发文档。

- [overview](overview.md):memex 怎么运作、为什么这么设计(理解导向)——三条 lane、RRF 融合、读写两路径、关键取舍。
- [architecture](architecture.md):引擎级 codemap(current-state model anchor)——读路径(recall/hybrid/lexical/semantic/planner)、写路径(indexing 子包)、registry/config、System Context 图、核心不变量、改 X 去哪。
- [governance-control-view](governance-control-view.md):治理控制视图——读/写两路径核心不变量 + 写安全守卫(prune 三守卫/unit-mode/退出码)的 enforcement map(强制机制 + 重验口)。
