from memex.config import Settings


def test_settings_defaults() -> None:
    s = Settings()
    assert s.debug is False
