"""Root conftest for proactive-engine — ensures layer0/layer1 are importable."""

import sys
from pathlib import Path

# Make layer0 and layer1 importable as top-level packages
_root = Path(__file__).parent
for sub in ("layer0", "layer1"):
    p = str(_root / sub)
    if p not in sys.path:
        sys.path.insert(0, p)
