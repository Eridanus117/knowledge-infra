# memex

`memex` 是一个 local-first 的知识库(KB)混合检索引擎。它把散在多个源仓里的
Markdown note 编译进一个中央向量库,对外给出一条「最佳召回」——融合
lexical(BM25)、semantic(向量)与 hybrid(加权 RRF),再叠一层确定性的
query planner,中英混排、代码符号和自然语言问句都能稳定命中。

数据和服务都跑在本地:源仓是普通 Git 仓里的 Markdown + frontmatter,向量存
本地 qdrant,embedding 走一个可配置的本地服务。没有云端依赖,索引产物可审阅、
可重建。

## 适合什么场景

- 想给自己的一堆 Markdown 笔记/文档仓做一个能跨仓检索的本地引擎。
- 既要关键词精确命中(代码符号、标识符),又要语义召回(中文口语问句)。
- 希望检索结果对 agent / 脚本友好:稳定的 JSON 输出、带 title/path 富化。
- 希望索引是确定性的:note 的位置即 identity,改名/移动 = re-key,可 diff、可重建。

## 两个入口

| 入口 | 用途 |
|---|---|
| `memex` | 读路径:检索 / 召回 / 用量统计。 |
| `memex-sync` | 写路径:扫源仓 → 编译 → 同步进中央向量库。 |

## 检索原理

- **lexical**:per-repo 4 字段加权 BM25(title / body / object_key / path),
  自然语言走 jieba 分词,标识符走 slug 切分。
- **semantic**:调外部 embedding 服务把 query 向量化,在 qdrant 里按 Cosine 检索。
- **hybrid**:weighted-RRF 融合两条 lane。一个确定性 planner 判断 query 类型
  (中文低锚 / 代码符号强锚定),据此调权重——中文口语抬 semantic,代码符号
  密集抬 lexical,避免精确命中被语义召回挤出。
- **recall**:对外唯一的「最佳召回」入口,锁定生产最佳配置(hybrid +
  lexical-dependent protection + title/path 富化),调用方不必懂 lane 调参。

更细的架构、模块地图和核心不变量见 [`docs/architecture.md`](docs/architecture.md)。

## 源仓配置

memex 通过一个 `kb-sources.toml` 知道要检索哪些源仓:

```toml
# workspace 根(可选;默认 ~/workspace)
workspace_root = "~/workspace"

[[source]]
name = "docs"
path = "~/workspace/docs"

[[source]]
name = "notes"
# 省略 path 时 = workspace_root / name
```

解析顺序:

1. `$KB_SOURCES` 指定的文件路径(若设置)。
2. 否则 `$KB_WORKSPACE_ROOT`(或 `~/workspace`)下的 `kb-sources.toml`。
3. 都不可用时 fail-safe 降级到内置默认,并打 warning。

只有真正建过索引(有 artifacts)的源仓才会进入 active 集合。

## 外部服务

semantic lane 依赖两个本地服务,默认地址可用 env 覆盖(前缀 `KB_SEARCH_`):

- qdrant 向量库:`KB_SEARCH_QDRANT_URL`(默认 `<embedding-endpoint>`)。
- embedding 服务(OpenAI 兼容 `/v1/embeddings`):`KB_SEARCH_EMBEDDING_URL`、
  `KB_SEARCH_EMBEDDING_MODEL`、`KB_SEARCH_EMBEDDING_DIMENSIONS`。

只用 lexical lane 时无需这两个服务。

## 安装

```sh
uv tool install --force .
```

本地开发:

```sh
uv sync
uv run memex --help
uv run pytest
```

## 常用命令

读路径:

```sh
memex recall "查询文本"                 # 最佳召回(hybrid),默认入口
memex recall "查询文本" --domain decisions --kind reference
memex recall "查询文本" --format json   # 给脚本/agent 的稳定 JSON
memex query "查询文本" --lane lexical   # 低层单 lane 调参/调试
memex stats                             # 本地用量 telemetry 汇总
```

写路径:

```sh
memex-sync compile                      # 扫源仓 → 编译落盘(dry-run 友好)
memex-sync sync --apply                 # 编译 + 写进 qdrant
memex-sync sync-all --apply             # 遍历 registry 同步所有源仓
```

写路径默认 dry-run(零写入),`--apply` 才真正动 qdrant;prune 删除有
「单仓待删 >50% 拒绝、需 `--force`」守卫,防误删整仓。

## 开发检查

```sh
uv run poe check         # lint + typecheck + test
uv run poe lint          # ruff check
uv run poe fmt           # ruff format
uv run poe typecheck     # pyrefly check
uv run poe test          # pytest
```

GitHub Actions 会运行 lint、format check、type check 和 tests。

## 许可证

MIT
