"""frontmatter 解析 —— 口径对齐 authoring 侧 frontmatter parser。

WHY 不直接用 pyyaml: 上游写入工具用的是一个 .so-free 的宽松 flat 解析器,
它对 bare scalar/flow list/block list/block scalar/comment 的处理与
yaml.v3 有意不同(惰性, loud-skip 兜底)。写路径必须读出与上游工具一致的
字段, 故在此 port 同一套语义, 而非引入 pyyaml 制造口径漂移。不是通用 YAML 引擎。
"""

from __future__ import annotations

import re

_FM_KEY_RE = re.compile(r"^([A-Za-z0-9_-]+):(.*)$")

# 最短带引号 scalar = 一对引号(开+闭)。
_MIN_QUOTED_LEN = 2


class FrontmatterError(ValueError):
    """frontmatter 结构损坏(开了 --- 却不闭合)。"""


def split_frontmatter(text: str) -> tuple[str, str] | None:
    """切分开头的 `---\\n…\\n---` 为 (去栅栏的块, 正文)。

    无开栏 → None(= 无 frontmatter, C1 闸门据此 loud-skip)。开了不闭合 → 抛错
    (损坏的 note, 不是无栏文件)。容忍 BOM 与 CRLF(对齐 yaml.v3)。
    """
    text = text.lstrip("﻿")
    lines = text.splitlines(keepends=True)
    if not lines or lines[0].strip() != "---":
        return None
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return "".join(lines[1:i]), "".join(lines[i + 1 :])
    raise FrontmatterError("unterminated frontmatter (opening --- with no closing ---)")


def _unquote(s: str) -> str:
    s = s.strip()
    if len(s) >= _MIN_QUOTED_LEN and s[0] == s[-1] and s[0] in "\"'":
        inner = s[1:-1]
        if s[0] == '"':
            inner = inner.replace('\\"', '"').replace("\\\\", "\\")
        return inner
    return s


def _parse_scalar(s: str) -> str | None:
    s = s.strip()
    if s == "" or s in ("~", "null", "Null", "NULL"):
        return None
    return _unquote(s)


def _parse_flow_list(s: str) -> list[str]:
    inner = s.strip()[1:-1].strip()  # 去掉外层 [ ]
    if not inner:
        return []
    items: list[str] = []
    cur = ""
    quote: str | None = None
    for ch in inner:
        if quote:
            cur += ch
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
            cur += ch
        elif ch == ",":
            items.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        items.append(cur)
    return [_unquote(x) for x in items]


def _strip_comment(s: str) -> str:
    """去掉未加引号的尾随 `# comment`(YAML: # 在行首或空白后)。"""
    out: list[str] = []
    quote: str | None = None
    for i, ch in enumerate(s):
        if quote:
            out.append(ch)
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
            out.append(ch)
        elif ch == "#" and (i == 0 or s[i - 1] in " \t"):
            break
        else:
            out.append(ch)
    return "".join(out).strip()


def parse_frontmatter(text: str) -> dict[str, object] | None:  # noqa: C901 — port 上游宽松 flat 解析器的逐 case 语义(见模块 docstring), 拆分会偏离对齐口径
    """解析 note frontmatter 为扁平 dict, 无 frontmatter 返回 None。

    结构损坏抛 FrontmatterError。支持 flow list(可跨行)、block list(`- x`)、
    block scalar(`|`/`>`, 折成单串)、引号/裸 scalar、null(`~`/`null`/空)、尾随
    `# 注释`。无法解析的顶层行直接跳过(宽松, index-time loud-skip 兜底)。
    """
    split = split_frontmatter(text)
    if split is None:
        return None
    block, _ = split
    out: dict[str, object] = {}
    lines = block.splitlines()
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        i += 1
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if line[:1] in (" ", "\t"):  # 游离缩进行, 非顶层 key
            continue
        m = _FM_KEY_RE.match(line)
        if not m:
            continue
        key, rest = m.group(1), m.group(2).strip()

        if rest and rest[0] in "|>":  # block scalar → 折叠后续缩进行
            buf: list[str] = []
            while i < n and (not lines[i].strip() or lines[i][:1] in (" ", "\t")):
                buf.append(lines[i].strip())
                i += 1
            out[key] = " ".join(x for x in buf if x) or None
            continue

        rest = _strip_comment(rest)

        if rest.startswith("["):  # flow list, 可能跨行
            while not rest.rstrip().endswith("]") and i < n:
                rest += " " + _strip_comment(lines[i].strip())
                i += 1
            out[key] = (
                _parse_flow_list(rest)
                if rest.rstrip().endswith("]")
                else _parse_scalar(rest)
            )
        elif rest == "":  # block list, 或空/null scalar
            block_items: list[str | None] = []
            while (
                i < n
                and lines[i][:1] in (" ", "\t")
                and lines[i].strip().startswith("- ")
            ):
                block_items.append(_parse_scalar(_strip_comment(lines[i].strip()[2:])))
                i += 1
            out[key] = block_items if block_items else None
        else:
            out[key] = _parse_scalar(rest)
    return out
