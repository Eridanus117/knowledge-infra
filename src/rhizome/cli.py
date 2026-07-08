"""Rhizome CLI — `rhizome new` authors a compliant §存 note.

  rhizome new <topic> --description D --keywords k1,k2 [--kind K] [--domain PATH] \
                      [--links a,b] [--code repo/path,...] [--assets ns:id,...] \
                      [--body-file body.md | < body.md]

The note body comes from `--body-file PATH` (`-` = stdin) or, by default, stdin;
the CLI mechanically assembles the contract-compliant frontmatter (so the author
spends no tokens on YAML) and lands the file in the correct domain directory.
Authoring the body via `--body-file` (write it to a file first, then point here)
keeps it out of the shell's quoting layer and lets a flag typo be retried without
re-emitting the body — cheaper and safer than an inline heredoc.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import sys
import time
from datetime import datetime
from pathlib import Path

from orrery_heartbeat import check_update

from . import __version__, check, contract, doctor, sources, telemetry
from . import adopt as adopt_mod
from . import amend as amend_mod
from . import capture as capture_mod
from . import relocate as relocate_mod
from .contract import ContractError
from .telemetry import STDERR_CAP, STDOUT_CAP, Tee


class CliError(Exception):
    """User-facing CLI failure (bad path, existing file, no domain, ...)."""


def _split_csv(value: str | None) -> list[str]:
    if not value:
        return []
    return [part.strip() for part in value.split(",") if part.strip()]


def run_new(  # noqa: C901
    topic: str,
    *,
    description: str,
    keywords: list[str],
    kind: str = contract.DEFAULT_KIND,
    domain: str | None = None,
    links: list[str] | None = None,
    code: list[str] | None = None,
    assets: list[str] | None = None,
    body: str,
    cwd: Path,
) -> dict:
    """Author a note; return {path, domain, identity, kind}. Pure (no printing)."""
    topic = contract.validate_topic(topic)
    description = contract.validate_description(description)
    keywords = contract.validate_keywords(keywords)
    kind = contract.validate_kind(kind)
    assets = contract.validate_assets(assets or [])
    if assets and kind != "decision":
        raise CliError("--assets requires --kind decision")
    if not body.strip():
        raise CliError(
            "note body is empty; pipe the body via stdin or pass --body-file PATH"
        )

    repo_root = contract.find_repo_root(cwd)
    if repo_root is None:
        raise CliError(
            f"not inside a git repo (no .git found from {cwd}); "
            "cd into a KB source repo first (`rhizome domains` lists them)"
        )

    if domain:
        # Explicit --domain must name an exact domain dir (with its own INDEX.md);
        # never silently fall back to an ancestor domain (fail-loud,/3).
        domain_dir = (repo_root / domain.strip().strip("/")).resolve()
        if not _within(domain_dir, repo_root):
            raise CliError(f"--domain {domain!r} escapes the repo root")
        if not domain_dir.is_dir():
            raise CliError(
                f"--domain {domain!r}: no such directory {domain_dir}"
                f"{_domain_hint(repo_root, domain)}"
            )
        if not contract.has_index(domain_dir):
            raise CliError(
                f"--domain {domain!r}: no {contract.INDEX_FILENAME} at {domain_dir} "
                f"(a domain is a directory with its own {contract.INDEX_FILENAME})"
                f"{_domain_hint(repo_root, domain)}"
            )
    else:
        # Derive from cwd: nearest ancestor (inclusive) with an INDEX.md.
        domain_dir = contract.find_domain_dir(cwd.resolve(), repo_root)
        if domain_dir is None:
            raise CliError(
                f"no {contract.INDEX_FILENAME} found from {cwd} up to repo root; "
                f"create an {contract.INDEX_FILENAME} to define the domain, or pass "
                f"--domain{_domain_hint(repo_root)} "
                f"(see contracts/kb-source-repository-contract.md)"
            )

    # C2 node-chain口径 : the identity must match what the central
    # index actually stores; the physical path is only the file's location.
    domain_path = contract.derive_node_chain_domain(domain_dir, repo_root)
    if not domain_path:
        raise CliError(
            f"{contract.INDEX_FILENAME} at repo root is not a domain; domains are "
            "subdirectories (see contracts/kb-source-repository-contract.md)"
        )
    identity = contract.derive_identity(
        contract.repo_name(repo_root), domain_path, topic
    )

    dest = domain_dir / f"{topic}.md"
    if dest.exists():
        raise CliError(f"note already exists: {dest} (rhizome does not overwrite)")

    fm = contract.render_frontmatter(
        description=description,
        keywords=keywords,
        kind=kind,
        links=links,
        code=code,
        assets=assets,
    )
    dest.write_text(contract.render_note(fm, body), encoding="utf-8")

    return {
        "path": str(dest),
        "domain": domain_path,
        "identity": identity,
        "kind": kind,
    }


def _within(path: Path, root: Path) -> bool:
    path, root = path.resolve(), root.resolve()
    return path == root or root in path.parents


def _domain_hint(repo_root: Path, tried: str | None = None) -> str:
    """A parenthesized did-you-mean tail naming the repo's real domains.

    `--domain` is repo-relative and agents routinely mis-guess it (pass a
    workspace-relative path, or prepend the repo name → `source-delta/source-delta/docs`).
    Listing the repo's actual domains — and a close match for `tried` — turns a
    dead-end error into a fix. Best-effort: a discovery failure yields no tail.

    The list is the *physical* repo-relative path of each INDEX.md dir (what
    `--domain` actually consumes), NOT the C2 node-chain domain — those diverge
    when an intermediate dir lacks an INDEX.md (e.g. domain `a/c` physically at
    `a/b/c`), and suggesting the C2 form would just re-fail.
    """
    root = repo_root.resolve()
    try:
        domains = sorted(
            Path(d["index_path"]).parent.relative_to(root).as_posix()
            for d in sources.discover_domains(root)
        )
    except (OSError, ValueError, sources.SourcesError):
        return ""
    if not domains:
        return f" (no domains in {contract.repo_name(repo_root)} yet — add an INDEX.md)"
    parts = []
    if tried:
        probe = tried.strip().strip("/")
        close = difflib.get_close_matches(
            probe, domains, n=1, cutoff=0.5
        ) or difflib.get_close_matches(
            probe.rsplit("/", 1)[-1], domains, n=1, cutoff=0.6
        )
        if close:
            parts.append(f"did you mean {close[0]!r}?")
    parts.append("valid domains: " + ", ".join(domains))
    return " (" + "; ".join(parts) + ")"


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="rhizome", description="Rhizome KB §存 authoring CLI"
    )
    sub = parser.add_subparsers(dest="command", required=True)

    new = sub.add_parser(
        "new", help="author a compliant §存 note (body via --body-file or stdin)"
    )
    new.add_argument(
        "topic",
        help="kebab-case slug; becomes <topic>.md and the identity tail. "
        "SHOULD be ≤ 6 words — a slug only locates/distinguishes; semantics "
        "belong in --description/--keywords (kb-note-contract §3)",
    )
    new.add_argument(
        "--body-file",
        default=None,
        metavar="PATH",
        help="read the note body from PATH ('-' = stdin); default: stdin. "
        "Prefer a file: keeps the body out of the shell and lets a flag typo be "
        "retried without re-sending it.",
    )
    new.add_argument(
        "-d", "--description", required=True, help="one-line recall summary"
    )
    new.add_argument(
        "-k", "--keywords", required=True, help="comma-separated recall keywords"
    )
    new.add_argument(
        "--kind",
        default=contract.DEFAULT_KIND,
        help=f"one of {list(contract.KINDS)} (default: {contract.DEFAULT_KIND})",
    )
    new.add_argument(
        "--domain",
        default=None,
        help="domain dir (repo-relative); default: derive from cwd",
    )
    new.add_argument(
        "--links", default=None, help="comma-separated slugs of related notes"
    )
    new.add_argument("--code", default=None, help="comma-separated repo/path pointers")
    new.add_argument(
        "--assets",
        default=None,
        help="comma-separated delivery/runtime asset ids; requires --kind decision",
    )
    new.add_argument("--json", action="store_true", help="emit result as JSON")

    chk = sub.add_parser("check", help="validate notes against the §存 contract")
    chk.add_argument(
        "paths", nargs="*", help="note files to check (e.g. lefthook {staged_files})"
    )
    chk.add_argument(
        "--all", action="store_true", help="check every *.md under the repo root"
    )
    chk.add_argument(
        "-w",
        "--warnings",
        action="store_true",
        help="also show non-blocking warnings (soft signals)",
    )
    chk.add_argument(
        "--fix",
        action="store_true",
        help="strip killed/derived legacy fields in place (lossless), then check",
    )
    chk.add_argument(
        "--duplicate-domains",
        action="store_true",
        help="repo-level: FAIL if two physical INDEX.md paths derive the same C2 domain",
    )
    chk.add_argument(
        "--staged-frozen",
        action="store_true",
        help="repo-level: FAIL if staged deletions/renames touch a HEAD-frozen document",
    )
    chk.add_argument("--json", action="store_true", help="emit findings as JSON")

    dom = sub.add_parser(
        "domains", help="show the KB domain tree (registry + INDEX.md self-discovery)"
    )
    dom.add_argument(
        "repo",
        nargs="?",
        help="drill one source repo: show only its domains (full tree)",
    )
    dom.add_argument(
        "--compact",
        action="store_true",
        help="core sources inline; vertical sources listed by name "
        "(expand a vertical with `rhizome domains <repo>`)",
    )
    dom.add_argument(
        "--diff",
        action="store_true",
        help="completeness check: discovered domains vs actually-indexed",
    )
    dom.add_argument("--json", action="store_true", help="emit as JSON")

    adp = sub.add_parser(
        "adopt",
        help="adopt a repo as a KB source — registry row + INDEX skeleton + lefthook gate; idempotent",
    )
    adp.add_argument(
        "repo", help="path (contains /, or . / ..) or bare name under workspace_root"
    )
    adp.add_argument(
        "-d",
        "--description",
        default=None,
        help="one-line domain summary (required only when the repo has no domain yet)",
    )
    adp.add_argument(
        "-k",
        "--keywords",
        default=None,
        help="comma-separated recall keywords (required only when the repo has no domain yet)",
    )
    adp.add_argument("--json", action="store_true", help="emit result as JSON")

    doc = sub.add_parser(
        "doctor",
        help="KB pipeline-integrity checks: source repos (--sources) and/or the tool itself (--self)",
    )
    doc.add_argument(
        "--sources",
        action="store_true",
        help="check every registered KB source repo (gate present + gate resolvable + INDEX present)",
    )
    doc.add_argument(
        "--self",
        dest="self_check",
        action="store_true",
        help="check the tool's own gate template/probe consistency (catches a rename at the template)",
    )
    doc.add_argument(
        "--all",
        dest="all_checks",
        action="store_true",
        help="run both --sources and --self",
    )
    doc.add_argument("--json", action="store_true", help="emit report as JSON")

    amd = sub.add_parser(
        "amend",
        help="audited in-place edit of ONE frozen doc — provenance-tracked bypass of the "
        "frozen gate, replacing bare --no-verify",
    )
    amd.add_argument(
        "file",
        help="the frozen KB doc to amend (must be status: frozen / kind: decision in HEAD)",
    )
    amd.add_argument(
        "-m",
        "--reason",
        required=True,
        help="why this frozen doc is being amended — recorded in the commit trailer + ledger (the audit)",
    )
    amd.add_argument("--json", action="store_true", help="emit result as JSON")

    rel = sub.add_parser(
        "relocate",
        help="move a KB note to another domain/repo — identity recompute + "
        "frozen-aware provenance (dry-run by default)",
    )
    rel.add_argument(
        "source",
        nargs="?",
        help="the note to relocate (path); omit when using --batch",
    )
    rel.add_argument(
        "--to",
        default=None,
        help="target as <repo>:<domain>[:<slug>] (repo = registered KB source; "
        "slug optional — defaults to the source filename)",
    )
    rel.add_argument(
        "--batch",
        default=None,
        help="TOML plan file with [[move]] source=.. to=.. rows (one bucket per plan)",
    )
    rel.add_argument(
        "--apply",
        action="store_true",
        help="actually move + rewrite + record (default: dry-run, touches nothing)",
    )
    rel.add_argument("--json", action="store_true", help="emit result as JSON")

    cap = sub.add_parser(
        "capture",
        help="jot a fleeting thought: one timestamped line to the inbox (raw, "
        "out of KB boundary), triage later into `rhizome new` / source-beta",
    )
    cap.add_argument(
        "text",
        nargs="*",
        help="the thought (words are joined); omit to read one line from stdin",
    )
    cap.add_argument("--json", action="store_true", help="emit result as JSON")

    sub.add_parser(
        "stats",
        help="local usage telemetry: per-command count / p50·p95 latency / "
        "error rate (zero network)",
    )
    return parser


def _read_new_body(args) -> str | None:
    """Resolve the note body from --body-file (`-` = stdin) or, by default, stdin.

    Returns None on a *channel* error (bad file, or a TTY where a body was
    expected) — the caller maps that to exit 2. Emptiness is not judged here;
    run_new owns the empty-body contract (exit 1).
    """
    src = args.body_file
    if src is not None and src != "-":
        try:
            return Path(src).read_text(encoding="utf-8")
        except (OSError, ValueError) as exc:
            # OSError: missing / directory / permission. ValueError covers
            # UnicodeDecodeError (non-UTF8 body file) and embedded-null paths —
            # both channel errors (exit 2), not tracebacks. strerror is
            # OSError-only, so fall back to str(exc) for the ValueErrors.
            reason = getattr(exc, "strerror", None) or exc
            print(f"rhizome new: --body-file {src!r}: {reason}", file=sys.stderr)
            return None
    if sys.stdin.isatty():
        if src == "-":
            print(
                "rhizome new: --body-file - reads the body from stdin, "
                "but stdin is a TTY (pipe it, or give a file path)",
                file=sys.stderr,
            )
        else:
            print(
                "rhizome new: pipe the note body via stdin, or pass --body-file PATH",
                file=sys.stderr,
            )
        return None
    return sys.stdin.read()


def _cmd_new(args) -> int:
    body = _read_new_body(args)
    if body is None:
        return 2
    try:
        result = run_new(
            args.topic,
            description=args.description,
            keywords=_split_csv(args.keywords),
            kind=args.kind,
            domain=args.domain,
            links=_split_csv(args.links),
            code=_split_csv(args.code),
            assets=_split_csv(args.assets),
            body=body,
            cwd=Path.cwd(),
        )
    except (ContractError, CliError) as exc:
        print(f"rhizome new: {exc}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        print(f"created {result['path']}")
        print(f"  domain:   {result['domain']}")
        print(f"  identity: {result['identity']}")
        print(f"  kind:     {result['kind']}")
    excess = contract.slug_excess_words(args.topic)
    if excess:
        print(
            f"rhizome new: hint: slug is {excess} word(s) over the SHOULD ≤ "
            f"{contract.SLUG_WORDS_SHOULD_MAX} cap (kb-note-contract §3) — a slug "
            "only locates; put semantics in --description/--keywords, humans "
            "read the domain INDEX",
            file=sys.stderr,
        )
    return 0


def _read_capture_text(args) -> str | None:
    """Resolve the thought from positional args, else one line from stdin.

    Returns None on a channel error (nothing given and stdin is a TTY) — the
    caller maps that to exit 2. Emptiness of piped text is judged by run_capture.
    """
    if args.text:
        return " ".join(args.text)
    if sys.stdin.isatty():
        print(
            "rhizome capture: give the thought as arguments, or pipe it via stdin",
            file=sys.stderr,
        )
        return None
    return sys.stdin.read()


def _cmd_capture(args) -> int:
    text = _read_capture_text(args)
    if text is None:
        return 2
    try:
        result = capture_mod.run_capture(text)
    except capture_mod.CaptureError as exc:
        print(f"rhizome capture: {exc}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        print(f"captured → {result['path']}")
        print(f"  {result['line']}")
    return 0


def _collect_check_paths(args) -> list[Path] | None:
    if args.all:
        repo_root = contract.find_repo_root(Path.cwd())
        if repo_root is None:
            print("rhizome check: --all: not inside a git repo", file=sys.stderr)
            return None
        skip = {".git", ".venv", "__pycache__", "node_modules"}
        return sorted(p for p in repo_root.rglob("*.md") if skip.isdisjoint(p.parts))
    return [Path(p) for p in args.paths]


_FIXABLE_FIELDS = contract.KILLED_FIELDS | contract.DERIVED_FIELDS
# Max referencing docs to list inline before truncating with "...".
_EXTERNALIZE_REF_PREVIEW = 5


def _fix_paths(paths: list[Path]) -> tuple[dict[str, list[str]], list[str]]:
    """Strip killed/derived fields from each note in place.

    Returns ``(fixed, skipped_frozen)``: {path: removed fields} plus the paths
    left untouched because their HEAD version is frozen (--fix must
    never rewrite a read-only snapshot; supersede instead).

    Lossless (domain/title/verified are reconstructed; killed fields are noise),
    so it only rewrites files that actually carry such fields. `status` is
    value-aware: a non-frozen value (draft/living/...) is stripped, a legal
    `status: frozen` is kept.
    """
    fixed: dict[str, list[str]] = {}
    skipped_frozen: list[str] = []
    for p in paths:
        if not contract.is_note_location(p):
            continue  # outside any KB domain → not a note (e.g. PM issues/) → never rewrite
        if check.head_frozen(p):
            skipped_frozen.append(str(p))
            continue
        try:
            text = p.read_text(encoding="utf-8")
        except OSError:
            continue
        remove = _FIXABLE_FIELDS
        try:
            fm = contract.parse_frontmatter(text)
        except ContractError:
            continue  # malformed frontmatter — check still ERRORs on it
        if (
            fm
            and contract.STATUS_FIELD in fm
            and fm.get(contract.STATUS_FIELD) not in contract.STATUS_ALLOWED
        ):
            remove = remove | {contract.STATUS_FIELD}
        new_text, removed = contract.strip_fields(text, remove)
        if removed and new_text != text:
            p.write_text(new_text, encoding="utf-8")
            fixed[str(p)] = removed
    return fixed, skipped_frozen


def _duplicate_domain_results() -> tuple[dict[str, list], bool]:
    """Run the repo-level C2 duplicate-domain guard from cwd's repo root.

    Returns (results, any_error). On no repo / clean tree results is empty.
    """
    repo_root = contract.find_repo_root(Path.cwd())
    if repo_root is None:
        return {
            "<duplicate-domains>": [
                check.Finding(
                    check.ERROR, None, "--duplicate-domains: not inside a git repo"
                )
            ]
        }, True
    findings = check.duplicate_domain_findings(repo_root)
    if not findings:
        return {}, False
    return {"<duplicate-domains>": findings}, True


def _staged_frozen_results() -> tuple[dict[str, list], bool]:
    """Run the repo-level staged delete/rename frozen guard from cwd's repo root."""
    repo_root = contract.find_repo_root(Path.cwd())
    if repo_root is None:
        return {
            "<staged-frozen>": [
                check.Finding(
                    check.ERROR, None, "--staged-frozen: not inside a git repo"
                )
            ]
        }, True
    findings = check.staged_frozen_findings(repo_root)
    if not findings:
        return {}, False
    return {"<staged-frozen>": findings}, True


