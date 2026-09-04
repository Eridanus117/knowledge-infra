"""qdrant 写路径 sync: 两级 reuse + unit-mode 断言 + prune 三守卫。

流程: compile → 逐篇决策(① point_id 命中 + text_hash 同 → 只补 payload /
skip;② point_id miss → 按 (text_hash, embedding_profile) 复用现存向量 re-key, 零
embed;③ 都 miss → embed + upsert)→ prune diff(per-repo identity 前缀收窄 +
>50% 拒绝 + 默认 dry-run)。单篇失败记录 + 继续, 不 abort 全仓。
"""

from __future__ import annotations

import hashlib
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from itertools import batched
from pathlib import Path
from typing import Any

from memex.config import Settings, settings
from memex.indexing.compile import CompiledDoc, embed_text
from memex.indexing.pipeline import CompileOutput, compile_repo
from memex.indexing.qdrant import Qdrant, QdrantError
from memex.semantic import (
    CENTRAL_INDEX_PROFILE as INDEX_PROFILE,
)
from memex.semantic import (
    CENTRAL_POINT_KIND as POINT_KIND,
)
from memex.semantic import (
    EMBEDDING_PROFILE_ID,
    VECTOR_FIELD,
    embed_texts,
)

# 写路径产物口径: index_profile/point_kind 与读侧 semantic.py 同源(防口径漂移),
# embedding_profile 沿用现役(向量可复用)。
UNIT_MODE_WHOLE = "whole"
# point_id 固定 namespace: 由稳定字面量派生;此种子必须保持稳定——改它会导致
# 整库 point_id 全部变化,须 drop collection 全量重灌。
POINT_NAMESPACE = uuid.uuid5(uuid.NAMESPACE_URL, "memex:point-id:v1")

_RETRIEVE_BATCH = 256
_SCROLL_PAGE = 256

# 渲染时每个清单最多逐条列出多少项, 超出折叠成 "... 共 N 篇"。
_RENDER_LIST_CAP = 20

# 待删点占本仓现存点的比例超过此阈值即触发 mass-prune 守卫(需 --force 放行)。
_MASS_PRUNE_RATIO = 0.5
_PLAN_PROGRESS_EVERY = 25

ProgressFn = Callable[[str], None]

PAYLOAD_INDEX_FIELDS: tuple[str, ...] = (
    "domain_prefixes",
    "kind",
    "status",
    "point_kind",
    "text_hash",
    "embedding_profile",
    "index_profile",
    "unit_mode",
)


@dataclass(frozen=True)
class SyncMode:
    """单仓 sync 的写入策略。默认 dry-run(零写入);apply 真写, force 放行 mass-prune 守卫。"""

    apply: bool = False
    force: bool = False


# frozen → 可安全共享为默认参数 singleton(避开 B008 的可变默认陷阱)。
_DRY_RUN = SyncMode()


# payload 全字段(C5 + text_hash;比较/覆盖都以此为准)。
PAYLOAD_KEYS: tuple[str, ...] = (
    "identity",
    "domain",
    "domain_prefixes",
    "kind",
    "kind_explicit",
    "status",
    "keywords",
    "source_path",
    "source_hash",
    "compiled_hash",
    "index_profile",
    "embedding_profile",
    "point_kind",
    "commit_time",
    "unit_mode",
    "text_hash",
)


def point_id(identity: str, unit_mode: str = UNIT_MODE_WHOLE) -> str:
    """point_id = uuid5(固定 namespace, identity + ":" + unit-mode)(C6)。"""
    return str(uuid.uuid5(POINT_NAMESPACE, f"{identity}:{unit_mode}"))


def doc_embed_text(doc: CompiledDoc) -> str:
    """不含 title(M1: title 可回退文件名, 改名不该破 re-key 零 embed)。"""
    return embed_text(doc.description, doc.keywords, doc.body_text)


def doc_text_hash(doc: CompiledDoc) -> str:
    """text_hash = sha256(embed_text)——两级 reuse 的钥匙(C5 补列字段)。"""
    return hashlib.sha256(doc_embed_text(doc).encode("utf-8")).hexdigest()


