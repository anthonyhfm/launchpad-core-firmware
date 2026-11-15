#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Start the remap config editor: remap-gui.py [config.toml]."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from remap_gui.gui import main  # noqa: E402

main()