def _cmd_check(args) -> int:  # noqa: C901, PLR0912
    # --duplicate-domains / --staged-frozen are repo-level, not per-file; they
    # may run standalone (no paths) as the commit-hook repo guard, or alongside
    # the per-file checks (e.g. with --all).
    dup_results, dup_error = (
        _duplicate_domain_results()
        if (args.duplicate_domains or args.all)
        else ({}, False)
    )
    frozen_results, frozen_error = (
        _staged_frozen_results() if args.staged_frozen else ({}, False)
    )

    paths = _collect_check_paths(args)
    if paths is None:
        return 2
    if not paths and not args.duplicate_domains and not args.staged_frozen:
        print(
            "rhizome check: give note paths, --all, --duplicate-domains, or --staged-frozen",
            file=sys.stderr,
        )
        return 2

    if args.fix:
        fixed, skipped_frozen = _fix_paths(paths)
        if not args.json:
            for path in skipped_frozen:
                print(
                    f"{path}: skipped — HEAD version is frozen; --fix never rewrites a "
                    "read-only snapshot (supersede instead)",
                    file=sys.stderr,
                )
            for path, removed in sorted(fixed.items()):
                print(f"{path}: fixed — removed {', '.join(removed)}", file=sys.stderr)
            if fixed:
                print(
                    f"rhizome check --fix: stripped legacy fields in {len(fixed)} file(s)",
                    file=sys.stderr,
                )

    results: dict[str, list] = dict(dup_results)
    results.update(frozen_results)
    any_error = dup_error or frozen_error
    for p in paths:
        findings = check.check_path(p)
        if findings:
            results[str(p)] = findings
            any_error = any_error or check.has_errors(findings)

    for asset, refs in _asset_reuse_candidates(paths).items():
        results.setdefault("<asset-audit>", []).append(
            check.Finding(
                check.WARN,
                "assets",
                "externalize candidate: "
                f"{asset!r} appears in {len(refs)} decision docs "
                f"(threshold {contract.ASSET_EXTERNALIZE_REUSE_THRESHOLD}): "
                + ", ".join(refs[:_EXTERNALIZE_REF_PREVIEW])
                + (" ..." if len(refs) > _EXTERNALIZE_REF_PREVIEW else ""),
            )
        )

    if args.json:
        print(
            json.dumps(
                {
                    path: [
                        {"severity": f.severity, "field": f.field, "message": f.message}
                        for f in fs
                    ]
                    for path, fs in results.items()
                },
                ensure_ascii=False,
            )
        )
        return 1 if any_error else 0

    n_err = n_warn = 0
    for path, fs in results.items():
        for f in fs:
            if f.severity == check.ERROR:
                n_err += 1
            else:
                n_warn += 1
                if not args.warnings:
                    continue
            loc = f"{f.field}: " if f.field else ""
            print(f"{path}: {f.severity}: {loc}{f.message}", file=sys.stderr)
    if any_error:
        print(
            f"rhizome check: {n_err} error(s) across {len(paths)} file(s) — fix before commit",
            file=sys.stderr,
        )
    elif n_warn and args.warnings:
        print(f"rhizome check: ok ({n_warn} warning(s))", file=sys.stderr)
    return 1 if any_error else 0


