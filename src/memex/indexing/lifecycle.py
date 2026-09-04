"""低摩擦 raw 捕获与显式生命周期晋级。

捕获只写最小元数据: ``status: raw``、时间和来源;不要求 kind。
canonical 是一个需要人工确认的状态,只能从 derived 晋级,并要求显式
``last_verified`` 与 ``evidence``。所有文件修改由调用方的 ``--apply`` 控制。
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from datetime import date, datetime
from pathlib import Path

from memex.indexing.frontmatter import FrontmatterError, parse_frontmatter, split_frontmatter

RAW_DIR = "000-raw"
RAW_INDEX = "INDEX.md"
RAW_INDEX_CONTENT = """---
description: raw capture inbox;内容先收下,核验后再晋级。
keywords: [raw, inbox]
kind: index
---

# Raw capture

这里保存尚未核验的原始材料。不要把 raw 直接当作 canonical 依据。
"""

STATUSES = frozenset({"unclassified", "raw", "derived", "canonical"})
_STATUS_ORDER = {"unclassified": 0, "raw": 1, "derived": 2, "canonical": 3}
_DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
_KEY_RE = re.compile(r"^([A-Za-z0-9_-]+):")


class LifecycleError(ValueError):
    """生命周期动作不满足机械门禁。"""


@dataclass(frozen=True)
class CapturePlan:
    path: Path
    content: str
    index_path: Path
    index_created: bool


@dataclass(frozen=True)
class PromotionPlan:
    path: Path
    source_status: str
    target_status: str
    content: str


def _slug(title: str) -> str:
    """把标题收窄成安全文件名;保留中文,不允许路径分隔符。"""
    value = " ".join(title.strip().split())
    value = re.sub(r"[^\w\-\u4e00-\u9fff.]+", "-", value, flags=re.UNICODE)
    value = value.strip("-._")
    return (value or "untitled")[:80]


def _capture_path(repo_root: Path, title: str, captured_at: datetime) -> Path:
    stamp = captured_at.strftime("%H%M%S")
    day = captured_at.strftime("%Y/%m/%d")
    base = repo_root / RAW_DIR / day / f"{stamp}-{_slug(title)}.md"
    if not base.exists():
        return base
    for n in range(2, 1000):
        candidate = base.with_name(f"{base.stem}-{n:02d}{base.suffix}")
        if not candidate.exists():
            return candidate
    raise LifecycleError(f"capture filename collision: {base}")


def plan_capture(
    repo_root: Path,
    *,
    title: str,
    body: str,
    source: str = "manual",
    captured_at: datetime | None = None,
) -> CapturePlan:
    """生成 raw note 计划,不写磁盘。"""
    repo_root = repo_root.expanduser().resolve()
    if not repo_root.is_dir():
        raise LifecycleError(f"repo path not found: {repo_root}")
    title = " ".join(title.strip().split())
    if not title:
        raise LifecycleError("capture title cannot be empty")
    source = " ".join(source.strip().split()) or "manual"
    captured_at = captured_at or datetime.now().astimezone()
    captured = captured_at.isoformat(timespec="seconds")
    content = (
        "---\n"
        "status: raw\n"
        f"captured_at: {json.dumps(captured, ensure_ascii=False)}\n"
        f"source: {json.dumps(source, ensure_ascii=False)}\n"
        "---\n\n"
        f"# {title}\n\n"
        f"{body.strip()}\n"
    )
    index_path = repo_root / RAW_DIR / RAW_INDEX
    return CapturePlan(
        path=_capture_path(repo_root, title, captured_at),
        content=content,
        index_path=index_path,
        index_created=not index_path.exists(),
    )


def apply_capture(plan: CapturePlan) -> None:
    """应用 capture 计划;不覆盖已存在文件。"""
    if plan.path.exists():
        raise LifecycleError(f"refuse to overwrite existing capture: {plan.path}")
    plan.path.parent.mkdir(parents=True, exist_ok=True)
    if plan.index_created:
        plan.index_path.parent.mkdir(parents=True, exist_ok=True)
        if plan.index_path.exists():
            raise LifecycleError(f"raw index appeared during capture: {plan.index_path}")
        plan.index_path.write_text(RAW_INDEX_CONTENT, encoding="utf-8")
    plan.path.write_text(plan.content, encoding="utf-8")


def _parse_note(path: Path) -> tuple[str, dict[str, object], str]:
    text = path.read_text(encoding="utf-8")
    try:
        fm = parse_frontmatter(text)
        split = split_frontmatter(text)
    except FrontmatterError as exc:
        raise LifecycleError(f"invalid frontmatter: {path}: {exc}") from exc
    if fm is None or split is None:
        raise LifecycleError(f"note has no frontmatter: {path}")
    return text, fm, split[1]


def _scalar(value: object) -> str:
    return value.strip() if isinstance(value, str) else ""


def _list(value: object) -> list[str]:
    if isinstance(value, list):
        return [x.strip() for x in value if isinstance(x, str) and x.strip()]
    if isinstance(value, str) and value.strip():
        return [value.strip()]
    return []


def _valid_date(value: str) -> bool:
    if not _DATE_RE.fullmatch(value):
        return False
    try:
        date.fromisoformat(value)
    except ValueError:
        return False
    return True


def _yaml_value(value: str | list[str]) -> str:
    if isinstance(value, list):
        return "[" + ", ".join(json.dumps(x, ensure_ascii=False) for x in value) + "]"
    if value in STATUSES:
        return value
    return json.dumps(value, ensure_ascii=False)


def _rewrite_frontmatter(text: str, updates: dict[str, str | list[str]]) -> str:
    """只改顶层 key,正文按 split_frontmatter 原样保留。"""
    split = split_frontmatter(text)
    if split is None:
        raise LifecycleError("note has no frontmatter")
    block, body = split
    lines = block.splitlines()
    remaining = dict(updates)
    out: list[str] = []
    for line in lines:
        match = _KEY_RE.match(line)
        if match and match.group(1) in remaining:
            key = match.group(1)
            out.append(f"{key}: {_yaml_value(remaining.pop(key))}")
        else:
            out.append(line)
    for key, value in remaining.items():
        out.append(f"{key}: {_yaml_value(value)}")
    return "---\n" + "\n".join(out) + "\n---\n" + body


def plan_promotion(
    path: Path,
    *,
    target_status: str,
    last_verified: str | None = None,
    evidence: list[str] | None = None,
) -> PromotionPlan:
    """验证晋级门禁并生成修改计划,不写磁盘。"""
    path = path.expanduser().resolve()
    if not path.is_file():
        raise LifecycleError(f"note path not found: {path}")
    target_status = target_status.strip().casefold()
    if target_status not in STATUSES or target_status == "unclassified":
        raise LifecycleError("target status must be raw, derived, or canonical")
    text, fm, _body = _parse_note(path)
    source_status = _scalar(fm.get("status")).casefold()
    if source_status not in STATUSES:
        raise LifecycleError(
            "source status must be explicitly declared; missing status is not inferred"
        )
    if _STATUS_ORDER[target_status] != _STATUS_ORDER[source_status] + 1:
        raise LifecycleError(
            f"invalid lifecycle transition: {source_status or 'undeclared'} -> {target_status}"
        )

    updates: dict[str, str | list[str]] = {"status": target_status}
    if target_status == "canonical":
        verified = (last_verified or "").strip()
        facts = [x.strip() for x in (evidence or []) if x.strip()]
        if not _valid_date(verified):
            raise LifecycleError("canonical promotion requires last_verified=YYYY-MM-DD")
        if not facts:
            raise LifecycleError("canonical promotion requires at least one evidence item")
        updates["last_verified"] = verified
        updates["evidence"] = facts
    return PromotionPlan(
        path=path,
        source_status=source_status,
        target_status=target_status,
        content=_rewrite_frontmatter(text, updates),
    )


def apply_promotion(plan: PromotionPlan) -> None:
    """应用 promotion 计划。"""
    plan.path.write_text(plan.content, encoding="utf-8")