def build_payload(doc: CompiledDoc, text_hash: str) -> dict[str, Any]:
    """C5 payload。None 字段(commit_time)不写入, 比较时按缺失对齐。"""
    payload: dict[str, Any] = {
        "identity": doc.identity,
        "domain": doc.domain,
        "domain_prefixes": doc.domain_prefixes,
        "kind": doc.kind,
        "kind_explicit": doc.kind_explicit,
        "keywords": doc.keywords,
        "source_path": doc.source_path,
        "source_hash": doc.source_hash,
        "compiled_hash": doc.compiled_hash,
        "index_profile": INDEX_PROFILE,
        "embedding_profile": EMBEDDING_PROFILE_ID,
        "point_kind": POINT_KIND,
        "unit_mode": UNIT_MODE_WHOLE,
        "text_hash": text_hash,
    }
    if doc.commit_time is not None:
        payload["commit_time"] = doc.commit_time
    # 缺 status 不写入 payload, 保持旧文档“未声明”而非伪造 canonical。
    if doc.status:
        payload["status"] = doc.status
    return payload


def _payload_view(payload: dict[str, Any]) -> dict[str, Any]:
    """按管理字段投影(忽略外来 key), None 与缺失等价 → 比较稳定。"""
    return {k: payload.get(k) for k in PAYLOAD_KEYS}


def ensure_collection(client: Qdrant, s: Settings = settings) -> None:
    """不存在则建中央 collection(named vector object/4096/Cosine)+ payload index。

    payload index 创建为幂等操作:既覆盖新 collection 初始化,也允许 apply 路径给
    存量 collection 补非破坏性索引。
    """
    name = s.central_collection
    if not client.collection_exists(name):
        client.create_collection(name, VECTOR_FIELD, s.embedding_dimensions)
    for fld in PAYLOAD_INDEX_FIELDS:
        client.create_payload_index(name, fld, "keyword")


@dataclass
class SyncReport:
    """单仓 sync 报告(dry-run 时各清单 = 计划动作)。"""

    repo: str
    collection: str
    dry_run: bool
    embedded: list[str] = field(default_factory=list)  # 新 embed(新增/内容变更)
    rekeyed: list[str] = field(default_factory=list)  # 复用现存向量 re-key
    payload_updated: list[str] = field(default_factory=list)
    unchanged: list[str] = field(default_factory=list)
    prune_candidates: list[str] = field(default_factory=list)
    pruned: list[str] = field(default_factory=list)
    prune_refused: str | None = None
    failures: list[tuple[str, str]] = field(default_factory=list)  # (identity, error)
    notes: list[str] = field(default_factory=list)
    error: str | None = None

    @property
    def needs_force(self) -> bool:
        """M-3: prune 守卫拒绝 = 需人工介入(--force);CLI 据此退 2(区别硬失败 1)。"""
        return self.prune_refused is not None

    def summary_line(self) -> str:
        if self.error:
            return f"{self.repo} → {self.collection}: ERROR — {self.error}"
        mode = "dry-run" if self.dry_run else "apply"
        return (
            f"{self.repo} → {self.collection} [{mode}]: "
            f"embed {len(self.embedded)}, re-key {len(self.rekeyed)}, "
            f"payload更新 {len(self.payload_updated)}, 未变 {len(self.unchanged)}, "
            f"prune候选 {len(self.prune_candidates)}, 失败 {len(self.failures)}"
        )

    def render(self) -> str:  # noqa: C901 — 报告渲染: 逐 section 拼装文本行, 分支多但线性、无嵌套逻辑
        lines = [f"--- sync {self.summary_line()}"]
        if self.error:
            return "\n".join(lines)
        if self.dry_run:
            lines.append("  (dry-run: 以下为计划, 未写入)")
        for label, items in (
            ("embed 新增/变更", self.embedded),
            ("re-key 复用向量", self.rekeyed),
            ("payload 更新", self.payload_updated),
        ):
            if items:
                lines.append(f"  {label} [{len(items)}]:")
                lines.extend(f"    - {x}" for x in items[:_RENDER_LIST_CAP])
                if len(items) > _RENDER_LIST_CAP:
                    lines.append(f"    ... 共 {len(items)} 篇")
        if self.prune_candidates:
            lines.append(f"  prune 候选 [{len(self.prune_candidates)}]:")
            lines.extend(f"    - {x}" for x in self.prune_candidates)
        if self.prune_refused:
            lines.append(f"  prune 拒绝: {self.prune_refused}")
        if self.pruned:
            lines.append(f"  pruned [{len(self.pruned)}]")
        if self.failures:
            lines.append(f"  失败 [{len(self.failures)}]:")
            lines.extend(f"    - {ident}: {err}" for ident, err in self.failures)
        for n in self.notes:
            lines.append(f"  note: {n}")
        return "\n".join(lines)


