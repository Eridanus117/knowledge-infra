from __future__ import annotations

from pathlib import Path

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    """应用配置: env > .env > 默认值。"""

    model_config = SettingsConfigDict(
        env_prefix="KB_SEARCH_", env_file=".env", extra="ignore"
    )

    debug: bool = False

    # 写路径编译产物落点(中央数据目录, 源仓零污染)。各仓写到 <compiled_dir>/<repo>/。
    compiled_dir: Path = Path.home() / ".local/share/memex/compiled"

    # 写路径中央 collection(全新 collection, 绝不复用/修改 legacy per-root)。
    central_collection: str = "kb_central_qwen3_v1"
    # 写路径 embed 批大小(分批调 embedding 服务, 单批失败只损一批)。
    embed_batch_size: int = 8

    # 读路径双源 flag: False = legacy 行为(.legacy-index artifact +
    # per-root collection);True = lexical 读 compiled 目录、semantic 查中央
    # collection。此处只留开关供切换。
    read_from_central: bool = True

    # 语义 lane 外部服务配置(env 可覆盖)。默认值面向本机开发;
    # 生产部署通过 KB_SEARCH_* env vars 指到实际网关。
    # embedding_url 是查询路径;sync_embedding_url 是写入向量化路径。
    qdrant_url: str = "<embedding-endpoint>"
    embedding_url: str = "<embedding-endpoint>/v1/embeddings"
    sync_embedding_url: str | None = None
    embedding_model: str = "qwen3-embedding-8b"
    embedding_dimensions: int = 4096
    embed_timeout_secs: float = 600.0
    qdrant_timeout_secs: float = 30.0

    @property
    def effective_sync_embedding_url(self) -> str:
        """Write-path embedding endpoint, deriving sync lane from query lane when possible."""
        if self.sync_embedding_url:
            return self.sync_embedding_url
        return self.embedding_url.replace("/embedding-query/", "/embedding-sync/", 1)

    # 选项 2 特性开关(eval-gated):强锚定/导航 query 抬 lexical 权重,保护强
    # lexical 命中不被 RRF 融合挤出。A/B 严格 Pareto 改进 + 零回归后经评估后翻默认 ON。
    lexical_dependent_protection: bool = True

    # kind 排序 prior(post-fusion 按 kind 档位加伪 lane 票):
    # eval-gated,promotion gate 过(评测集上零回归 + 14 处改善)后经评估翻默认 ON。
    kind_prior: bool = True


settings = Settings()
