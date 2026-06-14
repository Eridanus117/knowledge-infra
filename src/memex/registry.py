"""Source registry —— 哪些仓是 KB 检索源。

真相收敛: 源仓清单由外部 kb-sources.toml 定义(authoring 工具侧),
memex 消费同一份数据文件;不可用时 fallback 内置 DEFAULT_SOURCE_REPOS
并日志标降级。只有真有 artifacts 目录的仓进入 active 集合
(没建索引的源仓自动跳过,不报错)。
"""

from __future__ import annotations

import logging
import os
import tomllib
from dataclasses import dataclass
from pathlib import Path

_WORKSPACE = Path(os.environ.get("KB_WORKSPACE_ROOT", str(Path.home() / "workspace")))
_ARTIFACTS_SUBPATH = Path(".legacy-index/index/artifacts")
# 源仓清单真相文件(authoring 工具侧产出);$KB_SOURCES 覆盖默认路径。
# 默认指向 workspace 根下的 kb-sources.toml;不存在则 fallback 内置默认(见下)。
_KB_SOURCES_DEFAULT = _WORKSPACE / "kb-sources.toml"

# fallback 清单:无 kb-sources.toml 时使用的中性默认。
# 实际部署请通过 kb-sources.toml(或 $KB_SOURCES)配置真实源仓;
# 此处仅给一个示例条目,缺省可留空 dict。
DEFAULT_SOURCE_REPOS: dict[str, Path] = {
    "docs": _WORKSPACE / "docs",
}
# legacy 标记的源仓名:命中要在消费时刻标「未核验」(迁移期用)。默认无。
DEFAULT_LEGACY_REPOS: frozenset[str] = frozenset()


@dataclass(frozen=True)
class SourceRegistry:
    """源仓清单 + 降级状态(M-2: 运行面要能看见降级, 不只埋在日志)。"""

    repos: dict[str, Path]
    degraded: bool
    reason: str | None  # degraded 时的原因, 否则 None
    # legacy = true 的源仓名: 命中要在消费时刻标「未核验」(迁移期内容未经实地核验)。
    legacy: frozenset[str] = frozenset()


def _illegal_name(name: str) -> bool:
    """非法 source name(含 / \\ .. 或前导 ~)= 路径穿越风险。"""
    return "/" in name or "\\" in name or ".." in name or name.startswith("~")


def load_source_registry() -> SourceRegistry:
    """读 kb-sources.toml(authoring 工具侧真相)→ SourceRegistry。

    toml 本身就是源仓清单的存储真相, 用 stdlib tomllib 读同一份。

    口径: $KB_SOURCES 覆盖 toml 路径, $KB_WORKSPACE_ROOT 覆盖 workspace 根。
    读路径 fail-safe(不 raise):缺失/损坏文件 → 降级内置 DEFAULT_SOURCE_REPOS;
    重复 name → 取首个跳过后者;非法 name(路径穿越判据见 _illegal_name)→
    跳过该条;均记 warning。
    """
    log = logging.getLogger(__name__)
    reg = Path(os.environ.get("KB_SOURCES", str(_KB_SOURCES_DEFAULT))).expanduser()
    try:
        data = tomllib.loads(reg.read_text(encoding="utf-8"))
        base = Path(
            os.environ.get(
                "KB_WORKSPACE_ROOT", data.get("workspace_root", "~/workspace")
            )
        ).expanduser()
        out: dict[str, Path] = {}
        legacy: set[str] = set()
        for entry in data.get("source", []):
            name = entry.get("name")
            if not name or not isinstance(name, str):
                continue
            if _illegal_name(name):
                log.warning(
                    "skipping illegal source name %r in %s (path traversal risk)",
                    name,
                    reg,
                )
                continue
            if name in out:
                log.warning(
                    "skipping duplicate source name %r in %s (first wins)", name, reg
                )
                continue
            path = entry.get("path")
            out[name] = Path(path).expanduser() if path else base / name
            if entry.get("legacy") is True:
                legacy.add(name)
        if not out:
            raise ValueError("no usable [[source]] entries")
        return SourceRegistry(
            repos=out, degraded=False, reason=None, legacy=frozenset(legacy)
        )
    except (OSError, tomllib.TOMLDecodeError, ValueError) as exc:
        # stdlib logging(非 structlog): PrintLogger 会绑死被 capture 的流, CLI 场景易炸。
        reason = f"{reg}: {exc}"
        log.warning("kb sources degraded to builtin defaults: %s", reason)
        return SourceRegistry(
            repos=dict(DEFAULT_SOURCE_REPOS),
            degraded=True,
            reason=reason,
            legacy=DEFAULT_LEGACY_REPOS,
        )


def load_source_repos() -> dict[str, Path]:
    """源仓清单 {name: path}(降级状态不关心时的便捷面;要状态用 load_source_registry)。"""
    return load_source_registry().repos


@dataclass(frozen=True)
class Source:
    name: str
    artifacts_dir: Path
    collection: str  # legacy 语义 lane 的 per-repo qdrant collection


def collection_for(repo: str) -> str:
    """repo → per-root hybrid collection(legacy 读路径用;中央 collection 走 Settings)。"""
    return f"{repo.replace('-', '_')}_hybrid_qwen3_v0"


def active_sources(repos: dict[str, Path] | None = None) -> list[Source]:
    """返回有 artifacts 的源仓(有索引才算 active)。清单默认取收敛后的真相。"""
    out: list[Source] = []
    for name, root in (repos if repos is not None else load_source_repos()).items():
        d = root / _ARTIFACTS_SUBPATH
        if d.is_dir() and next(d.glob("*.json"), None) is not None:
            out.append(
                Source(name=name, artifacts_dir=d, collection=collection_for(name))
            )
    return out
