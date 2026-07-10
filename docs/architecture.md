---
description: rhizome 代码仓开发地图 — 4 个 verb、frontmatter 契约、两套 domain 口径、frozen/duplicate-domain 门禁的真实模块与不变量
keywords: [rhizome, architecture, frontmatter, contract, domain, check, frozen-gate, mermaid-validator, CLI]
kind: reference
---

# rhizome 架构

> 开发者地图。装 / 用见 [README](../README.md);本文给要改这块代码的人。本篇只讲**现状**:为什么是这些取舍(契约收敛、C2 node-chain 口径的来由)归 PM / ADR,这里只留指针。
>
> 校验过的 source of truth(校验日期 2026-07-06,version 0.1.0):`pyproject.toml`(console script `rhizome = "rhizome.cli:run"`,Python >=3.12,runtime 依赖 `gnomon` telemetry core)、`src/rhizome/cli.py`、`src/rhizome/contract.py`、`src/rhizome/check.py`、`src/rhizome/sources.py`、`src/rhizome/adopt.py`、`src/rhizome/amend.py`、`src/rhizome/capture.py`、`src/rhizome/config.py`、`src/rhizome/doctor.py`、`src/rhizome/links.py`、`src/rhizome/relocate.py`、`src/rhizome/telemetry.py`、`mermaid-validator/validate-mermaid.mjs`。

## 鸟瞰

rhizome 是一个**去中心、文件式知识库的写入端 CLI**:把 Markdown + frontmatter 笔记按契约落进正确的域目录,并在 commit 时把不合契约的拦下。它不存数据、不建索引、不做检索——索引与查询交给外部引擎读这些文件自建。整个包是 `src/rhizome`(对外命令同名 `rhizome`),核心逻辑以 Python 标准库为主,运行期只接入 `gnomon` 做本地 telemetry,可选一个 Node sidecar 做 Mermaid 校验。主要 verb:`new`(写)/ `amend`(补写)/ `check`(门禁)/ `domains`(看域树 + 与中央索引对账)/ `adopt`(纳管一个仓)/ `capture`(闪念捕获)/ `relocate`(跨源搬迁)。

## 分层

```
cli.py            ── argparse 入口,纯 IO 边界(parse/print/exit code)
  │
  ├─ contract.py  ── 契约单一真相:字段集 / kind 枚举 / 两套 domain 推导 / frontmatter render+parse+strip
  │
  ├─ check.py     ── 校验:per-file 契约检查 + frozen gate + Mermaid gate + 仓级 duplicate-domain / staged-frozen
  │     └─ links.py ── (check 延迟 import)links/code 断引用检查
  │
  ├─ sources.py   ── registry(kb-sources.toml)解析 + 域树自发现 + 与中央 Qdrant 对账
  │
  └─ adopt.py     ── 一键纳管:registry 行 + INDEX 骨架 + lefthook 门禁,plan-then-apply 幂等

amend.py / capture.py / relocate.py / telemetry.py
                 ── 补写、闪念捕获、跨源搬迁、本地 telemetry

mermaid-validator/  ── 可选 Node sidecar,用 Mermaid 自己的 JS parser 校验图块(check.py 经 subprocess 调)
```

`contract.py` 是所有人的依赖底座(`check`/`cli`/`sources`/`adopt`/`links` 全 import 它);`links.py` 反过来依赖 `check.Finding`,所以 `check.check_path` 用**延迟 import** 破环。

## 真实模块 codemap