def _asset_reuse_candidates(paths: list[Path]) -> dict[str, list[str]]:
    refs: dict[str, list[str]] = {}
    for p in paths:
        try:
            fm = contract.parse_frontmatter(p.read_text(encoding="utf-8"))
        except (OSError, ContractError):
            continue
        if not fm or fm.get("kind") != "decision":
            continue
        assets = fm.get("assets")
        if not isinstance(assets, list):
            continue
        for asset in assets:
            if isinstance(asset, str) and asset.strip():
                refs.setdefault(asset.strip(), []).append(str(p))
    return {
        asset: paths
        for asset, paths in sorted(refs.items())
        if len(paths) >= contract.ASSET_EXTERNALIZE_REUSE_THRESHOLD
    }


def _print_domain_node(node: dict) -> None:
    mark = "" if node["exists"] else "  (MISSING)"
    print(f"{node['name']}{mark}")
    if not node["domains"]:
        print("  (no domains — no INDEX.md)")
    for d in node["domains"]:
        desc = f" — {d['description']}" if d["description"] else ""
        print(f"  {d['domain']}{desc}")


def _print_domains_compact(tree: list[dict]) -> None:
    """core sources inline (full domains); vertical sources collapsed to a
    name list expandable via `rhizome domains <repo>`."""
    vertical = [n for n in tree if n.get("surface") != "core"]
    for node in (n for n in tree if n.get("surface") == "core"):
        _print_domain_node(node)
    if vertical:
        names = " · ".join(n["name"] for n in vertical)
        print()
        print(f"vertical(按需 `rhizome domains <repo>`):{names}")


