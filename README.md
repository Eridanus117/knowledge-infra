# memex

`memex` 是一个 local-first 的 Markdown 知识库检索引擎。它把多个源仓里的
Markdown note 编译成可重建索引,对外提供 lexical(BM25)、semantic(向量)与
hybrid(weighted RRF) 检索。

数据默认留在本机:源仓是普通 Git 仓里的 Markdown + frontmatter,编译产物在
用户本地数据目录,向量库和 embedding endpoint 都通过环境变量配置。只用 lexical
lane 时不需要外部服务。

## 入口

| 命令 | 用途 |
|---|---|
| `memex` | 读路径:检索、召回、本地用量统计。 |
| `memex-sync` | 写路径:扫描源仓、编译、同步向量索引。 |

## 源仓配置

memex 通过 `kb-sources.toml` 声明需要索引的源仓:

```toml
source_root = "~/projects"

[[source]]
name = "docs"

[[source]]
name = "notes"
path = "~/projects/notes"
```

解析顺序:

1. `$KB_SOURCES` 指定的 TOML 文件。
2. 否则 `$KB_SOURCE_ROOT` 下的 `kb-sources.toml`。
3. 都不可用时降级到内置示例源,并打印 warning。

基础路径依次接受 `$KB_SOURCE_ROOT`、共享的 `$KB_WORKSPACE_ROOT`、registry
中的 `source_root` / `workspace_root`。registry 同目录可放一个
`<stem>.local.toml`（例如 `sources.local.toml`）覆盖已有 source 的机器本地
`path` / `legacy`；它不新增逻辑 source，避免物理路径变化改写索引 identity。

## 外部服务

semantic lane 依赖一个 OpenAI-compatible embedding endpoint 和一个 qdrant
实例。常用配置:

- `KB_SEARCH_QDRANT_URL`:qdrant URL,默认 `<embedding-endpoint>`。
- `KB_SEARCH_EMBEDDING_URL`:query embedding endpoint,默认 `<embedding-endpoint>/v1/embeddings`。
- `KB_SEARCH_SYNC_EMBEDDING_URL`:sync/write-path embedding endpoint;未配置时从
  `/embedding-query/` 自动派生 `/embedding-sync/`,否则复用 `KB_SEARCH_EMBEDDING_URL`。
- `KB_SEARCH_EMBEDDING_MODEL`:embedding model name。
- `KB_SEARCH_EMBEDDING_DIMENSIONS`:embedding vector dimension。
- `KB_SEARCH_BEARER_TOKEN`:可选 Qdrant bearer credential。
- `KB_SEARCH_CA_BUNDLE`:可选 CA bundle 路径。

## 安装

运行环境使用 [GitHub Releases](https://github.com/source-gamma/memex/releases) 中的
自包含二进制，不需要 Python、`uv` 或本地源码仓。每个 release 提供
`memex-<os>-<arch>`、`memex-sync-<os>-<arch>` 和 `SHA256SUMS`；安装器必须先按
checksum 校验，再写入 PATH。

当前构建目标是 macOS arm64 与 Linux x86_64；Linux 产物以 Ubuntu 22.04 为
兼容基线。直接安装 macOS arm64 版本：

```sh
base=https://github.com/source-gamma/memex/releases/latest/download
curl -fL "$base/memex-darwin-arm64" -o /tmp/memex-darwin-arm64
curl -fL "$base/memex-sync-darwin-arm64" -o /tmp/memex-sync-darwin-arm64
curl -fL "$base/SHA256SUMS" -o /tmp/memex-SHA256SUMS
(cd /tmp && grep -E '  memex(-sync)?-darwin-arm64$' memex-SHA256SUMS | shasum -a 256 -c -)
install -m 0755 /tmp/memex-darwin-arm64 ~/.local/bin/memex
install -m 0755 /tmp/memex-sync-darwin-arm64 ~/.local/bin/memex-sync
```

## 开发

```sh
uv sync --group dev
uv run memex --help
uv run memex-sync --help
uv run pytest
```

运行 `./scripts/build-release.sh` 可在 `dist/release/` 生成当前 OS/arch 的两个
二进制。Pull request 会先在双平台构建和 smoke test；推送与 `pyproject.toml`
版本一致的 `v*` tag 后才生成 `SHA256SUMS` 并发布不可变 release。

常用命令:

```sh
memex recall "查询文本"
memex recall "查询文本" --format json
memex query "查询文本" --lane lexical
memex-sync compile
memex-sync sync --apply
```

写路径默认 dry-run,只有显式传 `--apply` 才会写向量库或落盘 compiled 产物。

低摩擦 raw 捕获和生命周期晋级:

```sh
memex-sync capture --repo source-alpha=/path/to/source-alpha \
  --title "先收下的想法" --text "原始材料" --apply
memex-sync promote --repo source-alpha=/path/to/source-alpha \
  --path 000-raw/2026/08/25/123000-先收下的想法.md --to derived --apply
memex-sync promote --repo source-alpha=/path/to/source-alpha \
  --path 000-raw/2026/08/25/123000-先收下的想法.md --to canonical \
  --last-verified 2026-08-25 --evidence "人工核验: 资料来源" --apply
```

`capture` 默认 dry-run,按 `000-raw/YYYY/MM/DD/` 落盘并只写 `status: raw`;
不要求 `kind`。`promote` 只能相邻晋级: `unclassified → raw → derived →
canonical`;晋级 canonical 必须显式提供核验日期和至少一条 evidence。`kind` 在
显式 lifecycle 文档中只是可选检索标签,不是捕获或晋级门禁。

## 文档

架构和模块地图见 [`docs/architecture.md`](docs/architecture.md)。

## 许可证

MIT
