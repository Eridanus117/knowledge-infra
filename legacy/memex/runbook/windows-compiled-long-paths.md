---
description: "Windows compiled 长路径的症状、根因与修复验收：保留 identity URL 编码文件名，在文件系统边界使用扩展路径；区分 MAX_PATH 总长度与单组件限制，记录 312/507 字符真实 CLI 编译、检索、更新和清旧。"
keywords: [memex, Windows, MAX_PATH, 中文路径, compiled, prune, 长路径]
kind: runbook
code: [src/memex/_paths.py, src/memex/indexing/compile.py, src/memex/compiled.py, src/memex/indexing/pipeline.py, src/memex/indexing/doctor.py]
---

# Windows compiled 长路径

## 症状与根因

中文知识根或笔记路径变长后，`memex-sync compile` 可以完成扫描，却在
`compile_cmd → persist → write_compiled → Path.write_text` 抛出
`FileNotFoundError`。只验证一个很短的隔离输出目录，不能证明正常 compiled 落点可用。

这不是「Windows 不支持中文」。`safe_filename(identity)` 将完整 identity 按
UTF-8 做 URL 百分号编码，保持一一对应。例如常用汉字的三个 UTF-8 字节会成为
九个 ASCII 字符。内容根进入 domain/identity 后，也会进入这个编码文件名。

需要分别检查两个限制：

- **总路径长度**：传统 Win32 `MAX_PATH` 是 260，包含结尾空字符；普通路径可能在
  超过 259 个可见字符时失败。父目录长度同样计入；目录创建还有自己的传统长度限制。
- **单组件长度**：文件系统通常允许单个目录名或文件名最多 255 个 UTF-16 单元。
  扩展路径不会取消这个限制。本次百分号编码后的文件名是 ASCII，最长 221 字符，
  因而是合法组件；失败来自总路径，而非单组件超限。

## 修复前的可复现证据（2026-09-07）

修复前，正常 compiled 落点的长路径会在写入阶段失败；仅验证短隔离目录不能证明正常落点可用。

- 最长编码文件名组件为 221 字符，正常完整路径最长 282 字符，其中 6 项超过 259。
- 在输出根长度为 50 字符的隔离 fixture 中，完整路径 261 字符的写入失败并返回 exit 1。
- 更短输出根下最大完整路径为 257 字符，40 篇匿名化的真实文档样本和查询通过；该结果不覆盖正常长度。

这些数字来自隔离输出中的真实文档样本；具体内容和路径省略，不将样本路径冒充通用
synthetic fixture 的实测长度。

## 通用复现 fixture

使用至少一个会编码为 221 字符文件名的 synthetic identity，并分别设置输出根长度为 50、
80 和 275 字符。运行 compile，记录实际输出根、最长完整路径和 exit code；不要将占位符
`<out>`、`<fixture>` 或重复字符示例当作实测路径长度。

公共回归案例 `test_long_compiled_paths_roundtrip_and_prune` 覆盖长文件名的写入、读回、
同 identity 更新、清旧和路径边界；完整 private 运行证据不放入公开 runbook。

公开记录只保留可复现的通用行为和测试统计；本机路径、用户名和知识标题
属于内部证据边界，不是公开接口。

## 修法与兼容边界

`memex._paths.io_path` 只在 compiled 文件系统边界提供 Windows 扩展路径：

