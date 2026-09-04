"""单仓编译报告(C1 覆盖率 diff + loud-skip 清单 + 降级 + 域树)。

可读打印 + 结构化字段。一个 repo 编一次 → 一个 RepoReport。
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class SkipEntry:
    source_path: str
    reason: str


@dataclass(frozen=True)
class KindDowngrade:
    source_path: str
    identity: str
    from_kind: str


@dataclass
class RepoReport:
    repo: str
    repo_path: str
    indexed: int = 0
    skipped: list[SkipEntry] = field(default_factory=list)
    kind_downgrades: list[KindDowngrade] = field(default_factory=list)
    kind_missing: list[str] = field(
        default_factory=list
    )  # 无 kind frontmatter 的 source_path
    domains: list[str] = field(default_factory=list)  # 发现的全部域(含根域 "")
    empty_domains: list[str] = field(default_factory=list)  # 发现但无 note 的域
    duplicate_error: str | None = None  # duplicate-domain / identity 撞键 loud error
    error: str | None = None  # 仓不可用 / 其它硬错误

    def summary_line(self) -> str:
        """一行摘要(给 dry-run / 编排日志)。"""
        if self.error:
            return f"{self.repo}: ERROR — {self.error}"
        if self.duplicate_error:
            return f"{self.repo}: DUPLICATE — {self.duplicate_error}"
        empties = f", {len(self.empty_domains)} empty" if self.empty_domains else ""
        return (
            f"{self.repo}: indexed {self.indexed}, skip {len(self.skipped)}, "
            f"domains {len(self.domains)}{empties}, "
            f"kind-downgrade {len(self.kind_downgrades)}, "
            f"kind-missing {len(self.kind_missing)}"
        )

    def render(self) -> str:  # noqa: C901 — 报告渲染: 逐 section 拼装文本行, 分支多但线性、无嵌套逻辑
        """完整可读报告。"""
        lines = [f"=== {self.repo}  ({self.repo_path})"]
        if self.error:
            lines.append(f"  ERROR: {self.error}")
            return "\n".join(lines)
        if self.duplicate_error:
            lines.append(f"  DUPLICATE ERROR: {self.duplicate_error}")
            return "\n".join(lines)
        lines.append(self.summary_line())
        if self.domains:
            lines.append("  domains:")
            for d in self.domains:
                label = d or "(root)"
                mark = "  [EMPTY]" if d in self.empty_domains else ""
                lines.append(f"    - {label}{mark}")
        else:
            lines.append("  domains: (none — no INDEX.md)")
        if self.skipped:
            lines.append(f"  loud-skip (no frontmatter) [{len(self.skipped)}]:")
            for s in self.skipped:
                lines.append(f"    - {s.source_path}  ({s.reason})")
        if self.kind_downgrades:
            lines.append(f"  kind downgraded → note [{len(self.kind_downgrades)}]:")
            for k in self.kind_downgrades:
                lines.append(f"    - {k.source_path}: {k.from_kind!r} → note")
        if self.kind_missing:
            lines.append(
                f"  kind missing (默认 note, 稀释 kind prior) [{len(self.kind_missing)}]:"
            )
            for p in self.kind_missing:
                lines.append(f"    - {p}")
        return "\n".join(lines)
