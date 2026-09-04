---
description: "rhizome CLI 的理解导向概览：解释去中心 Markdown 知识、note/domain/identity/frontmatter、new/check 写入门禁、frozen 边界与工具心智模型。"
keywords: [rhizome, overview, CLI, note, domain, identity, frontmatter, new, check, frozen]
kind: reference
links: [architecture]
code: [src/rhizome]
---

# rhizome 概览

> 讲 rhizome 怎么运作、为什么这么设计(理解导向)。要跑起来 → [README](../README.md);要改代码 → [architecture](architecture.md)。本文不讲安装、不列字段清单。

## 它解决什么

知识库通常有个隐形的中央服务:一个 schema、一个数据库、一套 uuid。写一篇笔记,要先问"它该归哪个集合、ID 怎么生成、字段对不对"。rhizome 反过来:**知识就是普通 Git 仓里的 Markdown 文件**,分散在你机器上任意多个仓里,没有中央存储,也不绑定任何检索引擎。

rhizome 是这套去中心格局里的**写入端工具**——它只做两件事:**按契约把一篇笔记落到正确的位置**(`rhizome new`),和**在提交前把不合契约的笔记拦下来**(`rhizome check`)。它**不存数据、不建索引、不做检索**。这条边界是有意的:一个既管写入又管检索的工具,会把"知识怎么存"和"知识怎么找"耦死;rhizome 只钉死前者(写入契约 + 提交门禁),后者(向量/全文索引)交给外部引擎读这些文件自己建。**代价**:rhizome 自己回答不了"帮我找一篇讲 X 的笔记"——那是检索引擎的事,不在本工具范围内。

## 几个核心概念

**note = Markdown + frontmatter,身份由位置推导。** 一篇笔记是一个 `.md` 文件,顶部带一段固定的 frontmatter(必填 `description`/`keywords`,可选 `kind`/`links`/`code`)。关键设计:笔记**没有 uuid、没有写死的 ID**。它的 identity 是 `<repo>:<domain>:<slug>`——仓名、域路径、文件名,**全部从文件物理位置推导**。**为什么**:位置即真相,意味着移动一个文件就是改它的归属,不需要同步任何元数据;也意味着新旧 frontmatter 能并存——身份不依赖字段内容,所以老笔记不刷新也能被正确归类。

**domain = 带 INDEX.md 的目录,仓根即根。** "域"不是配置出来的,是**自发现**的:任何含 `INDEX.md` 的目录就是一个域节点。一个仓的域树就是它的 INDEX.md 分布。**为什么**:域和目录同构,你用文件系统组织知识时顺手就定义了域,不需要单独维护一棵分类树。一个微妙之处:**域路径不等于物理路径**。canonical 的域口径是 "C2 node-chain"——从仓根往下,只有**自己带 INDEX.md 的目录段**才计入域路径,中间那些没有 INDEX.md 的物理目录(比如 `docs/`、`source-notes/`)被跳过。所以 `docs/source-notes/blue/`(只有 `blue/` 有 INDEX.md)的域是 `blue`,不是 `docs/source-notes/blue`。**为什么**:物理布局是你的整理自由(想塞进 `docs/` 随你),但对外暴露的域口径要稳定、干净,且跟中央索引里实际存的 key 一致。

**frontmatter 契约 = 单一真相,字段精简到刚好够检索。** 契约只认 5 个字段,外加 `kind: decision` 专属的 `assets`/`supersedes` 和单值 `status: frozen`。一批历史字段(`object_id`/`updated_at`/`topic`/...)被**明确"杀掉"**,还有一批(`domain`/`title`/`identity`/`verified`)是**派生字段**——它们能从位置/H1/git 无损重建,所以不该手写进 frontmatter。**为什么**:frontmatter 越胖,越容易漂移、越容易在多个地方各写一份真相。契约把"必须手写的"压到最小,其余一律推导。

