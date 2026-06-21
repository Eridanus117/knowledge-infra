"""编译: note → compiled doc(C1 闸门 / C4 产物)。

C1: 有 frontmatter ⟺ 可索引;无 → loud-skip。新 5 字段(description/keywords/kind/
links/code)直接取;旧 v3 尽量挖等价字段(description/keywords/kind), 没有就空/默认。
kind 超出 enum → 降级为 note 并记入报告。

C4: compiled doc = identity/repo/domain/domain_prefixes/title/description/keywords/
kind/body_text/source_path/source_hash/compiled_hash/commit_time。落 JSON 到中央
数据目录(源仓零污染)。
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
from dataclasses import asdict, dataclass
from pathlib import Path

from memex.indexing.frontmatter import (
    FrontmatterError,
    parse_frontmatter,
    split_frontmatter,
)
from memex.indexing.scan import ScannedNote

# C4 编译产物 schema 名(payload/读路径共用此口径)。
SCHEMA = "kb-note-v1"

# 合法 kind enum(对齐 kb 工具 contract.KINDS)。超出 → 降级 note。
KINDS: frozenset[str] = frozenset(
    {"spec", "reference", "decision", "research", "runbook", "note", "index"}
)
DEFAULT_KIND = "note"

_H1_RE = re.compile(r"^#\s+(.+?)\s*$", re.MULTILINE)


@dataclass(frozen=True)
class CompiledDoc:
    """C4 编译产物。compiled_hash 不入序列化输入(它就是序列化产物的 hash)。"""

    identity: str
    repo: str
    domain: str
    domain_prefixes: list[str]
    title: str
    description: str
    keywords: list[str]
    kind: str
    kind_explicit: bool
    body_text: str
    source_path: str
    source_hash: str
    compiled_hash: str
    commit_time: str | None
    schema: str = SCHEMA


@dataclass(frozen=True)
class CompileResult:
    """单文件编译结果: 成功(doc)、loud-skip(无 frontmatter)、或 kind 降级记录。"""

    note: ScannedNote
    doc: CompiledDoc | None
    skipped_no_frontmatter: bool
    kind_downgraded_from: str | None  # 非 None = 原 kind 越界, 已降 note


def _sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _canonical_json(payload: dict[str, object]) -> str:
    """稳定规范化 JSON: 排序 key + 紧凑分隔 + 非 ASCII 保留。

    WHY 稳定: compiled_hash 算法跨进程/跨机一致, 切片② diff 据此判内容变没变。
    """
    return json.dumps(
        payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    )


def _title(body_text: str, note: ScannedNote) -> str:
    """title = 正文首个 H1 优先, 无则文件名(去 .md)。"""
    m = _H1_RE.search(body_text)
    if m:
        return m.group(1).strip()
    return note.path.stem


def _str(value: object) -> str:
    return value.strip() if isinstance(value, str) else ""


def _str_list(value: object) -> list[str]:
    if isinstance(value, list):
        return [x.strip() for x in value if isinstance(x, str) and x.strip()]
    if isinstance(value, str) and value.strip():
        return [value.strip()]
    return []


def _git_commit_time(repo_root: Path, source_path: str) -> str | None:
    """`git log -1 --format=%cI -- <path>` 的 commit 时间(ISO 8601)。

    git 不可用 / 非 git 仓 / untracked → None。
    WHY 本切片每次全量计算: 增量重编译时「内容未变保留旧 commit_time」是切片② diff
    的语义, 此切片不做增量, 故每篇都真查 git;此处留注说明边界。
    """
    try:
        proc = subprocess.run(
            [
                "git",
                "-C",
                str(repo_root),
                "log",
                "-1",
                "--format=%cI",
                "--",
                source_path,
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    out = proc.stdout.strip()
    return out or None


def embed_text(description: str, keywords: list[str], body: str) -> str:
    """embedding 输入文本 = description、keywords、body 顺序拼接。

    WHY 不含 title: title 无 H1 时回退文件名, 改名会变 text_hash → level-② reuse
    必 miss → 重 embed, 破坏「移动 = re-key, embedding 零调用」。title 仍进
    compiled doc / payload / lexical, 只是不进 embedding 文本。
    空字段跳过, 段间双换行分隔。
    """
    parts = [description.strip(), " ".join(keywords).strip(), body.strip()]
    return "\n\n".join(p for p in parts if p)


def compile_note(note: ScannedNote, repo_root: Path) -> CompileResult:
    """编译单个 ScannedNote → CompileResult(C1 闸门 + C4 产物)。"""
    raw = note.path.read_text(encoding="utf-8", errors="replace")
    try:
        fm = parse_frontmatter(raw)
    except FrontmatterError:
        fm = None  # 损坏 frontmatter 当无 frontmatter 处理(loud-skip)
    if fm is None:
        return CompileResult(
            note=note, doc=None, skipped_no_frontmatter=True, kind_downgraded_from=None
        )

    split = split_frontmatter(raw)
    body_text = split[1] if split is not None else raw
    body_text = body_text.strip("\n")

    description = _str(fm.get("description"))
    # tag 索引 case-fold:消除 acronym 大小写漂移('PM'/'pm' 不再分叉)。
    # 只 casefold keywords,不动共享 _str_list(links/code 等路径字段不能 casefold)。
    keywords = [k.casefold() for k in _str_list(fm.get("keywords"))]
    # 缺 kind 默认 note 会静默稀释 kind prior → 记录缺失供读路径 loud。
    # 越界但给了也算 explicit: 越界已有 kind_downgrades 单独 loud。
    kind_explicit = bool(_str(fm.get("kind")))
    raw_kind = _str(fm.get("kind")) or DEFAULT_KIND
    downgraded_from: str | None = None
    if raw_kind not in KINDS:
        downgraded_from = raw_kind
        kind = DEFAULT_KIND
    else:
        kind = raw_kind

    title = _title(body_text, note)
    source_hash = _sha256(raw)
    commit_time = _git_commit_time(repo_root, note.source_path)

    repo = note.identity.split(":", 1)[0]
    # compiled_hash = 除 compiled_hash 外全字段规范化 JSON 的 sha256。
    payload: dict[str, object] = {
        "identity": note.identity,
        "repo": repo,
        "domain": note.node.domain,
        "domain_prefixes": list(note.node.prefixes),
        "title": title,
        "description": description,
        "keywords": keywords,
        "kind": kind,
        "kind_explicit": kind_explicit,
        "body_text": body_text,
        "source_path": note.source_path,
        "source_hash": source_hash,
        "commit_time": commit_time,
        "schema": SCHEMA,
    }
    compiled_hash = _sha256(_canonical_json(payload))

    doc = CompiledDoc(
        identity=note.identity,
        repo=repo,
        domain=note.node.domain,
        domain_prefixes=list(note.node.prefixes),
        title=title,
        description=description,
        keywords=keywords,
        kind=kind,
        kind_explicit=kind_explicit,
        body_text=body_text,
        source_path=note.source_path,
        source_hash=source_hash,
        compiled_hash=compiled_hash,
        commit_time=commit_time,
    )
    return CompileResult(
        note=note,
        doc=doc,
        skipped_no_frontmatter=False,
        kind_downgraded_from=downgraded_from,
    )


def compile_legacy_note(note: ScannedNote, repo_root: Path) -> CompileResult:
    """编译 legacy/raw ScannedNote。

    只给 source-level legacy 模式使用：不要求 frontmatter，但会把 compiled doc
    明确标为 legacy/raw/unverified，避免旧材料被读路径误当成已整理 KB。
    """
    raw = note.path.read_text(encoding="utf-8", errors="replace")
    fm: dict[str, object] | None = None
    body_text = raw
    try:
        fm = parse_frontmatter(raw)
        split = split_frontmatter(raw)
        if split is not None:
            body_text = split[1]
    except FrontmatterError:
        fm = None
    body_text = body_text.strip("\n")

    title = _title(body_text, note)
    base_description = _str(fm.get("description")) if fm is not None else ""
    warning = "LEGACY RAW UNVERIFIED: 仅作低可信线索，使用前必须实地核验。"
    description_tail = base_description or title
    description = f"{warning} {description_tail}".strip()

    raw_keywords = _str_list(fm.get("keywords")) if fm is not None else []
    keywords: list[str] = []
    for kw in [*raw_keywords, "legacy", "raw", "unverified"]:
        folded = kw.casefold()
        if folded and folded not in keywords:
            keywords.append(folded)
    marked_body = f"{warning}\n\n{body_text}".strip()

    source_hash = _sha256(raw)
    commit_time = _git_commit_time(repo_root, note.source_path)
    repo = note.identity.split(":", 1)[0]
    payload: dict[str, object] = {
        "identity": note.identity,
        "repo": repo,
        "domain": note.node.domain,
        "domain_prefixes": list(note.node.prefixes),
        "title": title,
        "description": description,
        "keywords": keywords,
        "kind": DEFAULT_KIND,
        "kind_explicit": True,
        "body_text": marked_body,
        "source_path": note.source_path,
        "source_hash": source_hash,
        "commit_time": commit_time,
        "schema": SCHEMA,
    }
    compiled_hash = _sha256(_canonical_json(payload))

    doc = CompiledDoc(
        identity=note.identity,
        repo=repo,
        domain=note.node.domain,
        domain_prefixes=list(note.node.prefixes),
        title=title,
        description=description,
        keywords=keywords,
        kind=DEFAULT_KIND,
        kind_explicit=True,
        body_text=marked_body,
        source_path=note.source_path,
        source_hash=source_hash,
        compiled_hash=compiled_hash,
        commit_time=commit_time,
    )
    return CompileResult(
        note=note,
        doc=doc,
        skipped_no_frontmatter=False,
        kind_downgraded_from=None,
    )


_SLUG_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")


def safe_filename(identity: str) -> str:
    """compiled doc 文件名 = identity 安全编码 + .json。

    `:` 与 `/` 不能进文件名, 编成 `__` / `--`;再清理其余非安全字符。整体确定且可逆性
    无关(回读靠 identity 字段, 不靠文件名)。
    """
    s = identity.replace(":", "__").replace("/", "--")
    s = _SLUG_SAFE_RE.sub("_", s)
    return f"{s}.json"


def doc_to_json(doc: CompiledDoc) -> str:
    """compiled doc → 落盘 JSON 文本(规范化, 与 compiled_hash 算法同序)。"""
    return _canonical_json(asdict(doc))


def write_compiled(doc: CompiledDoc, out_dir: Path) -> Path:
    """落 compiled doc 到 out_dir/<safe(identity)>.json, 返回写入路径。"""
    out_dir.mkdir(parents=True, exist_ok=True)
    dest = out_dir / safe_filename(doc.identity)
    dest.write_text(doc_to_json(doc), encoding="utf-8")
    return dest
