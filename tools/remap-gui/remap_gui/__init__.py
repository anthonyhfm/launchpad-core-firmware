# SPDX-License-Identifier: GPL-3.0-only
"""Config editor for the Launchpad Mini mk1 remap mode."""

import os
from pathlib import Path

DEFAULT_CONFIG = (
    Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    / "launchpad-remap"
    / "config.toml"
)