@dataclass
class _Plan:
    embed: list[tuple[CompiledDoc, str, dict[str, Any]]] = field(default_factory=list)
    rekey: list[tuple[str, dict[str, Any], list[float]]] = field(default_factory=list)
    set_payload: list[tuple[str, dict[str, Any]]] = field(default_factory=list)


def _assert_unit_mode(client: Qdrant, coll: str) -> str | None:
    """collection 内已有点的 unit_mode 必须与当前一致(C6 mode-switch 雷)。"""
    bad, _ = client.scroll(
        coll,
        flt={"must_not": [{"key": "unit_mode", "match": {"value": UNIT_MODE_WHOLE}}]},
        limit=1,
    )
    if bad:
        pl = bad[0].get("payload") or {}
        return (
            f"unit-mode 不一致: collection {coll} 内存在 unit_mode="
            f"{pl.get('unit_mode')!r} 的点(如 {pl.get('identity')!r}), 当前写入 mode="
            f"{UNIT_MODE_WHOLE!r}。切 unit-mode = 显式整库重建(删 collection 重灌), "
            "拒绝增量混跑。"
        )
    return None


def _scroll_repo_points(client: Qdrant, coll: str, repo: str) -> list[dict[str, Any]]:
    """本仓现存点(C7): server 侧只筛 point_kind/index_profile(无 repo facet),
    repo 收窄用 identity 前缀客户端过滤。只取 payload, 不取向量。"""
    flt = {
        "must": [
            {"key": "point_kind", "match": {"value": POINT_KIND}},
            {"key": "index_profile", "match": {"value": INDEX_PROFILE}},
        ]
    }
    prefix = f"{repo}:"
    out: list[dict[str, Any]] = []
    offset: Any = None
    while True:
        points, offset = client.scroll(coll, flt=flt, limit=_SCROLL_PAGE, offset=offset)
        for p in points:
            ident = (p.get("payload") or {}).get("identity")
            if isinstance(ident, str) and ident.startswith(prefix):
                out.append(p)
        if offset is None:
            break
    return out


def _find_reusable_vector(
    client: Qdrant, coll: str, text_hash: str
) -> list[float] | None:
    """两级 reuse ②: 按 (text_hash, embedding_profile) 找现存向量(C6)。"""
    points, _ = client.scroll(
        coll,
        flt={
            "must": [
                {"key": "text_hash", "match": {"value": text_hash}},
                {"key": "embedding_profile", "match": {"value": EMBEDDING_PROFILE_ID}},
            ]
        },
        limit=1,
        with_vector=True,
    )
    if not points:
        return None
    vec = points[0].get("vector")
    if isinstance(vec, dict):
        vec = vec.get(VECTOR_FIELD)
    return vec if isinstance(vec, list) else None


