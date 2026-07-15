"""
Life Log Factory — creates the appropriate adapter based on DB_STRATEGY.

Usage:
    from skills.life_log.factory import Settings, create_adapter

    settings = Settings.from_env()
    adapter = create_adapter(settings)
    # adapter satisfies LifeLogProtocol regardless of DB_STRATEGY
"""

from __future__ import annotations

import logging
import os
from dataclasses import dataclass, field
from pathlib import Path

from .db.protocols import LifeLogProtocol

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Settings
# ---------------------------------------------------------------------------


@dataclass
class Settings:
    """
    Configuration for the life-log adapter factory.

    Fields:
        DB_STRATEGY: "sqlite" (default) or "supabase"
        SQLITE_PATH:  Path to the SQLite database file
        SUPABASE_URL: Supabase project URL (required when DB_STRATEGY=supabase)
        SUPABASE_KEY: Supabase anon/service key (required when DB_STRATEGY=supabase)
    """

    DB_STRATEGY: str = "sqlite"
    SQLITE_PATH: str = "db/life-log.db"
    SUPABASE_URL: str = ""
    SUPABASE_KEY: str = ""

    @classmethod
    def from_env(cls) -> "Settings":
        """Build Settings from environment variables, falling back to defaults."""
        return cls(
            DB_STRATEGY=os.getenv("DB_STRATEGY", "sqlite"),
            SQLITE_PATH=os.getenv("SQLITE_PATH", "db/life-log.db"),
            SUPABASE_URL=os.getenv("SUPABASE_URL", ""),
            SUPABASE_KEY=os.getenv("SUPABASE_KEY", ""),
        )


# ---------------------------------------------------------------------------
# Factory
# ---------------------------------------------------------------------------


def create_adapter(settings: Settings) -> LifeLogProtocol:
    """
    Return a LifeLogProtocol adapter based on *settings.DB_STRATEGY*.

    Strategy "sqlite" (default):
        Returns SQLiteLifeLogAdapter pointing at settings.SQLITE_PATH.
        The database file is created automatically on first write.

    Strategy "supabase":
        Returns SupabaseLifeLogAdapter using settings.SUPABASE_URL and
        settings.SUPABASE_KEY. Raises ValueError if either is missing.

    Args:
        settings: Configuration dataclass (use Settings.from_env() for production).

    Returns:
        An object satisfying LifeLogProtocol.

    Raises:
        ValueError: If DB_STRATEGY=supabase but URL or KEY are not set.
        ValueError: If DB_STRATEGY is an unknown value.
    """
    strategy = settings.DB_STRATEGY.lower().strip()

    if strategy == "sqlite":
        from .db.sqlite_adapter import SQLiteLifeLogAdapter

        adapter: LifeLogProtocol = SQLiteLifeLogAdapter(
            db_path=Path(settings.SQLITE_PATH)
        )
        logger.info(
            "Life-log adapter: SQLite (path=%s)", settings.SQLITE_PATH
        )
        return adapter

    if strategy == "supabase":
        if not settings.SUPABASE_URL or not settings.SUPABASE_KEY:
            raise ValueError(
                "DB_STRATEGY=supabase requires SUPABASE_URL and SUPABASE_KEY "
                "to be set in the environment."
            )
        from .db.supabase_adapter import SupabaseLifeLogAdapter

        adapter = SupabaseLifeLogAdapter(
            supabase_url=settings.SUPABASE_URL,
            supabase_key=settings.SUPABASE_KEY,
        )
        logger.info("Life-log adapter: Supabase (url=%s)", settings.SUPABASE_URL)
        return adapter

    raise ValueError(
        f"Unknown DB_STRATEGY: {settings.DB_STRATEGY!r}. "
        "Valid values are 'sqlite' and 'supabase'."
    )
