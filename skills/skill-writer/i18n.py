"""Re-export i18n helpers from the shared skills/utils/i18n module."""
from __future__ import annotations

import sys
from pathlib import Path

_skills_dir = Path(__file__).resolve().parent.parent
if str(_skills_dir) not in sys.path:
    sys.path.insert(0, str(_skills_dir))

from utils.i18n import get_string, load_locale  # noqa: E402

__all__ = ["get_string", "load_locale"]
