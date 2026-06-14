# rhizome

`rhizome` 是一个去中心知识库的 authoring/校验 CLI。它把知识以 Markdown + frontmatter
的形式分散存在多个普通 Git 仓里，每个仓自带域树（domain tree），由 `INDEX.md` 文件
自发现。`rhizome` 本身不集中存储数据，也不绑定任何检索引擎——它只管写入契约和提交门禁。

## 核心概念

- **note**：一篇 Markdown 文件，带 5 字段 frontmatter（description、keywords、kind、links、code）。
- **domain**：一个带 `INDEX.md` 的目录，是知识的归属节点。
- **identity**：`<repo>:<domain>:<slug>`，完全由文件位置推导，无 uuid。
- **registry**：`kb-sources.toml`，手工维护的源仓列表。
- **检索引擎解耦**：`rhizome` 只写入和校验；检索（向量/全文）由外部引擎对接，不在本工具范围内。

## 命令

| 命令 | 作用 |
|---|---|
| `rhizome new <slug>` | 建一篇 5 字段 frontmatter note，body 从 stdin 读取。 |
| `rhizome check [files]` | 校验 frontmatter 契约；`--fix` 自动剔除 legacy 字段。 |
| `rhizome check --duplicate-domains` | 校验同仓无重名 C2 domain。 |
| `rhizome check --staged-frozen` | 阻断对 frozen 文档的删除/重命名。 |
| `rhizome domains` | 列出所有源仓及其域树。 |
| `rhizome domains --diff` | 与 Qdrant 中心集合对账，看哪些 domain 尚未索引。 |
| `rhizome adopt <repo>` | 一键纳管一个仓：写 registry 行 + INDEX 骨架 + lefthook 门禁。 |

## 安装

```sh
uv tool install --force .
```

本地开发：

```sh
uv sync
uv run rhizome --help
uv run pytest
```

## 配置

`rhizome` 查找 registry 的顺序：

1. `$KB_SOURCES` 环境变量，直接指向 `kb-sources.toml` 文件。
2. 从当前目录向上查找 `kb-sources.toml`。
3. `$KB_WORKSPACE_ROOT/kb-sources.toml`（默认 `~/workspace`）。
4. `~/.config/rhizome/sources.toml`。

`kb-sources.toml` 示例：

```toml
workspace_root = "~/workspace"

[[source]]
name = "my-kb"

[[source]]
name = "another-kb"
path = "~/notes/another-kb"
```

## frontmatter 契约

```yaml
---
description: "一句话描述，供检索用"
keywords: [关键词1, 关键词2]
kind: note          # spec / reference / runbook / decision / research / note / index
links: [other-note-slug]
code: [repo/path/to/file.py]
---
```

`kind: decision` 额外支持 `assets`（交付资产列表）和 `supersedes`（取代旧决策）字段。

## Mermaid 校验（可选）

`mermaid-validator/` 是一个可选的 Node.js sidecar，用 Mermaid 自身的 JS 解析器
校验文档中的 Mermaid 代码块。启用方式：

```sh
npm install --prefix mermaid-validator
```

未安装时，包含 Mermaid 块的文档在提交时会报 ERROR（需 node 在 PATH 上）。

## 开发检查

```sh
uv run pytest
uv run ruff check .
uv run ruff format --check .
```

## 许可证

MIT