def _cmd_domains(args) -> int:  # noqa: C901
    try:
        if args.diff:
            report = sources.diff()
            if args.json:
                print(json.dumps(report, ensure_ascii=False))
                return 0
            for r in report:
                st = r["status"]
                if st == "ok":
                    empties = r["empty_domains"]
                    tail = f"; empty: {empties}" if empties else ""
                    orph = (
                        f"; orphan notes: {r['orphan_notes']}"
                        if r["orphan_notes"]
                        else ""
                    )
                    print(
                        f"{r['name']}: {r['domains_indexed']}/{len(r['domains_discovered'])} domain(s) indexed{tail}{orph}"
                    )
                else:
                    hint = {
                        "missing-repo": "repo not found at path",
                        "no-domains": "no INDEX.md — add one to define a domain",
                        "not-indexed": "absent from central collection — wait for the sync cadence or run a manual sync",
                    }.get(st, st)
                    print(f"{r['name']}: {st} ({hint})")
            return 0

        tree = sources.build_tree()
        if args.repo:
            tree = [n for n in tree if n["name"] == args.repo]
            if not tree:
                print(f"rhizome domains: unknown source {args.repo!r}", file=sys.stderr)
                return 2
        if args.json:
            print(json.dumps(tree, ensure_ascii=False))
            return 0
        if args.compact and not args.repo:
            _print_domains_compact(tree)
        else:
            for node in tree:
                _print_domain_node(node)
    except sources.SourcesError as exc:
        print(f"rhizome domains: {exc}", file=sys.stderr)
        return 2
    return 0


