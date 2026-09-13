# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.

## 本仓的用法

本仓与同一 owner 下其他仓统一用上表五个标签，不另加分诊标签。

- `needs-triage`：待维护者评估或拍板，同时指派维护者，让通知找到人。
- `ready-for-agent`：不需要再决策，agent 可直接动手。
- `ready-for-human`：要维护者亲手做的事。
- `needs-info`：等 CI、上游或他人。
- `wontfix`：不做。
- 以后再说的：不打标签，直接关 issue，要做再重开。

旧标签的映射（读旧 issue 时用，旧标签已删）：

| 旧标签 | 新做法 |
| --- | --- |
| `等你定` | `needs-triage` |
| `本机agent` | `ready-for-agent` |
| `等外部` | `needs-info` |
| `缓` | 不打标签，关闭 issue |