def sync_repo(  # noqa: C901, PLR0911, PLR0912, PLR0913, PLR0915 — compile→diff→embed→prune 守卫→落盘的单仓 sync 编排; dry-run/apply/force 多路径耦合, 强拆会割裂事务语义
    name: str,
    repo_root: Path,
    *,
    client: Qdrant | None = None,
    s: Settings = settings,
    mode: SyncMode = _DRY_RUN,
    legacy: bool = False,
    progress: ProgressFn | None = None,
) -> tuple[CompileOutput, SyncReport]:
    """compile + qdrant sync 一个源仓。默认 dry-run(零写入, 含不建 collection)。"""

    def emit(message: str) -> None:
        if progress is not None:
            progress(f"sync {name}: {message}")

    apply, force = mode.apply, mode.force
    client = client if client is not None else Qdrant(s)
    emit(f"compile start ({repo_root})")
    out = compile_repo(name, repo_root, legacy=legacy)
    coll = s.central_collection
    report = SyncReport(repo=out.canonical_repo, collection=coll, dry_run=not apply)
    emit(f"compile done: {len(out.docs)} doc(s), collection={coll}")

    if out.report.error or out.report.duplicate_error:
        report.error = out.report.error or out.report.duplicate_error
        emit(f"compile error: {report.error}")
        return out, report
    if not out.docs:
        report.notes.append("0 篇可索引 doc, 无事可做")
        emit("no indexable docs")
        return out, report

    try:
        emit("checking qdrant collection")
        exists = client.collection_exists(coll)
    except QdrantError as exc:
        report.error = f"qdrant 不可达: {exc}"
        emit(f"qdrant collection check failed: {exc}")
        return out, report

    if apply:
        try:
            action = "ensuring" if exists else "creating"
            emit(f"{action} qdrant collection/payload indexes")
            ensure_collection(client, s)
            exists = True
        except QdrantError as exc:
            report.error = f"建 collection/index 失败: {exc}"
            emit(f"qdrant collection/index ensure failed: {exc}")
            return out, report
    elif not exists:
        report.notes.append(
            f"collection {coll} 不存在; --apply 时将创建"
            f"(named vector {VECTOR_FIELD}/{s.embedding_dimensions}/Cosine "
            f"+ payload index {','.join(PAYLOAD_INDEX_FIELDS)})"
        )

    existing_by_id: dict[str, dict[str, Any]] = {}
    repo_points: list[dict[str, Any]] = []
    if exists:
        try:
            emit("checking unit_mode guard")
            mode_err = _assert_unit_mode(client, coll)
            if mode_err:
                report.error = mode_err
                emit(f"unit_mode guard failed: {mode_err}")
                return out, report
            ids = [point_id(d.identity) for d in out.docs]
            emit(f"retrieving existing qdrant points: {len(ids)} id(s)")
            for chunk in batched(ids, _RETRIEVE_BATCH):
                for p in client.retrieve(coll, list(chunk)):
                    existing_by_id[str(p.get("id"))] = p
            emit(f"scrolling repo points for prune/reuse scope: {out.canonical_repo}")
            repo_points = _scroll_repo_points(client, coll, out.canonical_repo)
            emit(
                f"qdrant read done: {len(existing_by_id)} direct hit(s), "
                f"{len(repo_points)} repo point(s)"
            )
        except QdrantError as exc:
            report.error = f"qdrant 读现状失败: {exc}"
            emit(f"qdrant read failed: {exc}")
            return out, report

    # ---- 逐篇决策(读阶段;单篇失败记录 + 继续) ----
    plan = _Plan()
    emit(f"planning doc actions: {len(out.docs)} doc(s)")
    for idx, doc in enumerate(out.docs, start=1):
        if idx == 1 or idx % _PLAN_PROGRESS_EVERY == 0 or idx == len(out.docs):
            emit(f"planning doc actions {idx}/{len(out.docs)}")
        try:
            pid = point_id(doc.identity)
            th = doc_text_hash(doc)
            payload = build_payload(doc, th)
            existing = existing_by_id.get(pid)
            if existing is not None:
                epl = existing.get("payload") or {}
                if epl.get("text_hash") == th:
                    if _payload_view(epl) != _payload_view(payload):
                        plan.set_payload.append((pid, payload))
                        report.payload_updated.append(doc.identity)
                    else:
                        report.unchanged.append(doc.identity)
                else:
                    plan.embed.append((doc, pid, payload))
                    report.embedded.append(doc.identity)
                continue
            vec = _find_reusable_vector(client, coll, th) if exists else None
            if vec is not None:
                plan.rekey.append((pid, payload, vec))
                report.rekeyed.append(doc.identity)
            else:
                plan.embed.append((doc, pid, payload))
                report.embedded.append(doc.identity)
        except QdrantError as exc:
            report.failures.append((doc.identity, str(exc)))
            emit(f"planning failed for {doc.identity}: {exc}")
    emit(
        "plan done: "
        f"embed={len(plan.embed)}, re-key={len(plan.rekey)}, "
        f"payload={len(plan.set_payload)}, failures={len(report.failures)}"
    )

    # ---- prune diff(C7 三守卫: per-repo 前缀已收窄 / >50% 拒绝 / dry-run 默认) ----
    compiled_ids = {d.identity for d in out.docs}
    stale = [
        p
        for p in repo_points
        if (p.get("payload") or {}).get("identity") not in compiled_ids
    ]
    report.prune_candidates = sorted(
        str((p.get("payload") or {}).get("identity")) for p in stale
    )
    refuse_prune = (
        bool(stale) and len(stale) > len(repo_points) * _MASS_PRUNE_RATIO and not force
    )

    def _refusal_msg(verb: str) -> str:
        # M2: 点明双份状态——新点照写、旧点未删, 运维要知道 collection 此刻是孤儿暂存态。
        new_points = len(report.embedded) + len(report.rekeyed)
        return (
            f"待删 {len(stale)} > 本仓现存 {len(repo_points)} 的 50%, 拒绝删除;"
            f"本轮{verb}写入 {new_points} 个新点、旧点未删, collection 暂存新旧双份/"
            "孤儿点, 确认无误后 --force 清理"
        )

    if not apply:
        if refuse_prune:
            report.prune_refused = _refusal_msg("将")
            emit("dry-run prune guard refused")
        emit("dry-run done")
        return out, report

    # ---- 写阶段 ----
    emit(f"writing payload updates: {len(plan.set_payload)}")
    for pid, payload in plan.set_payload:
        ident = str(payload.get("identity"))
        try:
            client.overwrite_payload(coll, payload, [pid])
        except QdrantError as exc:
            report.payload_updated.remove(ident)
            report.failures.append((ident, f"set payload: {exc}"))

    if plan.rekey:
        emit(f"writing re-key upserts: {len(plan.rekey)}")
        for pid, payload, vec in plan.rekey:
            ident = str(payload.get("identity"))
            try:
                client.upsert(
                    coll,
                    [{"id": pid, "vector": {VECTOR_FIELD: vec}, "payload": payload}],
                )
            except QdrantError as exc:
                report.rekeyed.remove(ident)
                report.failures.append((ident, f"re-key upsert: {exc}"))

    sync_embedding_url = s.effective_sync_embedding_url
    embed_batches = list(batched(plan.embed, max(1, s.embed_batch_size)))
    if plan.embed:
        emit(
            f"embedding {len(plan.embed)} doc(s) in {len(embed_batches)} batch(es); "
            f"timeout={s.embed_timeout_secs:g}s "
            "(set KB_SEARCH_EMBED_TIMEOUT_SECS to lower while diagnosing)"
        )
    for batch_idx, batch in enumerate(embed_batches, start=1):
        idents = [doc.identity for doc, _, _ in batch]
        try:
            emit(
                f"embedding batch {batch_idx}/{len(embed_batches)}: {len(batch)} doc(s)"
            )
            vectors = embed_texts(
                [doc_embed_text(doc) for doc, _, _ in batch],
                s,
                endpoint=sync_embedding_url,
                lane="sync",
            )
        except Exception as exc:  # embed 网络/服务错: 整批记失败, 继续下一批
            if len(batch) > 1:
                report.notes.append(f"embed batch {len(batch)} 失败, 已逐篇重试: {exc}")
                for doc, pid, payload in batch:
                    try:
                        emit(f"embedding single retry: {doc.identity}")
                        vec = embed_texts(
                            [doc_embed_text(doc)],
                            s,
                            endpoint=sync_embedding_url,
                            lane="sync",
                        )[0]
                    except Exception as single_exc:
                        report.embedded.remove(doc.identity)
                        report.failures.append((doc.identity, f"embed: {single_exc}"))
                        emit(f"embedding failed for {doc.identity}: {single_exc}")
                        continue
                    try:
                        client.upsert(
                            coll,
                            [
                                {
                                    "id": pid,
                                    "vector": {VECTOR_FIELD: vec},
                                    "payload": payload,
                                }
                            ],
                        )
                    except QdrantError as upsert_exc:
                        report.embedded.remove(doc.identity)
                        report.failures.append((doc.identity, f"upsert: {upsert_exc}"))
                        emit(f"upsert failed for {doc.identity}: {upsert_exc}")
                continue
            for ident in idents:
                report.embedded.remove(ident)
                report.failures.append((ident, f"embed: {exc}"))
                emit(f"embedding failed for {ident}: {exc}")
            continue
        points = [
            {"id": pid, "vector": {VECTOR_FIELD: vec}, "payload": payload}
            for (_, pid, payload), vec in zip(batch, vectors, strict=True)
        ]
        try:
            emit(f"upserting embedded batch {batch_idx}/{len(embed_batches)}")
            client.upsert(coll, points)
        except QdrantError as exc:
            for ident in idents:
                report.embedded.remove(ident)
                report.failures.append((ident, f"upsert: {exc}"))
                emit(f"upsert failed for {ident}: {exc}")

    if refuse_prune:
        # 写阶段后再生成文案: new_points 取实际写成数(失败的已移出清单)。
        report.prune_refused = _refusal_msg("已")
        emit("prune guard refused")
    elif stale:
        try:
            emit(f"deleting stale qdrant points: {len(stale)}")
            client.delete_points(coll, [str(p.get("id")) for p in stale])
            report.pruned = list(report.prune_candidates)
        except QdrantError as exc:
            report.failures.append(("<prune>", str(exc)))
            emit(f"prune delete failed: {exc}")

    emit(
        "apply done: "
        f"embed={len(report.embedded)}, re-key={len(report.rekeyed)}, "
        f"payload={len(report.payload_updated)}, pruned={len(report.pruned)}, "
        f"failures={len(report.failures)}"
    )
    return out, report


