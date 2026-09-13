#!/usr/bin/env python3
"""Evaluate the anonymous synthetic retrieval fixture.

A gold fixture is intentionally required. The evaluator never chooses a local
or private default fixture, and its gate baseline is bound to the exact public
fixture content.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any


_PUBLIC_SOURCE_IDENTITIES = frozenset(
    {"source-alpha", "source-beta", "source-gamma", "source-delta"}
)
# The synthetic fixture has no source large enough to merit an individual group.
_BIG_SOURCES: frozenset[str] = frozenset()
_DEFAULT_RESULTS = Path(os.environ.get("KB_EVAL_RESULTS", "eval-results"))
_DEFAULT_BASELINE = Path(__file__).with_name("public-baseline.json")
_MARGIN = 0.01
_HYBRID_MARGIN = 0.01
_REQUIRED_GOLD_FIELDS = frozenset(
    {"fixture_id", "query", "gold_key", "qtype", "_slice"}
)


class EvaluationConfigError(ValueError):
    """The supplied public fixture or baseline is not usable."""


class EvaluationFailure(RuntimeError):
    """The evaluated index or its scores do not satisfy the public contract."""


def score(rows: list[dict[str, Any]]) -> dict[str, float | int]:
    n = len(rows)
    if not n:
        return {"n": 0}
    hits_at = {1: 0, 3: 0, 5: 0, 8: 0, 10: 0}
    mrr = 0.0
    misses = 0
    for row in rows:
        rank = row["rank"]
        if rank == 0:
            misses += 1
            continue
        mrr += 1.0 / rank
        for limit in hits_at:
            if rank <= limit:
                hits_at[limit] += 1
    return {
        "n": n,
        "no_hit": misses,
        **{f"gold_key@{limit}": round(hits / n, 4) for limit, hits in hits_at.items()},
        "mrr_gold": round(mrr / n, 4),
    }


def _fixture_hash(gold: list[dict[str, str]]) -> str:
    canonical = "\n".join(
        json.dumps(row, ensure_ascii=True, separators=(",", ":"), sort_keys=True)
        for row in gold
    )
    return hashlib.sha256(f"{canonical}\n".encode("utf-8")).hexdigest()


def _identity_from_gold_key(gold_key: str, row_number: int) -> str:
    parts = gold_key.split(":")
    if len(parts) != 3 or any(not part for part in parts):
        raise EvaluationConfigError(
            f"fixture record {row_number} has an invalid gold_key identity"
        )
    identity = parts[0]
    if identity not in _PUBLIC_SOURCE_IDENTITIES:
        raise EvaluationConfigError(
            f"fixture record {row_number} uses an unsupported source identity"
        )
    return identity


def _load_gold(path: Path) -> tuple[list[dict[str, str]], dict[str, str]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise EvaluationConfigError("cannot read the requested gold fixture") from exc

    gold: list[dict[str, str]] = []
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as exc:
            raise EvaluationConfigError(
                f"fixture record {line_number} is not valid JSON"
            ) from exc
        if not isinstance(record, dict):
            raise EvaluationConfigError(
                f"fixture record {line_number} must be a JSON object"
            )
        fields = set(record)
        missing = _REQUIRED_GOLD_FIELDS - fields
        unexpected = fields - _REQUIRED_GOLD_FIELDS
        if missing or unexpected:
            details = []
            if missing:
                details.append("missing " + ", ".join(sorted(missing)))
            if unexpected:
                details.append("unexpected " + ", ".join(sorted(unexpected)))
            raise EvaluationConfigError(
                f"fixture record {line_number} has invalid fields ({'; '.join(details)})"
            )
        if any(not isinstance(record[field], str) or not record[field] for field in fields):
            raise EvaluationConfigError(
                f"fixture record {line_number} has an empty or non-string required field"
            )
        _identity_from_gold_key(record["gold_key"], line_number)
        gold.append({field: record[field] for field in _REQUIRED_GOLD_FIELDS})

    if not gold:
        raise EvaluationConfigError("gold fixture contains no records")
    fixture_ids = {record["fixture_id"] for record in gold}
    if len(fixture_ids) != 1:
        raise EvaluationConfigError("gold fixture mixes fixture identities")
    return gold, {"id": fixture_ids.pop(), "sha256": _fixture_hash(gold)}


def _is_score(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and 0.0 <= float(value) <= 1.0
    )


def _load_baseline(path: Path, fixture: dict[str, str]) -> dict[str, Any]:
    try:
        baseline = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise EvaluationConfigError("cannot read the public baseline") from exc
    except json.JSONDecodeError as exc:
        raise EvaluationConfigError("public baseline is not valid JSON") from exc

    expected_fields = {"schema", "fixture_id", "fixture_sha256", "lanes", "hybrid_tracked"}
    if not isinstance(baseline, dict) or set(baseline) != expected_fields:
        raise EvaluationConfigError("public baseline has an invalid schema")
    if baseline["schema"] != "public-eval-baseline-v1":
        raise EvaluationConfigError("public baseline has an unsupported schema")
    if (
        baseline["fixture_id"] != fixture["id"]
        or baseline["fixture_sha256"] != fixture["sha256"]
    ):
        raise EvaluationConfigError(
            "public baseline does not match the supplied gold fixture"
        )

    lanes = baseline["lanes"]
    if not isinstance(lanes, dict) or set(lanes) != {"lexical", "hybrid"}:
        raise EvaluationConfigError("public baseline must define lexical and hybrid lanes")
    for lane, slices in lanes.items():
        if not isinstance(slices, dict) or not slices:
            raise EvaluationConfigError(f"public baseline {lane} slices are missing")
        if "_overall" not in slices or any(not _is_score(value) for value in slices.values()):
            raise EvaluationConfigError(f"public baseline {lane} has invalid scores")

    tracked = baseline["hybrid_tracked"]
    if (
        not isinstance(tracked, list)
        or not tracked
        or len(set(tracked)) != len(tracked)
        or any(not isinstance(key, str) or not key for key in tracked)
    ):
        raise EvaluationConfigError("public baseline has invalid hybrid tracked slices")
    for key in tracked:
        if key not in lanes["lexical"] or key not in lanes["hybrid"]:
            raise EvaluationConfigError(
                "public baseline hybrid tracked slices are incomplete"
            )
    return baseline


def _drift_check(engine: Engine, gold: list[dict[str, str]]) -> None:
    loaded = {document.object_key for index in engine.repos.values() for document in index.docs}
    missing = sorted({record["gold_key"] for record in gold} - loaded)
    if missing:
        raise EvaluationFailure(
            "drift check failed: required anonymous gold keys are absent from the loaded index: "
            + ", ".join(missing)
        )


def _search_keys_for_lane(
    args: argparse.Namespace, gold: list[dict[str, str]]
) -> Callable[[str, str], list[str]]:
    try:
        from memex.engine import Engine
        from memex.hybrid import HybridEngine
        from memex.semantic import SemanticEngine
    except ModuleNotFoundError as exc:
        raise EvaluationConfigError(
            "the selected lane requires an installed public memex engine"
        ) from exc

    if args.lane == "lexical":
        engine = Engine()
        print(f"[lexical] loaded indexes: {len(engine.repos)}", flush=True)
        _drift_check(engine, gold)

        def search_keys(query_text: str, source: str) -> list[str]:
            return [hit.object_key for hit in engine.search(query_text, k=10, repo=source)]

    elif args.lane == "semantic":
        _drift_check(Engine(), gold)
        semantic = SemanticEngine()

        def search_keys(query_text: str, source: str) -> list[str]:
            return [hit.object_key for hit in semantic.search(query_text, k=10, repo=source)]

    else:
        hybrid = HybridEngine(
            protect_anchored=args.protect,
            kind_prior=args.kind_prior,
        )
        print(
            f"[hybrid] protect_anchored={hybrid.protect_anchored} "
            f"kind_prior={hybrid.kind_prior}",
            flush=True,
        )
        _drift_check(hybrid.lexical, gold)

        def search_keys(query_text: str, source: str) -> list[str]:
            return [hit.object_key for hit in hybrid.search(query_text, k=10, repo=source)]

    return search_keys


def _evaluate(
    gold: list[dict[str, str]], search_keys: Callable[[str, str], list[str]]
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for record in gold:
        source = _identity_from_gold_key(record["gold_key"], len(rows) + 1)
        keys = search_keys(record["query"], source)
        rank = next(
            (position for position, key in enumerate(keys, 1) if key == record["gold_key"]),
            0,
        )
        rows.append(
            {
                **record,
                "rank": rank,
                "_repo_true": source,
                "_repo_group": source if source in _BIG_SOURCES else "long_tail",
            }
        )
    return rows


def _slices(rows: list[dict[str, Any]]) -> dict[str, dict[str, float | int]]:
    slices = {"_overall": score(rows)}
    for dimension in ("qtype", "_slice", "_repo_true", "_repo_group"):
        for value in sorted({row[dimension] for row in rows}):
            slices[f"{dimension}={value}"] = score(
                [row for row in rows if row[dimension] == value]
            )
    return slices


def _write_report(
    args: argparse.Namespace,
    fixture: dict[str, str],
    gold: list[dict[str, str]],
    slices: dict[str, dict[str, float | int]],
    out_path: Path,
    tracked: list[str],
) -> None:
    report = {
        "schema": "public-eval-report-v1",
        "lane": args.lane,
        "fixture_id": fixture["id"],
        "fixture_sha256": fixture["sha256"],
        "n_queries": len(gold),
        "slices": slices,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {out_path.name}\n")
    for key in tracked:
        if key in slices:
            values = slices[key]
            print(
                f"  {key:34} n={values.get('n'):3} "
                f"gold@10={values.get('gold_key@10')} "
                f"gold@5={values.get('gold_key@5')} "
                f"mrr={values.get('mrr_gold')} no_hit={values.get('no_hit')}"
            )


def _report_slices(
    path: Path, fixture: dict[str, str], expected_lane: str
) -> dict[str, dict[str, float | int]]:
    if not path.is_file():
        raise EvaluationFailure(
            f"hybrid relative gate requires {expected_lane} report {path.name}"
        )
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise EvaluationFailure(
            f"hybrid relative gate cannot read {expected_lane} report {path.name}"
        ) from exc
    if (
        not isinstance(report, dict)
        or report.get("schema") != "public-eval-report-v1"
        or report.get("lane") != expected_lane
        or report.get("fixture_id") != fixture["id"]
        or report.get("fixture_sha256") != fixture["sha256"]
        or not isinstance(report.get("slices"), dict)
    ):
        raise EvaluationFailure(
            f"hybrid relative gate found an incompatible {expected_lane} report"
        )
    return report["slices"]


def _metric(
    slices: dict[str, Any], key: str, metric: str, context: str
) -> float:
    values = slices.get(key)
    if not isinstance(values, dict) or metric not in values:
        raise EvaluationFailure(f"{context} is missing {metric} for slice {key}")
    value = values[metric]
    if not _is_score(value):
        raise EvaluationFailure(f"{context} has an invalid {metric} for slice {key}")
    return float(value)


def _check_hybrid_relative(
    slices: dict[str, dict[str, float | int]],
    baseline: dict[str, Any],
    fixture: dict[str, str],
    output_directory: Path,
    failed: list[str],
) -> None:
    lexical = _report_slices(
        output_directory / "lexical_goldset.json", fixture, "lexical"
    )
    semantic = _report_slices(
        output_directory / "semantic_goldset.json", fixture, "semantic"
    )
    print(
        "\n=== HYBRID GATE (hybrid >= best single lane per slice, margin",
        _HYBRID_MARGIN,
        ") ===",
    )
    for key in baseline["hybrid_tracked"]:
        hybrid_score = _metric(slices, key, "gold_key@10", "hybrid result")
        lexical_score = _metric(lexical, key, "gold_key@10", "lexical report")
        semantic_score = _metric(semantic, key, "gold_key@10", "semantic report")
        best = max(lexical_score, semantic_score)
        ok = hybrid_score >= best - _HYBRID_MARGIN
        print(
            f"  {'PASS' if ok else 'FAIL'}  {key:34} hybrid={hybrid_score} "
            f"lex={lexical_score} sem={semantic_score} max={best} "
            f"delta={round(hybrid_score - best, 4):+}"
        )
        if not ok:
            failed.append(key)


def _check_hybrid_floor(
    slices: dict[str, dict[str, float | int]], baseline: dict[str, Any], failed: list[str]
) -> None:
    print(
        "\n=== HYBRID ABSOLUTE FLOOR (public synthetic baseline, margin",
        _HYBRID_MARGIN,
        ") ===",
    )
    for key, expected in baseline["lanes"]["hybrid"].items():
        actual = _metric(slices, key, "gold_key@10", "hybrid result")
        ok = actual >= float(expected) - _HYBRID_MARGIN
        print(
            f"  {'PASS' if ok else 'FAIL'}  {key:34} got={actual} "
            f"base={expected} delta={round(actual - float(expected), 4):+}"
        )
        if not ok:
            failed.append(f"floor:{key}")


def _report_kind_prior(
    slices: dict[str, dict[str, float | int]],
    baseline: dict[str, Any],
    fixture: dict[str, str],
    default_file: Path,
) -> None:
    default = _report_slices(default_file, fixture, "hybrid")
    print("\n=== kind-prior versus protected hybrid per slice ===")
    regressed: list[str] = []
    improved = False
    for key in baseline["hybrid_tracked"]:
        for metric in ("gold_key@10", "gold_key@5", "mrr_gold"):
            candidate = _metric(slices, key, metric, "kind-prior result")
            current = _metric(default, key, metric, "protected hybrid report")
            delta = round(candidate - current, 4)
            marker = "up" if delta > 0 else ("down" if delta < 0 else "same")
            print(
                f"  {marker:4} {key:34} {metric:12} candidate={candidate} "
                f"default={current} delta={delta:+}"
            )
            if delta < -_HYBRID_MARGIN:
                regressed.append(f"{key}:{metric}")
            if delta > 0:
                improved = True
    verdict = "FLIP ON" if not regressed and improved else "KEEP OFF"
    print(f"\n  kind-prior promotion: {verdict}")


def _report_protection(
    slices: dict[str, dict[str, float | int]],
    baseline: dict[str, Any],
    fixture: dict[str, str],
    default_file: Path,
) -> None:
    default = _report_slices(default_file, fixture, "hybrid")
    print("\n=== protected hybrid versus default hybrid per slice ===")
    regressed: list[str] = []
    for key in baseline["hybrid_tracked"]:
        protected = _metric(slices, key, "gold_key@10", "protected hybrid result")
        current = _metric(default, key, "gold_key@10", "default hybrid report")
        delta = round(protected - current, 4)
        marker = "up" if delta > 0 else ("down" if delta < 0 else "same")
        print(
            f"  {marker:4} {key:34} protected={protected} default={current} "
            f"delta={delta:+}"
        )
        if delta < -_HYBRID_MARGIN:
            regressed.append(key)
    improved = any(
        _metric(slices, key, "gold_key@10", "protected hybrid result")
        > _metric(default, key, "gold_key@10", "default hybrid report")
        for key in baseline["hybrid_tracked"]
    )
    verdict = "FLIP ON" if not regressed and improved else "KEEP OFF"
    print(f"\n  protection promotion: {verdict}")


def _hybrid_gate(
    args: argparse.Namespace,
    slices: dict[str, dict[str, float | int]],
    baseline: dict[str, Any],
    fixture: dict[str, str],
    out_path: Path,
) -> int:
    hybrid_baseline = baseline["lanes"]["hybrid"]
    rebaseline = args.protect and float(hybrid_baseline["_overall"]) == 0.0
    if rebaseline:
        print("\n*** REBASELINE MODE: protected hybrid baseline is a placeholder ***")
    failed: list[str] = []
    output_directory = out_path.parent
    _check_hybrid_relative(slices, baseline, fixture, output_directory, failed)
    if args.protect and not rebaseline:
        _check_hybrid_floor(slices, baseline, failed)

    protected_file = output_directory / "hybrid_protected_goldset.json"
    if args.kind_prior and protected_file.exists():
        _report_kind_prior(slices, baseline, fixture, protected_file)
    elif args.kind_prior:
        print("\n*** kind-prior comparison skipped: protected hybrid report is absent ***")

    default_file = output_directory / "hybrid_goldset.json"
    if args.protect and not args.kind_prior and default_file.exists():
        _report_protection(slices, baseline, fixture, default_file)

    if failed:
        print(f"\nHYBRID GATE FAILED: {failed}")
        return 1
    print(
        "\nHYBRID GATE PASS — hybrid >= best single lane per slice"
        + (" and >= the public synthetic floor" if args.protect and not rebaseline else "")
    )
    return 0


def _lexical_gate(
    slices: dict[str, dict[str, float | int]], baseline: dict[str, Any]
) -> int:
    lexical_baseline = baseline["lanes"]["lexical"]
    if float(lexical_baseline["_overall"]) == 0.0:
        print("\n*** REBASELINE MODE: lexical baseline is a placeholder ***")
        return 0
    print("\n=== GATE (public synthetic baseline, margin", _MARGIN, ") ===")
    failed: list[str] = []
    for key, expected in lexical_baseline.items():
        actual = _metric(slices, key, "gold_key@10", "lexical result")
        ok = actual >= float(expected) - _MARGIN
        print(
            f"  {'PASS' if ok else 'FAIL'}  {key:34} got={actual} "
            f"base={expected} delta={round(actual - float(expected), 4):+}"
        )
        if not ok:
            failed.append(key)
    if failed:
        print(f"\nGATE FAILED: {failed}")
        return 1
    print("\nGATE PASS — lexical lane meets the public synthetic baseline")
    return 0


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--lane", choices=["lexical", "semantic", "hybrid"], default="lexical"
    )
    env_gold = os.environ.get("KB_EVAL_GOLDSET")
    parser.add_argument(
        "--gold",
        type=Path,
        default=Path(env_gold) if env_gold else None,
        metavar="PATH",
        help="anonymous synthetic gold fixture (or set KB_EVAL_GOLDSET)",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=_DEFAULT_BASELINE,
        metavar="PATH",
        help="synthetic baseline bound to the gold fixture",
    )
    parser.add_argument("--out", type=Path, default=None, metavar="PATH")
    parser.add_argument(
        "--validate-fixture",
        action="store_true",
        help="validate the anonymous gold fixture and its bound baseline without querying",
    )
    parser.add_argument(
        "--protect",
        action="store_true",
        help="hybrid: enable anchored lexical protection",
    )
    parser.add_argument(
        "--kind-prior",
        action="store_true",
        help="hybrid: enable the kind ranking prior",
    )
    args = parser.parse_args()
    if args.gold is None:
        parser.error("provide --gold PATH or set KB_EVAL_GOLDSET")
    return args


def main() -> int:
    args = _parse_args()
    try:
        gold, fixture = _load_gold(args.gold)
        baseline = _load_baseline(args.baseline, fixture)
        if args.validate_fixture:
            print(
                "fixture validation passed: "
                f"{fixture['id']} ({fixture['sha256']})"
            )
            return 0
        suffix = ""
        if args.lane == "hybrid":
            suffix += "_protected" if args.protect else ""
            suffix += "_kindprior" if args.kind_prior else ""
        out_path = args.out or _DEFAULT_RESULTS / f"{args.lane}{suffix}_goldset.json"
        search_keys = _search_keys_for_lane(args, gold)
        rows = _evaluate(gold, search_keys)
        slices = _slices(rows)
        _write_report(
            args,
            fixture,
            gold,
            slices,
            out_path,
            baseline["hybrid_tracked"],
        )

        if args.lane == "semantic":
            print("\n=== semantic lane: reports scores without an absolute gate ===")
            return 0
        if args.lane == "hybrid":
            return _hybrid_gate(args, slices, baseline, fixture, out_path)
        return _lexical_gate(slices, baseline)
    except EvaluationConfigError as exc:
        print(f"configuration error: {exc}", file=sys.stderr)
        return 2
    except EvaluationFailure as exc:
        print(f"evaluation failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