**rhizome new 与 rhizome check = 写入与门禁,职责分离。** `new` 负责**机械组装合规 frontmatter**(作者不花一个 token 在 YAML 上,正文从 stdin 管进来),并把文件落到正确的域目录。`check` 负责**在 commit 时校验**:缺字段、kind 非法、带了被杀字段——拦下。**为什么分两个**:写入是一次性动作(作者意图),门禁是反复发生的约束(任何改动、任何人/agent 提交都要过)。门禁通常挂在 git pre-commit hook 上自动跑,所以它必须独立于"谁写的、怎么写的"。

## 一次典型流程怎么流起来

把概念串成一条线——从想法到一篇落地且过门禁的知识:

1. 你 `cd` 进某个 KB 仓里某个域目录,把笔记正文管给 `rhizome new <slug> -d "一句话描述" -k 关键词1,关键词2`。
2. `new` 从 cwd 往上找最近的 `INDEX.md` 定出域,从仓根推出 identity,组装合规 frontmatter,把 `<slug>.md` 写进那个域目录。**它不会覆盖已存在的文件**,也**不会**在找不到 INDEX.md 时悄悄落到祖先域——没有域就报错,要你先建 INDEX.md。
3. 你 `git add` + `git commit`。仓里装的 pre-commit hook 对 staged Markdown 跑 `rhizome check`,并额外执行 duplicate-domain 与 staged-frozen 仓级守卫。
4. check 通过 → 提交落地;有 ERROR → 提交被拦,告诉你哪篇哪个字段错了。一类常见错误(带了被杀/派生字段)可以 `rhizome check --fix` 一键无损剥除,再提交。

第 2 步和第 4 步藏着 rhizome 的核心姿态:**写入和校验都 fail-loud**。`new` 找不到域宁可报错也不猜;`check` 测不准(frontmatter 坏了)就报 ERROR 而不是放行。"没明确合规"不等于"合规"。

## 一条关键边界:frozen 是只读历史

有两类笔记是**冻结的只读快照**:显式标了 `status: frozen` 的,和所有 `kind: decision`(ADR/PRD,生来即冻)。对它们,rhizome 有一条硬规矩:**冻结文档不许改,增量要靠写一篇新的 superseding 文档**,而不是编辑旧的。

这条规矩的实现有个值得点明的精巧处:门禁判断"是否冻结"看的是 **HEAD(已提交)版本的 frontmatter,不是工作区版本**。**为什么**:否则"同一个 commit 里先把 `status: frozen` 去掉、再编辑正文"就能绕过门禁——而那个"去冻结"动作本身就是在改一篇冻结文档,必须走 supersede。删除/重命名一篇冻结文档同样被拦(这条走仓级的 staged 检查,因为单文件检查看不到已删的路径)。

## 心智模型的边界

rhizome 是个**写入端的契约执法者**,别把它想成知识库本身:

- 它**不存知识**——知识存在你的 Git 仓里,rhizome 只是帮你把文件摆对、把不合规的拦下。
- 它**不做检索**——`rhizome domains --diff` 能告诉你哪些域还没被中央索引收录(对账),但索引和查询都是外部引擎(Qdrant 等)的事。
- 它**不是通用 YAML 工具**——它的 frontmatter 解析器是一个故意做小的扁平 reader(只认 `description`/`keywords` 这种笔记会带的形状),不支持 anchor/嵌套/多文档;复杂 YAML 留给索引端的 loud-skip 兜底。
- 它**只管 KB 域内的 Markdown**——同一个仓里混着 PM 的 `issues/*.md`(带自己的 schema)或仓根的草稿?只要不在某个 INDEX.md 域下,rhizome 就当它不是 note,不校验、不 `--fix`。

## 下一站

- 要纳管一个新仓 / 写第一篇笔记 → [README](../README.md)。
- 要加校验规则、改 frozen 生命周期、看真实模块地图 → [architecture](architecture.md)。
- 为什么是这些具体取舍(契约怎么收敛到这 5 个字段、domain 为什么用 C2 node-chain)→ 归 PM / ADR,本仓只留指针。
