from memex.config import Settings


def test_settings_defaults() -> None:
    s = Settings()
    assert s.debug is False


def test_sync_embedding_url_derives_from_query_lane() -> None:
    s = Settings(
        embedding_url="https://gateway.test/embedding-query/v1/embeddings",
    )

    assert (
        s.effective_sync_embedding_url
        == "https://gateway.test/embedding-sync/v1/embeddings"
    )


def test_sync_embedding_url_falls_back_to_embedding_url() -> None:
    s = Settings(
        embedding_url="<embedding-endpoint>/v1/embeddings",
    )

    assert s.effective_sync_embedding_url == "<embedding-endpoint>/v1/embeddings"


def test_sync_embedding_url_override_wins() -> None:
    s = Settings(
        embedding_url="https://gateway.test/embedding-query/v1/embeddings",
        sync_embedding_url="https://gateway.test/custom-sync/v1/embeddings",
    )

    assert (
        s.effective_sync_embedding_url
        == "https://gateway.test/custom-sync/v1/embeddings"
    )
