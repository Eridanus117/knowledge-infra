---
description: memex 读写路径、模块边界和索引不变量。
keywords: [memex, architecture, retrieval, indexing, sync]
kind: reference
links: [INDEX, overview]
---

# memex architecture

memex 分成读路径和写路径。

读路径:

1. `cli.py` 解析命令。
2. `engine.py` 组合 lexical、semantic 和 hybrid lane。
3. `planner.py` 判断 query 是否偏自然语言或强符号锚定。
4. `recall.py` 输出对 agent 和脚本稳定的最佳召回结果。

写路径:

1. `registry.py` 读取 `kb-sources.toml`。
2. `indexing/lifecycle.py` 提供 raw capture 和相邻状态晋级门禁。
3. `indexing/scan.py` 扫描 Markdown note。
4. `indexing/compile.py` 生成 compiled docs。
5. `indexing/sync.py` 同步 qdrant,复用未变化向量,并执行删除守卫。
6. `indexing/qdrant.py` 封装最小 HTTP client。

核心不变量:

- source registry 的 `name` 是 repo identity,不依赖物理目录名。
- identity 由位置派生,文件移动会生成新 identity。
- 写路径默认 dry-run,只有 `--apply` 才写本地 compiled 产物或 qdrant。
- raw capture 以 `000-raw/YYYY/MM/DD/` 为日期优先入口,显式 status 承担生命周期;
  kind 不参与 capture/promotion 门禁。
- lifecycle 只能相邻晋级;canonical 必须带 `last_verified` 和 `evidence`。
- semantic 配置只从环境变量读取,不要把 endpoint、认证值或 CA 路径写入仓库。
- 大比例 prune 必须显式 `--force`。
