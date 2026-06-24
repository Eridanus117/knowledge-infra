from __future__ import annotations

import urllib.error
import urllib.request
from typing import Any

from memex.config import Settings
from memex.indexing import qdrant
from memex.indexing.qdrant import Qdrant


class _Resp:
    def __enter__(self) -> "_Resp":
        return self

    def __exit__(self, *_args: object) -> None:
        return None

    def read(self) -> bytes:
        return b'{"result": true}'


def test_qdrant_request_uses_gateway_token_and_ssl_context(
    monkeypatch: Any,
) -> None:
    captured: dict[str, Any] = {}
    sentinel_context = object()

    def fake_open_no_proxy(
        req: urllib.request.Request, timeout: float, context: object | None = None
    ) -> _Resp:
        captured["url"] = req.full_url
        captured["headers"] = dict(req.header_items())
        captured["timeout"] = timeout
        captured["context"] = context
        return _Resp()

    monkeypatch.setenv("ORRERY_GATEWAY_TOKEN", "test-token")
    monkeypatch.setattr(
        "memex.indexing.qdrant._internal_ssl_context",
        lambda _url: sentinel_context,
    )
    monkeypatch.setattr(qdrant, "_open_no_proxy", fake_open_no_proxy)

    client = Qdrant(
        Settings(qdrant_url="https://example.test/qdrant", qdrant_timeout_secs=2.5)
    )

    assert client.collection_exists("kb_central") is True
    assert captured["url"] == "https://example.test/qdrant/collections/kb_central"
    assert captured["headers"]["Authorization"] == "Bearer test-token"
    assert captured["context"] is sentinel_context
    assert captured["timeout"] == 2.5


def test_qdrant_request_retries_transient_tls_verification(
    monkeypatch: Any,
) -> None:
    calls: list[str] = []

    def fake_open_no_proxy(
        req: urllib.request.Request, timeout: float, context: object | None = None
    ) -> _Resp:
        calls.append(req.full_url)
        if len(calls) == 1:
            raise urllib.error.URLError(
                "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: "
                "self-signed certificate"
            )
        return _Resp()

    monkeypatch.setattr(qdrant, "_open_no_proxy", fake_open_no_proxy)
    monkeypatch.setattr(qdrant.time, "sleep", lambda _seconds: None)

    client = Qdrant(
        Settings(qdrant_url="https://example.test/qdrant", qdrant_timeout_secs=2.5)
    )

    assert client.collection_exists("kb_central") is True
    assert len(calls) == 2


def test_internal_ssl_context_uses_ca_bundle_for_https(
    monkeypatch: Any, tmp_path: Any
) -> None:
    ca = tmp_path / "ca.pem"
    ca.write_text("test ca")
    sentinel_context = type("Ctx", (), {"verify_flags": 0})()
    captured: dict[str, Any] = {}

    def fake_default_context(*, cafile: str) -> object:
        captured["cafile"] = cafile
        return sentinel_context

    monkeypatch.setenv("KB_SEARCH_CA_BUNDLE", str(ca))
    monkeypatch.setattr(qdrant.ssl, "create_default_context", fake_default_context)

    assert (
        qdrant._internal_ssl_context("https://example.test/qdrant") is sentinel_context
    )
    assert captured["cafile"] == str(ca)


def test_internal_ssl_context_skips_plain_http(monkeypatch: Any, tmp_path: Any) -> None:
    ca = tmp_path / "ca.pem"
    ca.write_text("test ca")
    monkeypatch.setenv("KB_SEARCH_CA_BUNDLE", str(ca))

    assert qdrant._internal_ssl_context("<embedding-endpoint>") is None