def _cmd_adopt(args) -> int:
    try:
        result = adopt_mod.run_adopt(
            args.repo,
            description=args.description,
            keywords=_split_csv(args.keywords) or None,
            cwd=Path.cwd(),
        )
    except adopt_mod.AdoptUsageError as exc:
        print(f"rhizome adopt: {exc}", file=sys.stderr)
        return 2
    except sources.SourcesError as exc:
        print(f"rhizome adopt: {exc}", file=sys.stderr)
        return 2
    except (adopt_mod.AdoptError, ContractError) as exc:
        print(f"rhizome adopt: {exc}", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(result, ensure_ascii=False))
        return 0

    print(f"adopt {result['name']} ({result['repo']})")
    for s in result["steps"]:
        print(f"  {s['step']:<9} {s['status']:<8} {s['detail']}")
    for w in result["warnings"]:
        print(f"  WARN: {w}", file=sys.stderr)
    if result["stray_md"]:
        print(
            f"  note: {result['stray_md']} markdown file(s) outside any domain — rhizome does not index them; "
            f"move durable ones into a domain or add an INDEX.md"
        )
    changed_here = [
        s
        for s in result["steps"]
        if s["status"] == "changed" and s["step"] != "registry"
    ]
    reg_changed = any(
        s["step"] == "registry" and s["status"] == "changed" for s in result["steps"]
    )
    if changed_here:
        print(
            f"  next: commit the new files in {result['repo']} (the fresh hook validates them — natural closed loop)"
        )
    if reg_changed:
        print(
            f"  next: {result['registry']} updated — commit it to persist the registry change"
        )
    print("  next: indexing is picked up by your sync cadence automatically")
    return 0


