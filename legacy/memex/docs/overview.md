---
description: memex 的产品定位、检索能力和 local-first 运行方式。
keywords: [memex, overview, lexical, semantic, hybrid]
kind: reference
links: [INDEX, architecture]
---

# memex overview

memex 面向个人或小团队的 Markdown 知识库检索。它不要求把文档迁移到数据库:
源文档继续保存在 Git 仓里,memex 只负责扫描、编译和检索。

核心能力:

- lexical lane:对标题、正文、路径和对象键做加权 BM25 检索。
- semantic lane:调用可配置 embedding endpoint,在 qdrant 里检索向量。
- hybrid lane:用 weighted RRF 融合 lexical 与 semantic 候选。
- planner:根据 query 形态调整权重,让中文自然语言和代码符号查询都有稳定表现。
- sync:增量编译源仓,默认 dry-run,写入前有大比例删除守卫。

memex 的默认姿态是 local-first。索引产物、telemetry 和配置都在本机;外部服务地址、
认证值和 CA bundle 只通过环境变量注入,不写入仓库。
