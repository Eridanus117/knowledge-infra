"""读 compiled 目录(kb-note-v1)的 lexical 投影 —— read_from_central 的读路径。

与 artifacts.py(legacy .legacy-index loader)平行, 不动它。复用 artifacts.Doc 形状让
lexical.RepoIndex 零改动: object_key=identity, body = description + keywords +
body_text 顺序拼接(I7 的 desc/keywords 高 boost 是后续 eval 调优, 此处先保证
召回信号进索引;title 仍走 title 字段 boost 5)。
"""

from __future__ import annotations

import json
from pathlib import Path

from memex.artifacts import Doc
from memex.config import Settings, settings

COMPILED_SCHEMA = "kb-note-v1"


def load_compiled_docs(repo_dir: Path) -> list[Doc]:
    """加载一个仓的 compiled 目录 → 去重 Doc 列表(按 identity)。"""
    docs: dict[str, Doc] = {}
    for f in sorted(repo_dir.glob("*.json")):
        try:
            d = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue  # 单文件损坏不拖垮整仓(D4)
        if not isinstance(d, dict) or d.get("schema") != COMPILED_SCHEMA:
            continue
        identity = d.get("identity")
        if not isinstance(identity, str) or not identity or identity in docs:
            continue
        keywords = d.get("keywords") or []
        parts = [
            str(d.get("description") or ""),
            " ".join(k for k in keywords if isinstance(k, str)),
            str(d.get("body_text") or ""),
        ]
        docs[identity] = Doc(
            object_key=identity,
            title=str(d.get("title") or ""),
            body="\n\n".join(p for p in parts if p),
            path=str(d.get("source_path") or ""),
            kind=str(d.get("kind") or ""),
            domain_prefixes=tuple(
                p for p in (d.get("domain_prefixes") or []) if isinstance(p, str)
            ),
            keywords=tuple(k for k in keywords if isinstance(k, str)),
            # 缺字段默认 True: 老 compiled 产物重编译前不产生假 loud
            kind_explicit=bool(d.get("kind_explicit", True)),
            source_hash=d.get("source_hash") or None,
            compiled_hash=d.get("compiled_hash") or None,
        )
    return list(docs.values())


def load_compiled_corpus(s: Settings = settings) -> dict[str, list[Doc]]:
    """{repo: docs} —— 扫 compiled_dir 子目录(子目录名 = 规范仓名, sync 产出)。"""
    base = s.compiled_dir.expanduser()
    if not base.is_dir():
        return {}
    out: dict[str, list[Doc]] = {}
    for sub in sorted(p for p in base.iterdir() if p.is_dir()):
        docs = load_compiled_docs(sub)
        if docs:
            out[sub.name] = docs
    return out
