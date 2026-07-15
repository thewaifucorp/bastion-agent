"""Configure sys.path so output_validator is importable from tests."""

import sys
from pathlib import Path

# Add skills/output-validator/ to sys.path so `import output_validator` works.
# This file is at skills/output-validator/tests/conftest.py
# The output_validator package is at skills/output-validator/output_validator/
_skill_root = Path(__file__).parent.parent  # skills/output-validator/
if str(_skill_root) not in sys.path:
    sys.path.insert(0, str(_skill_root))
