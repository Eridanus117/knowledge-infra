---
description: memex 引擎的长期开发文档域:读/写两条路径的架构、设计取舍与改动入口。
keywords: [memex, architecture, recall, indexing, hybrid]
kind: index
---

# memex docs

memex 引擎自己的长期开发文档。

- [overview](overview.md):memex 怎么运作、为什么这么设计(理解导向)——三条 lane、RRF 融合、读写两路径、关键取舍。
- [architecture](architecture.md):引擎级 codemap——读路径(recall/hybrid/lexical/semantic/planner)、写路径(indexing 子包)、registry/config、核心不变量、改 X 去哪。