| 符号 / 文件 | 职责 | 路径 |
|---|---|---|
| `run` / `_build_parser` | argparse 入口;子命令解析与分发 | `src/rhizome/cli.py` |
| `run_new` | 纯函数:校验输入 → 定域 → 推 identity → 组装 frontmatter → 落文件(不打印) | `src/rhizome/cli.py` |
| `_cmd_check` / `_fix_paths` | check 子命令编排:per-file + 仓级守卫 + `--fix` 剥字段 + JSON/文本输出 | `src/rhizome/cli.py` |
| 契约常量 | `REQUIRED_FIELDS`/`OPTIONAL_FIELDS`/`KINDS`(7 值)/`KILLED_FIELDS`/`DERIVED_FIELDS`/`STATUS_ALLOWED`/`ASSET_PREFIXES` | `src/rhizome/contract.py` |
| `validate_*` | topic(kebab slug)/description(单行)/keywords/kind/assets 校验 | `src/rhizome/contract.py` |
| `find_repo_root` / `repo_name` | 最近 `.git` 祖先;worktree 下从 `gitdir:` 反推**主 checkout** 名(identity 稳定) | `src/rhizome/contract.py` |
| `find_domain_dir` / `has_index` | 最近含 INDEX.md 的祖先;`has_index` 用 `os.listdir` 精确比名(APFS 大小写安全) | `src/rhizome/contract.py` |
| `derive_domain` | **内部物理定位器**:域目录的仓相对路径(只用于落文件,不外泄语义) | `src/rhizome/contract.py` |
| `derive_node_chain_domain` | **canonical 域口径(C2 node-chain)**:仓根往下、只取自带 INDEX.md 的段拼成域路径 | `src/rhizome/contract.py` |
| `derive_identity` | `<repo>:<domain>:<slug>`,全位置推导,无 uuid | `src/rhizome/contract.py` |
| `render_frontmatter` / `render_note` | 组装合规 frontmatter(字段定序 + YAML 安全引用);不注入 H1 | `src/rhizome/contract.py` |
| `parse_frontmatter` / `split_frontmatter` | 故意做小的扁平 reader(flow/block list、block scalar、quoted/bare、null);非通用 YAML 引擎 | `src/rhizome/contract.py` |
| `strip_fields` | `--fix` 用:逐行剥指定 top-level key(含续行),正文与其余字节级保留 | `src/rhizome/contract.py` |
| `is_note_location` / `is_frozen_fm` | 是否在某 KB 域内(域感知 gating);是否冻结(`status: frozen` 或 `kind: decision`) | `src/rhizome/contract.py` |
| `check_text` | 单篇 raw text 校验,产 `Finding[]`(ERROR 阻断 / WARN 浮现) | `src/rhizome/check.py` |
| `check_path` | `check_text` + `frozen_gate_findings` + `links.link_findings` | `src/rhizome/check.py` |
| `frozen_gate_findings` / `head_frozen` | 工作区版本 ≠ **HEAD 冻结版本** → ERROR(`git show HEAD:<rel>`) | `src/rhizome/check.py` |
| `staged_frozen_findings` | 仓级:staged 的删除/重命名命中 HEAD 冻结 KB 笔记 → ERROR | `src/rhizome/check.py` |
| `duplicate_domain_findings` / `_iter_index_dirs` | 仓级:两个物理 INDEX.md 目录 C2 域路径相撞 → ERROR | `src/rhizome/check.py` |
| `mermaid_findings` / `mermaid_blocks` | 抽 ```mermaid 块,经 sidecar 校验;node/sidecar 缺失 → ERROR | `src/rhizome/check.py` |
| `body_asset_ids` | 从正文「触达资产 / Touched Assets」段抽 asset_id(机械、窄) | `src/rhizome/check.py` |
| `link_findings` | links(同仓 slug 须解析:断链 ERROR / 跨仓 WARN / identity 形 ERROR)、code(出处 hint,一律 ≤WARN) | `src/rhizome/links.py` |
| `find_registry` / `load_sources` | 定位基础 registry(4 级查找)+ 合并 sibling `*.local.toml` 机器路径 overlay，解析成 `[(name, path)]` | `src/rhizome/sources.py` |
| `discover_domains` / `note_domain` | 走 INDEX.md 自发现域树(C2 口径);一篇笔记归哪个域 | `src/rhizome/sources.py` |
| `build_tree` / `diff` | 域树(喂 surface-hook/recall)/ 与中央 Qdrant 集合的覆盖对账 | `src/rhizome/sources.py` |
| `central_note_index` / `_qdrant_scroll_page` | scroll 中央集合 → `{repo: {identity: source_path}}`(stdlib urllib);不可达则**大声报错** | `src/rhizome/sources.py` |
| `run_adopt` | plan-then-apply 幂等纳管:registry 行 + INDEX 骨架(仅当无域)+ lefthook 门禁 | `src/rhizome/adopt.py` |
| `run_capture` | 把临时文本 append 到本地 inbox,不进入 KB 索引边界 | `src/rhizome/capture.py` |
| `plan_relocate` / `apply_relocate` | 跨源移动 note,更新 wikilink,维护搬迁 ledger | `src/rhizome/relocate.py` |
| `record_invocation` | 记录本地 CLI 调用的 stdout/stderr 摘要和退出码 | `src/rhizome/telemetry.py` |
| `LEFTHOOK_YML` | 写入仓的 pre-commit 模板:`rhizome check {staged_files}` + `--duplicate-domains --staged-frozen` | `src/rhizome/adopt.py` |
| `validate-mermaid.mjs` | Node sidecar:用 Mermaid `parse()` 判图块合法,产 `{findings:[...]}` JSON | `mermaid-validator/` |

## 核心不变量

1. **identity 全位置推导,无 uuid。** `<repo>:<domain>:<slug>` = 仓名 / 域路径 / 文件名。`repo_name` 在 worktree 下故意取**主 checkout** 名(从 `.git` 文件的 `gitdir:` 反推),否则同一路径在 worktree 和 main 会得到不同 identity(中央索引 key 漂移)。

2. **两套 domain 口径,不可混。** `derive_domain`(物理仓相对路径)**仅**用于落新文件;一切对外语义(identity、`domains`、recall filter、duplicate 守卫)走 `derive_node_chain_domain`(C2 node-chain)。两者仅当域目录坐落在非域物理目录之下时分叉。混用会让暴露的域口径与中央索引实际存的 key 不一致。

3. **仓根永远不是域。** 即便仓根有 INDEX.md,它派生出空域("")并被所有消费方忽略;域是子目录。

4. **C2 域路径唯一(仓内)。** 两个物理 INDEX.md 目录派生出同一 C2 域 = 索引期静默合并,`rhizome check --duplicate-domains` 在 commit 时拦下。修法:给那个有差异的中间目录加一个 INDEX.md,让两条 node chain 分开。

5. **frozen = 只读历史,按 HEAD 判定。** 冻结 ⟺ `status: frozen` 或 `kind: decision`(生来即冻、字段隐含)。frozen gate 比的是**工作区文本 vs HEAD 版本**,不是工作区 frontmatter——否则"同 commit 先去 frozen 再编辑"会直接穿过(去冻结本身就是改冻结文档)。删除/重命名走仓级 `staged_frozen_findings`(单文件检查看不到已删路径)。

6. **域感知 gating:域外 Markdown 不是 note。** `is_note_location`:祖先无 INDEX.md → 不校验、不 `--fix`。让同一个 rhizome-check hook 能安全跑在混装 KB 与非 KB Markdown 的仓(PM `issues/*.md`、仓根草稿)。不在 git 仓里 → fallback 当 note 检查。

7. **被杀/派生字段是 ERROR,但可无损 `--fix`。** `KILLED_FIELDS`(object_id/updated_at/...)纯噪声、`DERIVED_FIELDS`(domain/title/identity/verified)可从位置/H1/git 重建,所以 `--fix` 机械剥除。`status` 是**值感知**的:非 `frozen` 值被剥,合法 `status: frozen` 保留。`--fix` **永不重写 HEAD 冻结文档**(只读快照,要 supersede)。

8. **render 出的 frontmatter YAML 安全。** flow 列表项只在"明确是纯字符串"时裸写,否则双引号——防 `2026`→int、`007`→7、`true`→bool、`~`→null 这类下游 parser(gopkg.in/yaml.v3 等)的强制转型。

9. **对账 fail-loud,不静默撒谎。** `central_note_index` 在 Qdrant 不可达时抛 `SourcesError`;`rhizome domains --diff` 必须明说"测不到",绝不悄悄报"not-indexed"。

10. **ERROR 阻断 / WARN 浮现。** ERROR = 缺字段 / kind 非法 / 被杀派生字段 / 坏 frontmatter / 断链 / Mermaid 解析失败 / 触碰冻结文档 → 退出码 1,挡 commit。WARN = 需人判断的软信号(decision-only 字段用错地方、未知 asset 前缀、assets 与正文段不一致、未知字段、code 出处漂移、跨仓 link)→ 不阻断,默认不打印(`-w` 才显示)。

## 关键流程

### `rhizome new`(从命令到落地)

`_cmd_new`(读 stdin 正文,tty 则拒)→ `run_new`(纯函数):

1. `validate_topic/description/keywords/kind/assets` 逐项校验;`--assets` 仅 `kind: decision` 可用,空正文报错。
2. `find_repo_root(cwd)` 定仓根;不在 git 仓 → 报错。
3. 定域:给了 `--domain` 则它必须是精确的域目录(自带 INDEX.md,不回退到祖先);否则 `find_domain_dir(cwd)` 取**最近含 INDEX.md 的祖先**。找不到域 → 报错(要你先建 INDEX.md),**不**悄悄落到祖先。
4. `derive_node_chain_domain` 出 C2 域路径(仓根 → 空,报错),`derive_identity` 拼 identity。
5. 目标 `<topic>.md` 已存在 → 报错(**不覆盖**)。`render_frontmatter` + `render_note`(正文 strip 包裹,不注入 H1)写文件。
6. 返回 `{path, domain, identity, kind}`,`_cmd_new` 打印或 `--json`。

### `rhizome check`(per-file + 仓级守卫)

`_cmd_check`:

1. 仓级守卫先跑:`--duplicate-domains`(或 `--all`)→ `duplicate_domain_findings`;`--staged-frozen` → `staged_frozen_findings`。两者皆可独立运行(commit hook 的仓守卫),也可与 per-file 并跑。
2. 收路径:`--all` 走仓根 `rglob("*.md")`(跳 `.git/.venv/__pycache__/node_modules`),否则取 hook 传入的 staged Markdown paths。
3. `--fix`:对每条路径,跳过域外文件与 HEAD 冻结文件,`strip_fields` 剥 `KILLED ∪ DERIVED`(+ 非法 `status`),仅当确有改动才回写,打印改了哪些。
4. per-file:`check_path` = `check_text`(契约 + Mermaid)+ `frozen_gate_findings` + `link_findings`。`check_text` 内对 `kind: decision` 还做 assets 审计(未知前缀、数量阈值 12、与正文「触达资产」段对账)。
5. 跨文件:`_asset_reuse_candidates` 聚合 ≥3 篇 decision 复用同一 asset → externalize 候选 WARN。
6. 任一 ERROR → 退出码 1。

### Mermaid 校验(check 的子流程)

`mermaid_findings`:抽 ```mermaid 块 → 若有块但 `node` 不在 PATH / sidecar 脚本缺失 / `node_modules/mermaid` 没装 → ERROR;否则把块 JSON 喂 `validate-mermaid.mjs`(经 stdin),sidecar 用 Mermaid 自己的 `parse()` 判合法,返回 `{findings:[{line,message}]}`,逐条转 ERROR。用真 parser 比 Python 正则 linter 更准,又避开 mmdc/Chromium 渲染,保持 hook 快且确定。

### `rhizome adopt`(纳管一个仓)

`run_adopt` 是 plan-then-apply、幂等:先探明所有前提(仓形状、registry 名/路径冲突、缺 `-d/-k`、lefthook binary、case-variant `index.md`)再落第一次写,失败不留半纳管态。三步各自探测当前态、只补缺失:① `kb-sources.toml` 追一行 `[[source]]`(文本追加 + `os.replace` 原子 + self-check 复读);② 仓无域时写 `docs/INDEX.md` 骨架(`kind: index`,过 check);③ 写 `lefthook.yml` + `lefthook install`(已有 pre-commit 框架接了 gate 则认作已收敛,不强装)。

## 改 X 去哪

| 我想加 / 改 … | 从这里入手 | 坑 |
|---|---|---|
| 加 / 改一个 frontmatter 字段 | `contract.py` 的字段集常量 + `check.py` 的对应分支 | 契约是单一真相;同步改 `ALLOWED`/`KILLED`/`DERIVED` 与 `check_text`,加 test |
| 加一个 kind | `contract.KINDS` | 7 值是有意收敛(design-doc 故意不是 kind);别随手加 |
| 改 frozen 生命周期 / 门禁 | `check.py` 的 `frozen_gate_findings` / `staged_frozen_findings` | 受不变量 5 约束:必须按 **HEAD** 判,不是工作区 |
| 改 domain 推导 | `contract.py` 的 `derive_node_chain_domain`(canonical)/ `derive_domain`(内部) | 不变量 2:别让物理口径外泄;改了同步 `duplicate_domain_findings` 与 test |
| 改 / 加校验规则 | `check.py` 的 `check_text` | 想清 ERROR(阻断,需可机械修)还是 WARN(软信号,需人判断) |
| 改断引用检查 | `links.py` | frozen 整篇豁免;延迟 import 自 `check`(破环);code 一律 ≤WARN |
| 改 registry 查找 / 域树 / 对账 | `sources.py` | 不变量 9:Qdrant 不可达必须抛,不能静默 not-indexed |
| 改 frontmatter 解析 / 渲染 | `contract.py` 的 `parse_frontmatter` / `render_frontmatter` | 不是通用 YAML 引擎;render 的引用规则(不变量 8)别破 |
| 改纳管流程 | `adopt.py` 的 `run_adopt` | plan-then-apply:所有探测在第一次写之前;改 `LEFTHOOK_YML` 模板要同步 hook 行为 |
| 改 Mermaid 校验 | `mermaid-validator/validate-mermaid.mjs` + `check.mermaid_findings` | 缺 node/sidecar 是 ERROR(有意);改 JSON 协议两端同步 |
| 加 CLI 命令 | `cli.py` 的 `_build_parser` + `_cmd_*` + `main` 分发 | 业务逻辑写成纯函数(像 `run_new`/`run_adopt`),`_cmd_*` 只做 IO |

## 非目标

- **不存知识、不建索引、不做检索** —— 知识在你的 Git 仓里;索引/查询交给外部引擎读文件自建。rhizome 只管写入契约与提交门禁。
- **不是通用 YAML 工具** —— `parse_frontmatter` 是为笔记的扁平 mapping 故意做小的 reader,不支持 anchor/嵌套/多文档。
- **不校验域外 Markdown** —— 不在任何 INDEX.md 域下的文件(PM `issues/`、仓根草稿)不是 note,不碰。
- **不绑定检索引擎** —— `domains --diff` 对账 Qdrant 是只读对照,不写、不依赖它跑通核心写入/校验。
- **不复述选型论证 / 历史** —— 契约为什么收敛、C2 node-chain 为什么这么定,归 PM / ADR。
