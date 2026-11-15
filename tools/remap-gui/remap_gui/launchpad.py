# SPDX-License-Identifier: GPL-3.0-only
"""Protocol helpers for the Novation Launchpad mk1 / Launchpad Mini mk1.

Button addressing used throughout this project (all 0-based, origin top-left):

    top.0 .. top.7          round buttons above the grid   (CC 104..111)
    grid.R.C                8x8 square pads, R=row, C=col  (note 16*R + C)
    side.0 .. side.7        round buttons right of grid    (note 16*R + 8)

The device sends note_on velocity 127 on press and velocity 0 on release
(top row: CC value 127 / 0). LEDs are driven by sending the same address
back with a "velocity" that encodes colour:

    velocity = 16 * green + red + flags      (green, red: 0..3)
    flags    = 12 (copy + clear: normal, non double-buffered operation)
"""

from __future__ import annotations

import re

import mido

TOP_CC_BASE = 104
FLAGS_NORMAL = 12

GRID = [f"grid.{r}.{c}" for r in range(8) for c in range(8)]
SIDE = [f"side.{r}" for r in range(8)]
TOP = [f"top.{c}" for c in range(8)]
ALL_BUTTONS = TOP + GRID + SIDE

RESET = mido.Message("control_change", channel=0, control=0, value=0)
# B0 00 7D/7E/7F: all LEDs on at low/medium/full brightness (test mode).
ALL_ON_FULL = mido.Message("control_change", channel=0, control=0, value=0x7F)


def color(red: int, green: int) -> int:
    """Encode a red/green brightness pair (0..3 each) as LED velocity."""
    if not (0 <= red <= 3 and 0 <= green <= 3):
        raise ValueError(f"red/green must be 0..3, got red={red} green={green}")
    return 16 * green + red + FLAGS_NORMAL


OFF = color(0, 0)

COLORS: dict[str, int] = {
    "off": OFF,
    "red_low": color(1, 0),
    "red_mid": color(2, 0),
    "red": color(3, 0),
    "green_low": color(0, 1),
    "green_mid": color(0, 2),
    "green": color(0, 3),
    "amber_low": color(1, 1),
    "amber_mid": color(2, 2),
    "amber": color(3, 3),
    "yellow": color(2, 3),
    "lime": color(1, 3),
    "orange": color(3, 2),
    "orange_low": color(2, 1),
}

_BUTTON_RE = re.compile(r"^(?:(grid)[ .](\d)[ .](\d)|(top|side)[ .](\d))$")


def parse_button(name: str) -> str:
    """Normalise a button name ("grid.0.3", "Grid 0 3", "top.2") or raise."""
    m = _BUTTON_RE.match(name.strip().lower())
    if m:
        if m.group(1):
            r, c = int(m.group(2)), int(m.group(3))
            if r < 8 and c < 8:
                return f"grid.{r}.{c}"
        else:
            n = int(m.group(5))
            if n < 8:
                return f"{m.group(4)}.{n}"
    raise ValueError(
        f"invalid button name {name!r} (expected grid.ROW.COL, top.N or side.N, all 0..7)"
    )


def decode(msg: mido.Message) -> tuple[str, bool] | None:
    """Translate a raw device message into (button, pressed), or None."""
    if msg.type in ("note_on", "note_off"):
        row, col = divmod(msg.note, 16)
        if row > 7 or col > 8:
            return None
        button = f"side.{row}" if col == 8 else f"grid.{row}.{col}"
        return button, msg.type == "note_on" and msg.velocity > 0
    if msg.type == "control_change" and TOP_CC_BASE <= msg.control < TOP_CC_BASE + 8:
        return f"top.{msg.control - TOP_CC_BASE}", msg.value > 0
    return None


def led_message(button: str, velocity: int) -> mido.Message:
    """Build the device message that sets the LED of `button`."""
    kind, *idx = button.split(".")
    if kind == "top":
        return mido.Message(
            "control_change", channel=0, control=TOP_CC_BASE + int(idx[0]), value=velocity
        )
    if kind == "side":
        note = 16 * int(idx[0]) + 8
    else:
        note = 16 * int(idx[0]) + int(idx[1])
    return mido.Message("note_on", channel=0, note=note, velocity=velocity)