def _print_sources_report(report: dict) -> None:
    print(f"doctor --sources ({report['registry']})")
    for r in report["sources"]:
        mark = "OK" if r["ok"] else "FAIL"
        print(f"  [{mark}] {r['name']} ({r['path']})")
        for c in r["checks"]:
            if c["status"] == doctor.PASS:
                sym, stream = "+", sys.stdout
            elif c["status"] == doctor.WARN:
                sym, stream = "!", sys.stdout
            else:
                sym, stream = "x", sys.stderr
            print(f"      {sym} {c['check']:<16} {c['detail']}", file=stream)
    n_fail = sum(1 for r in report["sources"] if not r["ok"])
    n_total = len(report["sources"])
    if report["ok"]:
        print(f"doctor --sources: ok ({n_total} source(s), all gate+index checks pass)")
    else:
        print(
            f"doctor --sources: {n_fail}/{n_total} source(s) FAIL — pipeline gate/index broken",
            file=sys.stderr,
        )


def _print_self_report(report: dict) -> None:
    mark = "OK" if report["ok"] else "FAIL"
    print(f"doctor --self [{mark}] (tool-chain gate template/probe consistency)")
    for c in report["checks"]:
        sym = "+" if c["status"] == doctor.PASS else "x"
        stream = sys.stdout if c["status"] == doctor.PASS else sys.stderr
        print(f"  {sym} {c['check']:<22} {c['detail']}", file=stream)
    if report["ok"]:
        print(
            f"doctor --self: ok ({len(report['checks'])} check(s), gate template ⇔ probe consistent)"
        )
    else:
        n_fail = sum(1 for c in report["checks"] if c["status"] != doctor.PASS)
        print(
            f"doctor --self: {n_fail}/{len(report['checks'])} check(s) FAIL — "
            "gate template/probe inconsistent (rename hazard)",
            file=sys.stderr,
        )


