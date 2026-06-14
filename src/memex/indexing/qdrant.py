"""qdrant 写侧 client(C5/C6)—— 纯 stdlib http 薄封装。

WHY 不复用 semantic._post_json: 写侧要 GET/PUT/DELETE + HTTP 状态码语义(404 =
collection 不存在), 读侧只 POST;封成类也给测试留 fake 替身位(子类覆盖)。
只操作传入的 collection 名 —— 绝不触碰现役 per-root collection(调用方保证传中央名)。
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any

from memex.config import Settings, settings


class QdrantError(Exception):
    """qdrant HTTP/网络错误(含状态码与响应摘要)。"""

    def __init__(self, message: str, code: int | None = None) -> None:
        super().__init__(message)
        self.code = code


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
        req = urllib.request.Request(
            url, data=data, headers={"Content-Type": "application/json"}, method=method
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", "replace")[:500]
            raise QdrantError(
                f"{method} {path} → {exc.code}: {detail}", code=exc.code
            ) from exc
        except OSError as exc:
            raise QdrantError(f"{method} {path}: {exc}") from exc

    # ---- collection ----

    def collection_exists(self, name: str) -> bool:
        try:
            self._request("GET", f"/collections/{name}")
            return True
        except QdrantError as exc:
            if exc.code == 404:
                return False
            raise

    def create_collection(self, name: str, vector_name: str, dim: int) -> None:
        self._request(
            "PUT",
            f"/collections/{name}",
            {"vectors": {vector_name: {"size": dim, "distance": "Cosine"}}},
        )

    def create_payload_index(
        self, name: str, field: str, schema: str = "keyword"
    ) -> None:
        self._request(
            "PUT",
            f"/collections/{name}/index",
            {"field_name": field, "field_schema": schema},
        )

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
