# SPDX-License-Identifier: GPL-3.0-only
"""Configuration loading and validation (TOML)."""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

import mido

from . import launchpad as lp

GLOBAL_SCOPE = "*"

ACTION_KEYS = ("note", "cc", "program", "send", "page")
MAPPING_KEYS = set(ACTION_KEYS) | {
    "channel",
    "velocity",
    "value",
    "off_value",
    "mode",
    "release",
    "color",
    "off_color",
    "label",
}
LAYOUT_KEYS = {
    "type",
    "start",
    "row_step",
    "col_step",
    "origin",
    "channel",
    "velocity",
    "mode",
    "color",
    "off_color",
}
SECTION_KEYS = {
    "device": {"match"},
    "output": {"name", "channel", "feedback"},
    "behavior": {
        "start_page",
        "unmapped",
        "local_feedback",
        "default_color",
        "default_off_color",
        "middle_c_octave",
    },
}
TOP_LEVEL_KEYS = set(SECTION_KEYS) | {"colors", "global", "pages"}

_NOTE_RE = re.compile(r"^([A-Ga-g])([#b]?)(-?\d+)$")
_SEMITONES = {"c": 0, "d": 2, "e": 4, "f": 5, "g": 7, "a": 9, "b": 11}


class ConfigError(Exception):
    pass


@dataclass
class Mapping:
    """What one button does on one page."""

    scope: str  # page name, or GLOBAL_SCOPE for [global] mappings
    mode: str  # "momentary" | "toggle"
    on_msgs: list[mido.Message]
    off_msgs: list[mido.Message]
    color: int
    off_color: int
    page: str | None = None  # set for page-switch buttons
    # Host feedback: (type, channel, number) and the value meaning "off".
    feedback_key: tuple[str, int, int] | None = None
    feedback_off_value: int = 0
    label: str = ""
    # Structured form, used to build the on-device (CoreFW Remap mode) table.
    action: str = ""  # note | cc | program | send | page
    channel: int = 0  # 0-based
    number: int = 0  # note / cc / program number
    on_value: int = 0  # note velocity or cc value
    off_value: int = 0  # cc off value

    def describe(self) -> str:
        if self.page:
            return f"page -> {self.page}"
        parts = [" ".join(m.hex() for m in self.on_msgs) or "-"]
        if self.off_msgs:
            parts.append("/ " + " ".join(m.hex() for m in self.off_msgs))
        text = f"{self.mode}: " + " ".join(parts)
        return f"{text}  ({self.label})" if self.label else text


@dataclass
class Config:
    device_match: str = "Launchpad Mini"
    port_name: str = "Launchpad Remap"
    host_feedback: bool = True
    start_page: str = ""
    unmapped: str = "ignore"  # "ignore" | "passthrough"
    local_feedback: bool = True
    pages: dict[str, dict[str, Mapping]] = field(default_factory=dict)
    # (type, channel, number) -> [(scope, button, mapping)]
    feedback_index: dict[tuple[str, int, int], list[tuple[str, str, Mapping]]] = field(
        default_factory=dict
    )


def _check_keys(where: str, table: dict, allowed: set[str]) -> None:
    unknown = set(table) - allowed
    if unknown:
        raise ConfigError(f"{where}: unknown key(s) {sorted(unknown)}; allowed: {sorted(allowed)}")