def _cmd_doctor(args) -> int:
    # Pick the mode(s). --all = both; otherwise honor each flag. At least one is
    # required so a future mode can join without silently changing the default.
    want_sources = args.sources or args.all_checks
    want_self = args.self_check or args.all_checks
    if not (want_sources or want_self):
        print(
            "rhizome doctor: pass --sources and/or --self (or --all)", file=sys.stderr
        )
        return 2

    sources_report = self_report = None
    if want_sources:
        try:
            sources_report = doctor.run_doctor()
        except sources.SourcesError as exc:
            print(f"rhizome doctor: {exc}", file=sys.stderr)
            return 2
    if want_self:
        self_report = doctor.run_self_check()

    if args.json:
        if want_sources and want_self:
            payload = {
                "sources": sources_report,
                "self": self_report,
                "ok": sources_report["ok"] and self_report["ok"],
            }
        else:
            payload = sources_report if want_sources else self_report
        print(json.dumps(payload, ensure_ascii=False))
        return 0 if payload["ok"] else 1

    ok = True
    if sources_report is not None:
        _print_sources_report(sources_report)
        ok = ok and sources_report["ok"]
    if self_report is not None:
        _print_self_report(self_report)
        ok = ok and self_report["ok"]
    return 0 if ok else 1


def _cmd_amend(args) -> int:
    try:
        result = amend_mod.run_amend(args.file, reason=args.reason, cwd=Path.cwd())
    except amend_mod.AmendUsageError as exc:
        print(f"rhizome amend: {exc}", file=sys.stderr)
        return 2
    except amend_mod.AmendError as exc:
        print(f"rhizome amend: {exc}", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(result, ensure_ascii=False))
        return 0

    print(f"amended frozen doc {result['rel']}")
    print(f"  commit: {result['commit']}")
    print(f"  reason: {result['reason']}")
    print(f"  trailer: {amend_mod.TRAILER_KEY}: {result['reason']}")
    print(f"  ledger: {result['ledger']}")
    return 0


def _print_relocate_move(entry: dict, *, applied: bool) -> None:
    plan = entry["plan"]
    head = "relocated" if applied else "relocate (dry-run)"
    print(f"{head}: {plan['old_rel']}")
    print(f"  from: {plan['old_identity']}")
    print(f"  to:   {plan['new_identity']}")
    print(f"  file: {plan['source']} -> {plan['dest']}")
    if plan["frozen"]:
        print(
            f"  frozen: content-preserving (sha256 {plan['content_hash'][:12]}; "
            "recorded in .relocate-ledger)"
        )
    nref = len(plan["references"])
    if nref:
        print(f"  references: {nref} note(s) link to the slug")
    if plan["slug_changed"]:
        print(
            f"  slug change: {Path(plan['source']).stem} -> "
            f"{Path(plan['dest']).stem} ({len(plan['rewrites'])} referrer(s) rewritten)"
        )
        for r in plan["frozen_blocked_refs"]:
            print(f"    frozen referrer (manual): {r['path']}", file=sys.stderr)
    elif plan["cross_repo_refs"]:
        names = ", ".join(r["path"] for r in plan["cross_repo_refs"][:5])
        print(
            f"    {len(plan['cross_repo_refs'])} will become cross-repo (WARN): {names}"
        )
    for w in plan["warnings"]:
        print(f"  warn: {w}", file=sys.stderr)
    if applied:
        for led in entry["applied"]["ledgers"]:
            print(f"  ledger: {led}")
        for p in entry["applied"]["rewritten"]:
            print(f"  rewrote: {p}")