1. 普通 Windows 路径先做绝对化和词法规范化，再加 `\\?\`，避免相对路径和 `..`
   在扩展路径命名空间下失去原有含义；不依赖目标已经存在，也不解析 source identity。
2. 普通 UNC 路径转换为 `\\?\UNC\server\share\…`；已有 `\\?\` 路径不重复加前缀。
3. 非 Windows 路径原样返回。
4. 即使父目录短，也在枚举前转换：其子文件可能因编码文件名而超过总路径限制。

覆盖范围：

| 入口 | 受保护 IO | 保持原契约的部分 |
|---|---|---|
| `write_compiled` | 创建目录、写入/覆盖 JSON | 返回原 logical `Path`，文件名不变 |
| `load_compiled_docs` / `load_compiled_corpus` | 判断目录、枚举、读 JSON | identity、source_path 和 repo 从原 JSON/目录名取得 |
| `prune_stale_compiled` | 枚举、删除陈旧文件 | 按原文件名比较，保留 >50% 守卫 |
| `prune_retired_repos` | 枚举、递归删除退役仓目录 | 保留全量 registry 边界和 >50% 守卫 |
| `check_compiled_consistency` | 按 identity 推导文件名并检查存在性 | orphan `expected_path` 仍是普通展示路径 |

`write_compiled` 在现有生产代码中只有 `persist` 调用，且不使用返回值；没有遗留的
消费者拿未转换的长返回路径继续做 read/stat。返回值仍可相对原输出目录做
`relative_to`。Engine、recall 的按 id 读取共用 compiled loader 构建的字典，无需另一套
文件名派生或加载协议。IO 前缀不写入 identity、source_path、JSON 协议或正常 CLI 输出。

不改 hash/URL 文件命名、中文根名、Memex/Rhizome 公共契约、全局 compiled 位置、registry
或 Windows 设置；不捕获写入异常来假装成功。

## 修复后的 Windows 验收（2026-09-07）

实现候选位于独立 linked worktree，使用分支 `fix/windows-compiled-paths`，基于
`origin/feature-port`。

验证使用项目已有解释器与依赖，并通过进程级 `PYTHONPATH=<candidate-src>` 指向候选源码；
输出目录、源路径和临时 registry 均省略具体值，不记录本机用户名或 private 绝对路径。

对同一批匿名化的真实文档样本使用两个隔离输出落点：

| 隔离输出根 | 输出根长度 | 文档数 | 最长组件 | 最长完整路径 | 超过 259 的文件 |
|---|---:|---:|---:|---:|---:|
| `<proof>/compiled` | 80 | 40 | 221 | 312 | 19 |
| `<proof>/nested-<60个x>/nested-<60个y>/nested-<60个z>` | 275 | 40 | 221 | 507 | 40 |

`<60个x>` 等表示运行时实际生成的 60 个重复字符；表中占位路径不代表测量值。
第二个落点的父目录本身超过 260，因此同时覆盖创建、目录存在性和枚举，而不只是单次写入。

以下是参数化命令形态，不是逐字转录的原始命令（`$python` 为项目解释器，`$out` 为隔离输出落点）：

```powershell
& $python -m memex.indexing.cli compile --repo "sample=<source-root>" --out $out
& $python -m memex.cli query "<representative-query>" --repo sample --lane lexical --hits 2 --format json
```

两处 compile 均 exit 0：`indexed 40, skip 0`、`wrote 40`。逐个读取 JSON，40 个匿名化
真实文档样本的文件名集合与修复前证据一致；加载器各读回 40 个 Doc。两处均执行实际
查询，top-1 identity 保持不变。第二组 507 字符输出还验证最长文件可查询，返回的 `path`
保持普通仓相对路径；具体查询内容、identity 和路径省略。

### 更新和清旧不是只测 write

在 275 字符 synthetic 输出根下创建独立 fixture 源，不修改真实知识正文：

1. 真实 CLI 编译 3 个 synthetic 文档；查询 `beforetoken` 命中长 identity。
2. 保持另一篇 identity 不变，将其正文更新为 `aftertoken`，移除旧源；再次真实 compile。
3. 查询 `aftertoken` 命中更新后的同 identity；旧 compiled 被 prune，查询
   `beforetoken` 返回 `{"hits": []}`。
4. 用 synthetic retired fixture 编译额外退役仓，再用同时包含原 40 篇真实文档样本仓
   与测试 fixture 仓的临时 registry 运行全量 compile；真实删除 retired 目录，剩余两个
   语料仓均保留。
5. 相对路径含 `..` 的真实 IO 成功；已扩展路径参与 loader IO 且不重复加前缀。

所有 smoke CLI 命令 exit 0；临时输出、fixture、registry 和一次性验证脚本已清理。

### 保留回归与验证限制

相关测试使用项目已有 dev 解释器（Python 3.12），同样以进程
`PYTHONPATH=<candidate-src>` 指向候选：

```text
python -m pytest tests/test_compiled_prune.py tests/test_doctor.py tests/test_central_read.py tests/test_indexing.py -q
93 passed, 1 skipped
```

新增回归保护长文件和深目录的读回、同 identity 覆盖、清旧/整仓清理，以及 doctor 不把
真实长文件误报为孤儿、错误报告不泄漏 IO 前缀。既有 TOML fixture 改用 `as_posix()` 生成
跨平台合法输入；删除仅绑定显示文案的断言。recall 的路径回归以临时真实源文件验证
返回绝对路径可以读取，不绑定 POSIX 分隔符。上述相关测试的跳过项是大小写不敏感
文件系统下无法构造的 `Foo.md` / `foo.md` 冲突，不是长路径测试。

最终项目验证（同一候选、未重建 smoke 输出）：

```text
python -m pytest -q -rs
227 passed, 7 skipped
uvx ruff@0.15.16 check --no-cache .
All checks passed!
uvx ruff@0.15.16 format --check --no-cache .
55 files already formatted
pyrefly check --python-interpreter-path <project-venv-python>
0 errors (3 suppressed)
```

全套测试进程移除 smoke 专用的 telemetry 开关，使使用临时数据库的 telemetry 测试覆盖
真实记录行为。7 个 skip 分别为本地 Qdrant 不可达的 5 项、大小写碰撞 1 项、Windows
无 `fork` 的多进程测试 1 项。

Ruff 初次普通检查虽通过但无法写缓存；最后以 `--no-cache` 检查全部文件，无告警，
没有为此改权限或工具配置。Pyrefly 显式使用项目 dev 解释器，避免误选其它环境；
按项目现有自动配置使用 `basic` preset，原有 3 个 suppression 未新增、未更改。

- UNC 只验证了路径转换；没有可用网络共享，**未声称真实 UNC 网络 IO 通过**。
- 本次真实运行是 Windows；非 Windows 分支保持原 Path，跨平台 CI 结果另行记录。
- 没有验证 Qdrant/embedding 网络同步，没有改排序、检索默认值或向量契约。
- 没有在正式 compiled 落点写入，没有安装/部署候选；本记录是候选实现验收，不是部署完成。
- 单组件超过文件系统限制仍会失败，写入错误仍向上抛出；不要将总路径修复当成无限文件名支持。