@dataclass
class RetiredQdrantPrune:
    """central collection 里 identity repo 段 ∉ active sources 的退役点清理结果。

    retired_repos = 退役的 repo 段名(去重排序);point_count = 待删/已删点数;
    deleted=True 已真删;refused = mass-delete 守卫拒绝文案(>50% 总点, 需 --force)。
    """

    retired_repos: list[str]
    point_count: int
    deleted: bool = False
    refused: str | None = None


def prune_retired_qdrant_points(
    client: Qdrant,
    collection: str,
    active_repos: set[str] | frozenset[str],
    *,
    apply: bool,
    force: bool = False,
) -> RetiredQdrantPrune:
    """清 central collection 里 identity repo 段 ∉ active sources 的退役点。

    pipeline.prune_retired_repos 的 qdrant 对偶: 改名后旧 identity(如
    project-a:→project-kb:)的点, 单仓 sync 的 per-repo prune 只按本仓 identity 前缀
    收窄 scope, 扫不到已退役 repo name 的点 → 永久残留成孤儿(doctor 报
    compiled_doc_missing)。本函数全量扫 collection, 按 identity 的 repo 段
    (split ':'[0])判退役整批删。守卫同 prune_retired_repos: 待删 >50% 总点拒绝
    (需 force);默认 dry-run(apply=False 只报告)。

    active_repos 须「全量 registry」仓名集合 —— 子集会把正常仓误判退役, 调用方须只在
    全量 sync 路径传入。collection 不存在 → noop。
    """
    if not client.collection_exists(collection):
        return RetiredQdrantPrune(retired_repos=[], point_count=0)
    retired_ids: list[str] = []
    retired_repos: set[str] = set()
    total = 0
    offset: Any = None
    while True:
        points, offset = client.scroll(collection, limit=_SCROLL_PAGE, offset=offset)
        for p in points:
            total += 1
            identity = (p.get("payload") or {}).get("identity", "")
            repo = identity.split(":", 1)[0] if identity else ""
            if repo and repo not in active_repos:
                retired_ids.append(str(p.get("id")))
                retired_repos.add(repo)
        if offset is None:
            break
    if not retired_ids:
        return RetiredQdrantPrune(retired_repos=[], point_count=0)
    if len(retired_ids) > total * _MASS_PRUNE_RATIO and not force:
        verb = "将" if not apply else ""
        return RetiredQdrantPrune(
            retired_repos=sorted(retired_repos),
            point_count=len(retired_ids),
            refused=(
                f"qdrant 待删退役点 {len(retired_ids)} > 现存 {total} 的 50%, "
                f"拒绝删除;退役点{verb}保留, 确认无误后 --force 清理"
            ),
        )
    result = RetiredQdrantPrune(
        retired_repos=sorted(retired_repos), point_count=len(retired_ids)
    )
    if not apply:
        return result
    for batch in batched(retired_ids, _SCROLL_PAGE):
        client.delete_points(collection, list(batch))
    result.deleted = True
    return result
