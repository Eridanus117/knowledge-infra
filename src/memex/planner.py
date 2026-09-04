"""Query planner —— 确定性本地分类。

query 语言分类逻辑:
zh_only_low_anchor = cjk_char_count>0 且 ascii_identifier_count==0
→ 中文低锚 query(靠语义吃饭),走 weighted-RRF(semantic 加权)+ semantic depth cap。
ascii_identifier:起始=ascii 字母/`_`,延续=ascii 字母数字/`_`/`:`/`-`。
"""

from __future__ import annotations

# is_cjk_char 的 range。
_CJK_RANGES = (
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x20000, 0x2A6DF),
    (0x2A700, 0x2B73F),
    (0x2B740, 0x2B81F),
    (0x2B820, 0x2CEAF),
    (0x2F800, 0x2FA1F),
)


def _is_cjk(ch: str) -> bool:
    cp = ord(ch)
    return any(lo <= cp <= hi for lo, hi in _CJK_RANGES)


def _is_ascii_ident_start(ch: str) -> bool:
    return ch.isascii() and (ch.isalpha() or ch == "_")


def _is_ascii_ident_continue(ch: str) -> bool:
    return ch.isascii() and (ch.isalnum() or ch in "_:-")


def cjk_char_count(query: str) -> int:
    return sum(1 for ch in query if _is_cjk(ch))


def ascii_identifier_count(query: str) -> int:
    """连续的 ascii-identifier run 数(逐字符扫描)。"""
    n = len(query)
    i = 0
    count = 0
    while i < n:
        if _is_ascii_ident_start(query[i]):
            count += 1
            i += 1
            while i < n and _is_ascii_ident_continue(query[i]):
                i += 1
        else:
            i += 1
    return count


def is_zh_low_anchor(query: str) -> bool:
    """中文低锚:有 CJK 且无 ascii identifier token。"""
    return cjk_char_count(query) > 0 and ascii_identifier_count(query) == 0


def _ascii_ident_runs(query: str) -> list[str]:
    runs: list[str] = []
    n, i = len(query), 0
    while i < n:
        if _is_ascii_ident_start(query[i]):
            start = i
            i += 1
            while i < n and _is_ascii_ident_continue(query[i]):
                i += 1
            runs.append(query[start:i])
        else:
            i += 1
    return runs


# 单字母大写(如句首 I/A)不算 ALL_CAPS 代码符号; 需 ≥2 字母。
_MIN_ALL_CAPS_LEN = 2


def _is_code_token(tok: str) -> bool:
    """代码符号 token:含 数字/`_`/`:`/`-`,或 camelCase,或 ALL_CAPS(len≥2)。"""
    if any(ch.isdigit() or ch in "_:-" for ch in tok):
        return True
    has_lower = any(ch.islower() for ch in tok)
    has_upper = any(ch.isupper() for ch in tok)
    if has_lower and has_upper:  # camelCase / PascalCase
        return True
    return has_upper and not has_lower and len(tok) >= _MIN_ALL_CAPS_LEN  # ALL_CAPS


def code_token_count(query: str) -> int:
    return sum(1 for tok in _ascii_ident_runs(query) if _is_code_token(tok))


# 选项 2 特性开关的检测器(eval-gated):强锚定 = 代码符号 token ≥ 阈值。
STRONG_ANCHOR_MIN_CODE_TOKENS = 2


def is_strongly_anchored(
    query: str, min_code_tokens: int = STRONG_ANCHOR_MIN_CODE_TOKENS
) -> bool:
    """强锚定/导航 query:代码符号 token 密集(object_id/uuid_v5/CHUNKED_EMBEDDING…)。"""
    return code_token_count(query) >= min_code_tokens
