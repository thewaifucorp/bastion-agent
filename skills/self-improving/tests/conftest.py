"""conftest.py — add skills/self-improving to sys.path so bare imports work.

This mirrors the pythonpath entry in the root pyproject.toml
(skills/self-improving) so tests can use `from promotion import ...`
and `from mcp_server import ...` regardless of which rootdir pytest picks.
"""

import sys
from pathlib import Path

# Insert the skill root (parent of tests/) so promotion.py, mcp_server.py etc.
# are importable without package qualification (consistent with how
# test_self_improving_properties.py uses `from promotion import ...`).
_SKILL_ROOT = Path(__file__).parent.parent
if str(_SKILL_ROOT) not in sys.path:
    sys.path.insert(0, str(_SKILL_ROOT))