def _cmd_relocate(args) -> int:
    if not args.batch and not (args.source and args.to):
        print(
            "rhizome relocate: give a source note and --to <repo>:<domain>, or --batch <plan>",
            file=sys.stderr,
        )
        return 2
    try:
        result = relocate_mod.run_relocate(
            source=args.source,
            to=args.to,
            batch=args.batch,
            apply=args.apply,
            cwd=Path.cwd(),
        )
    except relocate_mod.RelocateUsageError as exc:
        print(f"rhizome relocate: {exc}", file=sys.stderr)
        return 2
    except (relocate_mod.RelocateError, ContractError, sources.SourcesError) as exc:
        print(f"rhizome relocate: {exc}", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(result, ensure_ascii=False))
        return 0

    for entry in result["moves"]:
        _print_relocate_move(entry, applied=result["apply"])
    if not result["apply"]:
        print("  (nothing written — re-run with --apply to execute)")
    return 0


def main(argv: list[str] | None = None) -> int:  # noqa: PLR0911
    args = _build_parser().parse_args(argv)
    if args.command == "new":
        return _cmd_new(args)
    if args.command == "check":
        return _cmd_check(args)
    if args.command == "domains":
        return _cmd_domains(args)
    if args.command == "adopt":
        return _cmd_adopt(args)
    if args.command == "doctor":
        return _cmd_doctor(args)
    if args.command == "amend":
        return _cmd_amend(args)
    if args.command == "relocate":
        return _cmd_relocate(args)
    if args.command == "capture":
        return _cmd_capture(args)
    if args.command == "stats":
        print(telemetry.stats())
        return 0
    return 2  # argparse enforces subcommand presence; defensive


def run() -> None:
    check_update("rhizome", "source-gamma/rhizome")
    """Console-script entry: run the CLI under per-invocation telemetry capture.

    rhizome is argparse (not Typer/Click), so it can't use
    gnomon.run_instrumented's in-process Click wrapper — it uses the `record`
    posture, owning the capture loop here (mirrors source-beta.cli.run_wrapped).
    Telemetry is best-effort and must NEVER change the command's exit code.

    `main()` stays a pure dispatcher (tests call it directly); everything
    telemetry-related lives here.
    """
    argv = sys.argv[1:]
    start = time.monotonic()

    # command_path is pinned to the leading non-flag token, NOT derived by the
    # core from argv: rhizome has no global flags before the subcommand
    # (add_subparsers(required=True)), so argv[0] is reliably the verb; a leading
    # flag (`-h`/`--help`) legitimately yields an empty command_path. Deriving it
    # from raw argv would blank the verb if a global flag ever preceded it.
    verb = argv[0] if argv and not argv[0].startswith("-") else ""
    command_path = [verb] if verb else []
    rest = argv[1:] if verb else argv

    try:
        is_tty = sys.stdout.isatty()
    except Exception:
        is_tty = False

    # gnomon's Tee passes writes through to the real stream first and wraps all
    # byte accounting in try/except, so telemetry can never corrupt or fail the
    # command's own output (the "Tee 非 str 污染" P0). Do not hand-roll one.
    real_out, real_err = sys.stdout, sys.stderr
    out_tee, err_tee = Tee(real_out, STDOUT_CAP), Tee(real_err, STDERR_CAP)
    sys.stdout, sys.stderr = out_tee, err_tee

    exit_code: int = 0
    err_msg = ""
    try:
        exit_code = main(argv)
    except SystemExit as exc:  # argparse exits via SystemExit on -h / usage errors
        exit_code = (
            exc.code if isinstance(exc.code, int) else (0 if exc.code is None else 1)
        )
    except Exception as exc:
        print(f"rhizome: {exc}", file=sys.stderr)
        err_msg = str(exc)
        exit_code = 1
    finally:
        sys.stdout, sys.stderr = real_out, real_err

    telemetry.record(
        {
            "ts": datetime.now().astimezone().isoformat(timespec="milliseconds"),
            "pid": os.getpid(),
            "command_path": command_path,
            "args": list(rest),
            "cwd": str(Path.cwd()),
            "exit_code": exit_code if isinstance(exit_code, int) else 1,
            "duration_ms": int((time.monotonic() - start) * 1000),
            "out_bytes": out_tee.total,
            "stdout": out_tee.sample,
            "stderr": err_tee.sample,
            "err": err_msg,
            "version": __version__,
            "is_tty": is_tty,
            "is_ci": bool(os.environ.get("CI")),
            "meta": {"session": os.environ.get("CLAUDE_CODE_SESSION_ID", "")},
        }
    )
    sys.exit(exit_code)


if __name__ == "__main__":
    run()
