"""Ensure skills/cal is importable without conflicts."""
import sys
from pathlib import Path

_skill_dir = Path(__file__).parent.parent
if str(_skill_dir) not in sys.path:
    sys.path.insert(0, str(_skill_dir))
