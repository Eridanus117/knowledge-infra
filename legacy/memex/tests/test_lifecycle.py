"""raw capture 与 lifecycle promotion 的机械门禁。"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path

import pytest

from memex.indexing.lifecycle import (
    LifecycleError,
    apply_capture,
    apply_promotion,
    plan_capture,
    plan_promotion,
)


def test_capture_is_date_first_and_minimal(tmp_path: Path) -> None:
    plan = plan_capture(
        tmp_path,
        title="一个想法 / 先收下",
        body="原始内容",
        captured_at=datetime(2026, 8, 25, 9, 10, 11),
    )
    assert plan.path.relative_to(tmp_path).as_posix() == (
        "000-raw/2026/08/25/091011-一个想法-先收下.md"
    )
    assert "status: raw" in plan.content
    assert "kind:" not in plan.content
    apply_capture(plan)
    assert plan.path.is_file()
    assert plan.index_path.is_file()


def test_capture_never_overwrites_collision(tmp_path: Path) -> None:
    at = datetime(2026, 8, 25, 9, 10, 11)
    first = plan_capture(tmp_path, title="同名", body="a", captured_at=at)
    apply_capture(first)
    second = plan_capture(tmp_path, title="同名", body="b", captured_at=at)
    assert second.path.name.endswith("-02.md")


def test_promotion_requires_adjacent_states(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("---\nstatus: raw\n---\n\n# x\n", encoding="utf-8")
    with pytest.raises(LifecycleError, match="invalid lifecycle transition"):
        plan_promotion(path, target_status="canonical")
    derived = plan_promotion(path, target_status="derived")
    apply_promotion(derived)
    canonical = plan_promotion(
        path,
        target_status="canonical",
        last_verified="2026-08-25",
        evidence=["代码路径: foo/bar.py"],
    )
    apply_promotion(canonical)
    text = path.read_text(encoding="utf-8")
    assert "status: canonical" in text
    assert 'evidence: ["代码路径: foo/bar.py"]' in text


def test_canonical_requires_date_and_evidence(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("---\nstatus: derived\n---\nbody\n", encoding="utf-8")
    with pytest.raises(LifecycleError, match="last_verified"):
        plan_promotion(path, target_status="canonical", evidence=["x"])
    with pytest.raises(LifecycleError, match="evidence"):
        plan_promotion(path, target_status="canonical", last_verified="2026-08-25")


def test_missing_status_is_not_inferred(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("---\nkind: note\n---\nbody\n", encoding="utf-8")
    with pytest.raises(LifecycleError, match="explicitly declared"):
        plan_promotion(path, target_status="raw")
