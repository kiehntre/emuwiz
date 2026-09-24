#!/usr/bin/env python3
"""Repository entry point for the read-only recovery inspector."""

from __future__ import annotations

import sys
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPOSITORY_ROOT / "tools/pending_recovery"))

from inspector import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
