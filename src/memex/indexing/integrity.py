"""compile 内容完整性发现。

把 compile 现有的"打印 skip"升级为**结构化、可被 cadence 消费**的告警信号。
职责切分:内容是否真的进了索引归 compile;管道完整性(gate/INDEX 存在、命令可
解析)归上游 authoring 工具的 doctor,两者不重叠、不重复跑 compile。

本模块只回答 compile 侧的两类"内容没索引"完整性问题:

  ① ZERO_DOC —— 注册源仓**有 KB 结构(发现了 INDEX.md 域)却 compile 出 0 doc**:
     疑似全仓没进索引。注意区分"仓里根本没 INDEX.md"(域外散落 md 本就不该索引,
     不算问题)与"有域却 0 doc"(算问题)。

  ② DOMAIN_SKIP —— **域内**(有 INDEX.md 的目录子树下)文件**因无 frontmatter 被
     静默 skip**。scan 阶段只扫域子树内的 .md(域外散落 md 根本不进 scan、不产生
     SkipEntry),故 RepoReport.skipped 天然只含"域内该索引却被 skip"的文件,直接
     即是 ②,无需再做域内/域外区分。

硬错误(error / duplicate_error)是 compile 既有的 fail-stop 路径,不在本模块的
完整性发现范围内(它们已经 loud 且非 0;重复纳入只会噪音)。
"""

from __future__ import annotations

from dataclasses import dataclass, field

from memex.indexing.report import RepoReport

# 完整性发现类型(programmatic 信号: cadence 可按 kind 路由/聚合)。
ZERO_DOC = "zero_doc"  # 有域却 compile 出 0 doc(疑似全仓没进索引)
DOMAIN_SKIP = "domain_skip"  # 域内文件因无 frontmatter 被静默 skip


@dataclass(frozen=True)
class IntegrityFinding:
    """一条 compile 内容完整性发现(可诊断级:哪个仓、哪类、计数、明细)。"""

    repo: str
    repo_path: str
    kind: str  # ZERO_DOC | DOMAIN_SKIP
    detail: str  # 人类可读一句话
    count: int = 0  # 涉及文件数(DOMAIN_SKIP);ZERO_DOC 为 0
    paths: tuple[str, ...] = ()  # 明细(DOMAIN_SKIP 的被 skip source_path)


def findings_for_report(report: RepoReport) -> list[IntegrityFinding]:
    """从单仓 RepoReport 抽出 compile 完整性发现(零或多条)。

    硬错误仓(error/duplicate_error)直接返回空:它们走 compile 既有 fail-stop,
    非"悄悄没索引",不重复 loud。
    """
    if report.error or report.duplicate_error:
        return []
    findings: list[IntegrityFinding] = []
    # ① ZERO_DOC: 发现了域(有 INDEX.md)但 indexed==0 → 整仓内容没进索引。
    #    无域(domains 为空)= 仓里没 INDEX.md,域外散落 md 本就不索引,不算问题。
    if report.domains and report.indexed == 0:
        findings.append(
            IntegrityFinding(
                repo=report.repo,
                repo_path=report.repo_path,
                kind=ZERO_DOC,
                detail=(
                    f"{len(report.domains)} 个域被发现但 compile 出 0 doc"
                    "(疑似全仓没进索引)"
                ),
            )
        )
    # ② DOMAIN_SKIP: 域内文件无 frontmatter 被静默 skip(scan 已保证只含域内)。
    if report.skipped:
        paths = tuple(s.source_path for s in report.skipped)
        findings.append(
            IntegrityFinding(
                repo=report.repo,
                repo_path=report.repo_path,
                kind=DOMAIN_SKIP,
                detail=(
                    f"{len(paths)} 个域内文件因无 frontmatter 被 skip(该索引却没进索引)"
                ),
                count=len(paths),
                paths=paths,
            )
        )
    return findings


@dataclass
class IntegrityReport:
    """跨仓完整性发现聚合(compile/sync CLI 收尾产出, cadence 据此告警)。"""

    findings: list[IntegrityFinding] = field(default_factory=list)

    @property
    def has_findings(self) -> bool:
        """programmatic flag: 是否存在任一完整性问题。"""
        return bool(self.findings)

    def add_repo(self, report: RepoReport) -> None:
        self.findings.extend(findings_for_report(report))

    def zero_doc_repos(self) -> list[IntegrityFinding]:
        return [f for f in self.findings if f.kind == ZERO_DOC]

    def domain_skip_repos(self) -> list[IntegrityFinding]:
        return [f for f in self.findings if f.kind == DOMAIN_SKIP]

    def render(self) -> str:
        """显眼的独立 INTEGRITY section(无发现返回空串,调用方可不打印)。"""
        if not self.findings:
            return ""
        zero = self.zero_doc_repos()
        skip = self.domain_skip_repos()
        lines = [
            "!!! INTEGRITY — 内容完整性发现(compile 侧;管道完整性归上游 "
            "authoring 工具的 doctor)",
            f"!!! 0-doc 仓 {len(zero)} | 域内静默 skip 仓 {len(skip)} "
            f"(共 {sum(f.count for f in skip)} 文件)",
        ]
        if zero:
            lines.append("  [ZERO_DOC] 有域却 compile 出 0 doc:")
            for f in zero:
                lines.append(f"    - {f.repo} ({f.repo_path}): {f.detail}")
        if skip:
            lines.append("  [DOMAIN_SKIP] 域内文件无 frontmatter 被静默 skip:")
            for f in skip:
                lines.append(f"    - {f.repo}: {f.detail}")
                for p in f.paths:
                    lines.append(f"        · {p}")
        return "\n".join(lines)