def _int(where: str, value, lo: int, hi: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not lo <= value <= hi:
        raise ConfigError(f"{where}: expected integer {lo}..{hi}, got {value!r}")
    return value


class _Parser:
    def __init__(self, data: dict):
        self.data = data
        output = data.get("output", {})
        behavior = data.get("behavior", {})
        self.default_channel = _int("output.channel", output.get("channel", 1), 1, 16) - 1
        self.middle_c_octave = _int(
            "behavior.middle_c_octave", behavior.get("middle_c_octave", 4), -2, 8
        )
        self.colors = dict(lp.COLORS)
        for name, spec in data.get("colors", {}).items():
            self.colors[name.lower()] = self.color(f"colors.{name}", spec)
        self.default_color = self.color(
            "behavior.default_color", behavior.get("default_color", "green")
        )
        self.default_off_color = self.color(
            "behavior.default_off_color", behavior.get("default_off_color", "off")
        )

    # -- value parsers -----------------------------------------------------

    def color(self, where: str, spec) -> int:
        if isinstance(spec, str):
            try:
                return self.colors[spec.lower()]
            except KeyError:
                raise ConfigError(
                    f"{where}: unknown colour {spec!r}; known: {sorted(self.colors)}"
                ) from None
        if isinstance(spec, list) and len(spec) == 2:
            try:
                return lp.color(_int(where, spec[0], 0, 3), _int(where, spec[1], 0, 3))
            except ValueError as e:
                raise ConfigError(f"{where}: {e}") from None
        if isinstance(spec, int) and not isinstance(spec, bool):
            return _int(where, spec, 0, 127)
        raise ConfigError(
            f"{where}: colour must be a name, [red, green] (0..3 each) or raw velocity 0..127"
        )

    def note(self, where: str, spec) -> int:
        if isinstance(spec, str):
            m = _NOTE_RE.match(spec.strip())
            if not m:
                raise ConfigError(f"{where}: bad note name {spec!r} (examples: C4, F#3, Bb2)")
            letter, acc, octave = m.groups()
            n = _SEMITONES[letter.lower()] + {"": 0, "#": 1, "b": -1}[acc]
            n += (int(octave) - self.middle_c_octave + 5) * 12
            return _int(where, n, 0, 127)
        return _int(where, spec, 0, 127)

    def messages(self, where: str, spec) -> list[mido.Message]:
        if isinstance(spec, str):
            spec = [spec]
        if not isinstance(spec, list):
            raise ConfigError(f"{where}: expected a hex string or list of hex strings")
        out = []
        for i, item in enumerate(spec):
            try:
                out.append(mido.Message.from_hex(item))
            except (ValueError, TypeError) as e:
                raise ConfigError(f"{where}[{i}]: cannot parse MIDI hex {item!r}: {e}") from None
        return out

    # -- mappings ----------------------------------------------------------

    def mapping(self, where: str, scope: str, spec: dict) -> Mapping:
        if not isinstance(spec, dict):
            raise ConfigError(f"{where}: expected an inline table like {{ note = 36 }}")
        _check_keys(where, spec, MAPPING_KEYS)
        actions = [k for k in ACTION_KEYS if k in spec]
        if len(actions) != 1:
            raise ConfigError(f"{where}: needs exactly one of {list(ACTION_KEYS)}, got {actions}")
        action = actions[0]
        ch = _int(f"{where}.channel", spec.get("channel", self.default_channel + 1), 1, 16) - 1
        mode = spec.get("mode", "momentary")
        if mode not in ("momentary", "toggle"):
            raise ConfigError(f"{where}.mode: must be 'momentary' or 'toggle'")
        m = Mapping(
            scope=scope,
            mode=mode,
            on_msgs=[],
            off_msgs=[],
            color=self.color(f"{where}.color", spec.get("color", self.default_color)),
            off_color=self.color(
                f"{where}.off_color", spec.get("off_color", self.default_off_color)
            ),
            label=str(spec.get("label", "")),
            action=action,
            channel=ch,
        )
        if action == "note":
            n = self.note(f"{where}.note", spec["note"])
            vel = _int(f"{where}.velocity", spec.get("velocity", 127), 1, 127)
            m.on_msgs = [mido.Message("note_on", channel=ch, note=n, velocity=vel)]
            m.off_msgs = [mido.Message("note_off", channel=ch, note=n, velocity=0)]
            m.feedback_key = ("note", ch, n)
            m.number, m.on_value = n, vel
        elif action == "cc":
            n = _int(f"{where}.cc", spec["cc"], 0, 127)
            on = _int(f"{where}.value", spec.get("value", 127), 0, 127)
            off = _int(f"{where}.off_value", spec.get("off_value", 0), 0, 127)
            m.on_msgs = [mido.Message("control_change", channel=ch, control=n, value=on)]
            m.off_msgs = [mido.Message("control_change", channel=ch, control=n, value=off)]
            m.feedback_key = ("cc", ch, n)
            m.feedback_off_value = off
            m.number, m.on_value, m.off_value = n, on, off
        elif action == "program":
            n = _int(f"{where}.program", spec["program"], 0, 127)
            m.on_msgs = [mido.Message("program_change", channel=ch, program=n)]
            m.number = n
        elif action == "send":
            m.on_msgs = self.messages(f"{where}.send", spec["send"])
            m.off_msgs = self.messages(f"{where}.release", spec.get("release", []))
        elif action == "page":
            m.page = str(spec["page"])
            m.mode = "momentary"
        if "release" in spec and action != "send":
            raise ConfigError(f"{where}.release: only valid together with 'send'")
        return m

    def layout(self, where: str, scope: str, spec: dict) -> dict[str, Mapping]:
        """Expand a [pages.X.layout] table into per-pad grid mappings."""
        if not isinstance(spec, dict):
            raise ConfigError(f"{where}: expected a table")
        _check_keys(where, spec, LAYOUT_KEYS)
        kind = spec.get("type", "notes")
        if kind not in ("notes", "cc"):
            raise ConfigError(f"{where}.type: must be 'notes' or 'cc'")
        start = (
            self.note(f"{where}.start", spec.get("start", 36))
            if kind == "notes"
            else _int(f"{where}.start", spec.get("start", 0), 0, 127)
        )
        row_step = _int(f"{where}.row_step", spec.get("row_step", 8), -127, 127)
        col_step = _int(f"{where}.col_step", spec.get("col_step", 1), -127, 127)
        origin = spec.get("origin", "bottom-left")
        if origin not in ("bottom-left", "top-left"):
            raise ConfigError(f"{where}.origin: must be 'bottom-left' or 'top-left'")
        base = {k: spec[k] for k in ("channel", "mode", "color", "off_color") if k in spec}
        if kind == "notes" and "velocity" in spec:
            base["velocity"] = spec["velocity"]
        out = {}
        for r in range(8):
            for c in range(8):
                step_row = 7 - r if origin == "bottom-left" else r
                n = start + step_row * row_step + c * col_step
                if 0 <= n <= 127:
                    item = dict(base, **({"note": n} if kind == "notes" else {"cc": n}))
                    out[f"grid.{r}.{c}"] = self.mapping(f"{where}[grid.{r}.{c}]", scope, item)
        return out

    def button_table(self, where: str, scope: str, table: dict) -> dict[str, Mapping]:
        out: dict[str, Mapping] = {}
        if "layout" in table:
            out.update(self.layout(f"{where}.layout", scope, table["layout"]))
        for key, spec in table.items():
            if key == "layout":
                continue
            try:
                button = lp.parse_button(key)
            except ValueError as e:
                raise ConfigError(f"{where}: {e}") from None
            out[button] = self.mapping(f'{where}."{key}"', scope, spec)
        return out


def parse(data: dict) -> Config:
    _check_keys("config", data, TOP_LEVEL_KEYS)
    for section, allowed in SECTION_KEYS.items():
        _check_keys(section, data.get(section, {}), allowed)

    p = _Parser(data)
    device = data.get("device", {})
    output = data.get("output", {})
    behavior = data.get("behavior", {})

    cfg = Config(
        device_match=str(device.get("match", "Launchpad Mini")),
        port_name=str(output.get("name", "Launchpad Remap")),
        host_feedback=bool(output.get("feedback", True)),
        unmapped=behavior.get("unmapped", "ignore"),
        local_feedback=bool(behavior.get("local_feedback", True)),
    )
    if cfg.unmapped not in ("ignore", "passthrough"):
        raise ConfigError("behavior.unmapped: must be 'ignore' or 'passthrough'")
    if cfg.device_match.lower() in cfg.port_name.lower():
        raise ConfigError(
            "output.name must not contain device.match, or the remapper would find its own port"
        )

    global_map = p.button_table("global", GLOBAL_SCOPE, data.get("global", {}))
    pages_raw = data.get("pages", {})
    if not pages_raw:
        pages_raw = {"main": {}}
    for name, table in pages_raw.items():
        page_map = dict(global_map)
        page_map.update(p.button_table(f"pages.{name}", name, table))
        cfg.pages[name] = page_map

    cfg.start_page = str(behavior.get("start_page", next(iter(cfg.pages))))
    if cfg.start_page not in cfg.pages:
        raise ConfigError(f"behavior.start_page: no page named {cfg.start_page!r}")

    seen: set[tuple[str, str]] = set()
    for page_name, page_map in cfg.pages.items():
        for button, m in page_map.items():
            if m.page and m.page not in cfg.pages:
                raise ConfigError(
                    f"{button} on page {page_name!r}: switches to unknown page {m.page!r}"
                )
            if m.feedback_key and (m.scope, button) not in seen:
                seen.add((m.scope, button))
                cfg.feedback_index.setdefault(m.feedback_key, []).append((m.scope, button, m))
    return cfg


def load(path: Path) -> Config:
    try:
        with open(path, "rb") as f:
            data = tomllib.load(f)
    except FileNotFoundError:
        raise ConfigError(f"config file not found: {path}") from None
    except tomllib.TOMLDecodeError as e:
        raise ConfigError(f"{path}: {e}") from None
    return parse(data)
