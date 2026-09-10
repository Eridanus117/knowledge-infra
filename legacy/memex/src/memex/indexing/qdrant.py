"""qdrant 写侧 client(C5/C6)—— 纯 stdlib http 薄封装。

WHY 不复用 semantic._post_json: 写侧要 GET/PUT/DELETE + HTTP 状态码语义(404 =
collection 不存在), 读侧只 POST;封成类也给测试留 fake 替身位(子类覆盖)。
只操作传入的 collection 名 —— 绝不触碰现役 per-root collection(调用方保证传中央名)。
"""

from __future__ import annotations

import json
import os
import ssl
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from memex.config import Settings, settings

_HTTP_NOT_FOUND = 404
_TLS_RETRY_ATTEMPTS = 5


class QdrantError(Exception):
    """qdrant HTTP/网络错误(含状态码与响应摘要)。"""

    def __init__(self, message: str, code: int | None = None) -> None:
        super().__init__(message)
        self.code = code


def _internal_ssl_context(url: str) -> ssl.SSLContext | None:
    if urlparse(url).scheme != "https":
        return None
    ca = os.environ.get("KB_SEARCH_CA_BUNDLE")
    if not ca:
        return None
    p = Path(ca).expanduser()
    if not p.exists():
        return None
    ctx = ssl.create_default_context(cafile=str(p))
    if hasattr(ssl, "VERIFY_X509_PARTIAL_CHAIN"):
        ctx.verify_flags |= ssl.VERIFY_X509_PARTIAL_CHAIN
    if hasattr(ssl, "VERIFY_X509_STRICT"):
        ctx.verify_flags &= ~ssl.VERIFY_X509_STRICT
    return ctx


def _open_no_proxy(
    req: urllib.request.Request,
    *,
    timeout: float,
    context: ssl.SSLContext | None,
) -> Any:
    handlers: list[urllib.request.BaseHandler] = [urllib.request.ProxyHandler({})]
    if context is not None:
        handlers.append(urllib.request.HTTPSHandler(context=context))
    return urllib.request.build_opener(*handlers).open(req, timeout=timeout)


def _is_retryable_tls_error(exc: OSError) -> bool:
    return "CERTIFICATE_VERIFY_FAILED" in str(exc)


class Qdrant:
    """最小写侧操作面。所有方法可被测试 fake 覆盖。"""

    def __init__(self, s: Settings = settings) -> None:
        self.base = s.qdrant_url.rstrip("/")
        self.timeout = s.qdrant_timeout_secs

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        url = f"{self.base}{path}"
        data = json.dumps(body).encode("utf-8") if body is not None else None
        headers: dict[str, str] = {"Content-Type": "application/json"}
        token = os.environ.get("KB_SEARCH_BEARER_TOKEN")
        if token:
            headers["Authorization"] = f"Bearer {token}"
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        ctx = _internal_ssl_context(url)
        for attempt in range(_TLS_RETRY_ATTEMPTS):
            try:
                with _open_no_proxy(req, timeout=self.timeout, context=ctx) as resp:
                    return json.loads(resp.read())
            except urllib.error.HTTPError as exc:
                detail = exc.read().decode("utf-8", "replace")[:500]
                raise QdrantError(
                    f"{method} {path} → {exc.code}: {detail}", code=exc.code
                ) from exc
            except OSError as exc:
                if attempt < _TLS_RETRY_ATTEMPTS - 1 and _is_retryable_tls_error(exc):
                    time.sleep(0.2 * (attempt + 1))
                    continue
                raise QdrantError(f"{method} {path}: {exc}") from exc
        raise QdrantError(f"{method} {path}: retry exhausted")

    # ---- collection ----

    def collection_exists(self, name: str) -> bool:
        try:
            self._request("GET", f"/collections/{name}")
        except QdrantError as exc:
            if exc.code == _HTTP_NOT_FOUND:
                return False
            raise
        return True

    def create_collection(self, name: str, vector_name: str, dim: int) -> None:
        self._request(
            "PUT",
            f"/collections/{name}",
            {"vectors": {vector_name: {"size": dim, "distance": "Cosine"}}},
        )

    def create_payload_index(
        self, name: str, field: str, schema: str = "keyword"
    ) -> None:
        try:
            self._request(
                "PUT",
                f"/collections/{name}/index",
                {"field_name": field, "field_schema": schema},
            )
        except QdrantError as exc:
            msg = str(exc).lower()
            if exc.code in {400, 409} and "already" in msg and "exist" in msg:
                return
            raise

    def delete_collection(self, name: str) -> None:
        self._request("DELETE", f"/collections/{name}")

    # ---- points ----

    def retrieve(
        self, collection: str, ids: list[str], with_vector: bool = False
    ) -> list[dict[str, Any]]:
        resp = self._request(
            "POST",
            f"/collections/{collection}/points",
            {"ids": ids, "with_payload": True, "with_vector": with_vector},
        )
        return resp.get("result") or []

    def scroll(
        self,
        collection: str,
        flt: dict[str, Any] | None = None,
        limit: int = 100,
        offset: Any = None,
        with_vector: bool = False,
    ) -> tuple[list[dict[str, Any]], Any]:
        """一页 scroll → (points, next_page_offset);next 为 None 表示读完。"""
        body: dict[str, Any] = {
            "limit": limit,
            "with_payload": True,
            "with_vector": with_vector,
        }
        if flt is not None:
            body["filter"] = flt
        if offset is not None:
            body["offset"] = offset
        resp = self._request("POST", f"/collections/{collection}/points/scroll", body)
        result = resp.get("result") or {}
        return result.get("points") or [], result.get("next_page_offset")

    def upsert(self, collection: str, points: list[dict[str, Any]]) -> None:
        self._request(
            "PUT", f"/collections/{collection}/points?wait=true", {"points": points}
        )

    def overwrite_payload(
        self, collection: str, payload: dict[str, Any], ids: list[str]
    ) -> None:
        """整体覆盖 payload(PUT 语义)。WHY 不用 set(merge): set 不删旧 key,
        会让「字段变 None」类 diff 永远修不平 → 每轮重复 update。"""
        self._request(
            "PUT",
            f"/collections/{collection}/points/payload?wait=true",
            {"payload": payload, "points": ids},
        )

    def delete_points(self, collection: str, ids: list[str]) -> None:
        self._request(
            "POST",
            f"/collections/{collection}/points/delete?wait=true",
            {"points": ids},
        )
