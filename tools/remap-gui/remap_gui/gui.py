# SPDX-License-Identifier: GPL-3.0-only
"""Tk editor for Launchpad Mini mk1 remap configs.

Show the Launchpad layout per page, edit each button's mapping, import and
export TOML configs, read and upload the on-device table (remap mode).

Run: remap-gui.py [config.toml]
"""

from __future__ import annotations

import json
import queue
import re
import threading
import tkinter as tk
import tomllib
from pathlib import Path
from tkinter import filedialog, messagebox, simpledialog, ttk

from . import launchpad as lp
from .config import Config, ConfigError, Mapping, parse

SECTIONS = ("device", "output", "behavior", "colors")
ACTIONS = ("none", "note", "cc", "program", "page", "send")
MODES = ("momentary", "toggle")

CELL = 58
GAP = 6
PAD = 14


# ---------------------------------------------------------------------------
# Model


class Model:
    """Editable config: extra sections plus explicit per-page button specs.

    Button specs use the config file's own inline-table format, e.g.
    {"note": 36, "channel": 10, "color": "red"}; validation goes through the
    real config parser: the GUI accepts exactly what the parser accepts.
    """

    def __init__(self) -> None:
        self.sections: dict[str, dict] = {"behavior": {"start_page": "main"}}
        self.pages: dict[str, dict[str, dict]] = {"main": {}}
        self.path: Path | None = None
        self.dirty = False

    # -- conversion --------------------------------------------------------

    def color_names(self) -> dict[int, str]:
        names: dict[int, str] = {}
        custom = self.sections.get("colors", {})
        try:
            parsed = parse({"colors": custom}) if custom else None
        except ConfigError:
            parsed = None
        for name, value in lp.COLORS.items():
            names.setdefault(value, name)
        if parsed is not None:
            for name, spec in custom.items():
                try:
                    value = _velocity_of(spec)
                except ValueError:
                    continue
                names.setdefault(value, name)
        return names

    def color_spec(self, velocity: int):
        name = self.color_names().get(velocity)
        if name:
            return name
        if velocity & 0x0C == lp.FLAGS_NORMAL:
            return [velocity & 0x03, (velocity >> 4) & 0x03]
        return velocity

    def mapping_to_spec(self, m: Mapping) -> dict:
        spec: dict = {}
        if m.page:
            spec["page"] = m.page
        elif m.action == "note":
            spec["note"] = m.number
            spec["channel"] = m.channel + 1
            if m.on_value != 127:
                spec["velocity"] = m.on_value
        elif m.action == "cc":
            spec["cc"] = m.number
            spec["channel"] = m.channel + 1
            if m.on_value != 127:
                spec["value"] = m.on_value
            if m.off_value:
                spec["off_value"] = m.off_value
        elif m.action == "program":
            spec["program"] = m.number
            spec["channel"] = m.channel + 1
        elif m.action == "send":
            spec["send"] = [x.hex() for x in m.on_msgs]
            if m.off_msgs:
                spec["release"] = [x.hex() for x in m.off_msgs]
        if m.mode == "toggle" and not m.page:
            spec["mode"] = "toggle"
        spec["color"] = self.color_spec(m.color)
        spec["off_color"] = self.color_spec(m.off_color)
        if m.label:
            spec["label"] = m.label
        return spec

    def load_data(self, data: dict, path: Path | None) -> None:
        cfg = parse(data)  # raises ConfigError
        self.sections = {k: dict(data[k]) for k in SECTIONS if k in data}
        self.sections.setdefault("behavior", {})["start_page"] = cfg.start_page
        self.pages = {}
        for name, page in cfg.pages.items():
            self.pages[name] = {b: self.mapping_to_spec(m) for b, m in page.items()}
        self.path = path
        self.dirty = False

    def load_file(self, path: Path) -> None:
        with open(path, "rb") as f:
            data = tomllib.load(f)
        self.load_data(data, path)

    def to_data(self) -> dict:
        data: dict = {k: dict(v) for k, v in self.sections.items() if v}
        names = list(self.pages)
        common: dict[str, dict] = {}
        if len(names) > 1:
            for button, spec in self.pages[names[0]].items():
                if all(self.pages[n].get(button) == spec for n in names[1:]):
                    common[button] = spec
        if common:
            data["global"] = dict(common)
        data["pages"] = {
            name: {b: s for b, s in page.items() if b not in common}
            for name, page in self.pages.items()
        }
        return data

    def config(self) -> Config:
        return parse(self.to_data())

    def start_page(self) -> str:
        start = self.sections.get("behavior", {}).get("start_page")
        return start if start in self.pages else next(iter(self.pages))


def _velocity_of(spec) -> int:
    if isinstance(spec, list) and len(spec) == 2:
        return lp.color(int(spec[0]), int(spec[1]))
    if isinstance(spec, int):
        return spec
    raise ValueError(spec)


# ---------------------------------------------------------------------------
# TOML writer (tomllib only reads)

_BARE_KEY = re.compile(r"^[A-Za-z0-9_-]+$")


def _key(k: str) -> str:
    return k if _BARE_KEY.match(k) else json.dumps(k)


def _value(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return str(v)
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return "[" + ", ".join(_value(x) for x in v) + "]"
    if isinstance(v, dict):
        return "{ " + ", ".join(f"{_key(k)} = {_value(x)}" for k, x in v.items()) + " }"
    raise TypeError(f"cannot write {type(v).__name__} to TOML")


def _button_order(button: str) -> int:
    return lp.ALL_BUTTONS.index(button) if button in lp.ALL_BUTTONS else 999


def dump_toml(data: dict) -> str:
    out = ["# Launchpad Mini mk1 remap config (written by remap-gui)", ""]
    for section in SECTIONS:
        if data.get(section):
            out.append(f"[{section}]")
            out += [f"{_key(k)} = {_value(v)}" for k, v in data[section].items()]
            out.append("")
    if data.get("global"):
        out.append("[global]")
        for b in sorted(data["global"], key=_button_order):
            out.append(f"{_key(b)} = {_value(data['global'][b])}")
        out.append("")
    for name, page in data.get("pages", {}).items():
        out.append(f"[pages.{_key(name)}]")
        for b in sorted(page, key=_button_order):
            out.append(f"{_key(b)} = {_value(page[b])}")
        out.append("")
    return "\n".join(out)


# ---------------------------------------------------------------------------
# Device <-> model


def specs_from_device(link, page_names: list[str], log=print) -> dict[str, dict[str, dict]]:
    from . import device

    slot_to_button = {device.corefw_index(b): b for b in lp.ALL_BUTTONS}
    names = list(page_names[: device.BANKS])
    while len(names) < device.BANKS:
        names.append(f"bank{len(names)}")
    model = Model()
    pages: dict[str, dict[str, dict]] = {}
    for bank in range(device.BANKS):
        log(f"reading bank {bank} ...")
        specs: dict[str, dict] = {}
        for slot, button in slot_to_button.items():
            e = link.read_entry(bank, slot)
            kind, flags, ch, num, on, off, con, coff = e
            colors = {
                "color": model.color_spec(lp.color(con & 3, (con >> 2) & 3)),
                "off_color": model.color_spec(lp.color(coff & 3, (coff >> 2) & 3)),
            }
            toggle = {"mode": "toggle"} if flags & device.FLAG_TOGGLE else {}
            if kind == device.KIND_NOTE:
                spec = {"note": num, "channel": ch + 1, **({"velocity": on} if on not in (0, 127) else {})}
            elif kind == device.KIND_CC:
                spec = {"cc": num, "channel": ch + 1, **({"value": on} if on != 127 else {}),
                        **({"off_value": off} if off else {})}
            elif kind == device.KIND_PROGRAM:
                spec = {"program": num, "channel": ch + 1}
            elif kind == device.KIND_BANK and num < device.BANKS:
                spec, toggle = {"page": names[num]}, {}
            else:
                continue  # empty or native passthrough
            specs[button] = {**spec, **toggle, **colors}
        pages[names[bank]] = specs
    # Drop empty trailing banks nobody switches to.
    referenced = {s["page"] for p in pages.values() for s in p.values() if "page" in s}
    for name in names[1:]:
        if not pages[name] and name not in referenced:
            del pages[name]
    return pages


# ---------------------------------------------------------------------------
# UI


def _rgb(velocity: int) -> str:
    red, green = velocity & 0x03, (velocity >> 4) & 0x03
    if red == 0 and green == 0:
        return "#2b2b2b"
    return f"#{min(255, 40 + red * 72):02x}{min(255, 40 + green * 72):02x}28"


def _text_color(velocity: int) -> str:
    red, green = velocity & 0x03, (velocity >> 4) & 0x03
    return "#111111" if red + green >= 4 else "#f0f0f0"


def _summary(m: Mapping) -> tuple[str, str]:
    if m.page:
        return "> " + m.page[:7], ""
    sub = []
    if m.action == "note":
        main = f"N {m.number}"
        sub.append(f"ch{m.channel + 1}")
    elif m.action == "cc":
        main = f"CC {m.number}"
        sub.append(f"ch{m.channel + 1}")
    elif m.action == "program":
        main = f"PC {m.number}"
        sub.append(f"ch{m.channel + 1}")
    elif m.action == "send":
        main = "RAW"
    else:
        main = "?"
    if m.mode == "toggle":
        sub.append("T")
    return main, " ".join(sub)


class Editor(ttk.Frame):
    """Form for one button's mapping."""

    def __init__(self, master, app: "App"):
        super().__init__(master, padding=10)
        self.app = app
        self.vars = {
            "action": tk.StringVar(value="none"),
            "channel": tk.StringVar(value="1"),
            "number": tk.StringVar(),
            "value": tk.StringVar(),
            "off_value": tk.StringVar(),
            "page": tk.StringVar(),
            "send": tk.StringVar(),
            "release": tk.StringVar(),
            "mode": tk.StringVar(value="momentary"),
            "color": tk.StringVar(value="green"),
            "off_color": tk.StringVar(value="off"),
            "label": tk.StringVar(),
            "all_pages": tk.BooleanVar(value=False),
        }
        self.title = ttk.Label(self, text="Click a button", font=("TkDefaultFont", 12, "bold"))
        self.title.grid(row=0, column=0, columnspan=2, sticky="w", pady=(0, 8))

        rows = [
            ("Action", "action", "combo", ACTIONS),
            ("Channel (1-16)", "channel", "spin", (1, 16)),
            ("Note / CC / Program", "number", "entry", None),
            ("Velocity / CC value", "value", "entry", None),
            ("CC off value", "off_value", "entry", None),
            ("Switch to page", "page", "combo", ()),
            ("Send (hex, ';' between msgs)", "send", "entry", None),
            ("Release (hex)", "release", "entry", None),
            ("Mode", "mode", "combo", MODES),
            ("Color (on)", "color", "combo", ()),
            ("Color (off)", "off_color", "combo", ()),
            ("Label", "label", "entry", None),
        ]
        self.widgets: dict[str, tk.Widget] = {}
        for i, (text, key, kind, opts) in enumerate(rows, start=1):
            ttk.Label(self, text=text).grid(row=i, column=0, sticky="w", pady=2)
            if kind == "combo":
                w = ttk.Combobox(self, textvariable=self.vars[key], values=opts, width=22,
                                 state="readonly" if key in ("action", "mode", "page") else "normal")
            elif kind == "spin":
                w = ttk.Spinbox(self, textvariable=self.vars[key], from_=opts[0], to=opts[1], width=6)
            else:
                w = ttk.Entry(self, textvariable=self.vars[key], width=24)
            w.grid(row=i, column=1, sticky="we", pady=2)
            self.widgets[key] = w
        self.vars["action"].trace_add("write", lambda *_: self._update_enabled())

        n = len(rows) + 1
        ttk.Checkbutton(self, text="Apply to all pages", variable=self.vars["all_pages"]).grid(
            row=n, column=0, columnspan=2, sticky="w", pady=(8, 2))
        bar = ttk.Frame(self)
        bar.grid(row=n + 1, column=0, columnspan=2, sticky="we", pady=(6, 0))
        ttk.Button(bar, text="Apply", command=self.apply).pack(side="left")
        ttk.Button(bar, text="Clear button", command=self.clear).pack(side="left", padx=6)
        ttk.Button(bar, text="Revert", command=self.reload).pack(side="left")
        self.hint = ttk.Label(self, text="Note accepts numbers or names (C4, F#3).\nColors: name or r,g (0-3).",
                              foreground="#666")
        self.hint.grid(row=n + 2, column=0, columnspan=2, sticky="w", pady=(10, 0))
        self.columnconfigure(1, weight=1)
        self.button: str | None = None
        self._update_enabled()

    def refresh_choices(self) -> None:
        names = sorted(set(self.app.model.color_names().values()), key=str)
        self.widgets["color"]["values"] = names
        self.widgets["off_color"]["values"] = names
        self.widgets["page"]["values"] = list(self.app.model.pages)

    def _update_enabled(self) -> None:
        action = self.vars["action"].get()
        relevant = {
            "none": set(),
            "note": {"channel", "number", "value", "mode", "color", "off_color", "label"},
            "cc": {"channel", "number", "value", "off_value", "mode", "color", "off_color", "label"},
            "program": {"channel", "number", "color", "off_color", "label"},
            "page": {"page", "color", "off_color", "label"},
            "send": {"send", "release", "mode", "color", "off_color", "label"},
        }[action]
        for key, w in self.widgets.items():
            if key == "action":
                continue
            if key in relevant:
                w.configure(state="readonly" if key in ("mode", "page") else "normal")
            else:
                w.configure(state="disabled")

    def load(self, button: str) -> None:
        self.button = button
        self.refresh_choices()
        spec = self.app.model.pages[self.app.page].get(button, {})
        v = self.vars
        for key in ("number", "value", "off_value", "page", "send", "release", "label"):
            v[key].set("")
        v["channel"].set(str(spec.get("channel", self.app.model.sections.get("output", {}).get("channel", 1))))
        v["mode"].set(spec.get("mode", "momentary"))
        v["color"].set(_color_text(spec.get("color", "green")))
        v["off_color"].set(_color_text(spec.get("off_color", "off")))
        v["label"].set(spec.get("label", ""))
        action = next((a for a in ("note", "cc", "program", "page", "send") if a in spec), "none")
        v["action"].set(action)
        if action in ("note", "cc", "program"):
            v["number"].set(str(spec[action]))
        if action == "note" and "velocity" in spec:
            v["value"].set(str(spec["velocity"]))
        if action == "cc":
            v["value"].set(str(spec.get("value", "")))
            v["off_value"].set(str(spec.get("off_value", "")))
        if action == "page":
            v["page"].set(spec["page"])
        if action == "send":
            v["send"].set("; ".join(_as_list(spec["send"])))
            v["release"].set("; ".join(_as_list(spec.get("release", []))))
        v["all_pages"].set(all(p.get(button) == spec for p in self.app.model.pages.values()) and bool(spec)
                           and len(self.app.model.pages) > 1)
        self.title.configure(text=f"{button}   (page {self.app.page})")

    def reload(self) -> None:
        if self.button:
            self.load(self.button)

    def build_spec(self) -> dict | None:
        v = self.vars
        action = v["action"].get()
        if action == "none":
            return None
        spec: dict = {}

        def num(key, label):
            text = v[key].get().strip()
            if not text:
                return None
            try:
                return int(text)
            except ValueError:
                raise ValueError(f"{label} must be a number, got {text!r}") from None

        if action in ("note", "cc", "program"):
            text = v["number"].get().strip()
            if not text:
                raise ValueError("enter a note / CC / program number")
            spec[action] = int(text) if text.lstrip("-").isdigit() else text
            spec["channel"] = num("channel", "channel")
        if action == "note" and (vel := num("value", "velocity")) is not None:
            spec["velocity"] = vel
        if action == "cc":
            if (val := num("value", "CC value")) is not None:
                spec["value"] = val
            if (off := num("off_value", "CC off value")) is not None:
                spec["off_value"] = off
        if action == "page":
            if not v["page"].get():
                raise ValueError("choose a page to switch to")
            spec["page"] = v["page"].get()
        if action == "send":
            msgs = [x.strip() for x in v["send"].get().split(";") if x.strip()]
            if not msgs:
                raise ValueError("enter at least one hex message, e.g. B0 07 64")
            spec["send"] = msgs
            rel = [x.strip() for x in v["release"].get().split(";") if x.strip()]
            if rel:
                spec["release"] = rel
        if action in ("note", "cc", "send") and v["mode"].get() == "toggle":
            spec["mode"] = "toggle"
        spec["color"] = _parse_color_text(v["color"].get())
        spec["off_color"] = _parse_color_text(v["off_color"].get())
        if v["label"].get().strip():
            spec["label"] = v["label"].get().strip()
        return spec

    def apply(self) -> None:
        if not self.button:
            return
        try:
            spec = self.build_spec()
        except ValueError as e:
            messagebox.showerror("Invalid input", str(e), parent=self)
            return
        targets = list(self.app.model.pages) if self.vars["all_pages"].get() else [self.app.page]
        self.app.change(lambda m: [_set(m.pages[p], self.button, spec) for p in targets])

    def clear(self) -> None:
        if not self.button:
            return
        targets = list(self.app.model.pages) if self.vars["all_pages"].get() else [self.app.page]
        self.app.change(lambda m: [m.pages[p].pop(self.button, None) for p in targets])


def _set(page: dict, button: str, spec: dict | None) -> None:
    if spec is None:
        page.pop(button, None)
    else:
        page[button] = dict(spec)


def _as_list(x) -> list[str]:
    return [x] if isinstance(x, str) else list(x)


def _color_text(spec) -> str:
    if isinstance(spec, list):
        return f"{spec[0]},{spec[1]}"
    return str(spec)


def _parse_color_text(text: str):
    text = text.strip()
    if re.fullmatch(r"\d\s*,\s*\d", text):
        r, g = (int(x) for x in text.split(","))
        return [r, g]
    if text.isdigit():
        return int(text)
    return text or "off"


class App:
    def __init__(self, root: tk.Tk, path: Path | None):
        self.root = root
        self.model = Model()
        self.page = "main"
        self.selected: str | None = None
        self.jobs: queue.Queue = queue.Queue()
        root.minsize(980, 640)

        self._build_menu()
        top = ttk.Frame(root, padding=(10, 8))
        top.pack(fill="x")
        ttk.Label(top, text="Page:").pack(side="left")
        self.page_var = tk.StringVar()
        self.page_box = ttk.Combobox(top, textvariable=self.page_var, state="readonly", width=16)
        self.page_box.pack(side="left", padx=(4, 6))
        self.page_box.bind("<<ComboboxSelected>>", lambda e: self.show_page(self.page_var.get()))
        ttk.Button(top, text="Add", command=self.add_page).pack(side="left")
        ttk.Button(top, text="Rename", command=self.rename_page).pack(side="left", padx=4)
        ttk.Button(top, text="Delete", command=self.delete_page).pack(side="left")
        ttk.Label(top, text="   Start page:").pack(side="left")
        self.start_var = tk.StringVar()
        self.start_box = ttk.Combobox(top, textvariable=self.start_var, state="readonly", width=16)
        self.start_box.pack(side="left", padx=4)
        self.start_box.bind("<<ComboboxSelected>>", lambda e: self.set_start(self.start_var.get()))
        ttk.Button(top, text="Upload to device", command=self.upload).pack(side="right")
        ttk.Button(top, text="Read from device", command=self.read_device).pack(side="right", padx=6)

        body = ttk.Frame(root)
        body.pack(fill="both", expand=True)
        size = PAD * 2 + 9 * CELL + 8 * GAP
        self.canvas = tk.Canvas(body, width=size, height=size, background="#181818", highlightthickness=0)
        self.canvas.pack(side="left", padx=10, pady=(0, 10))
        self.canvas.bind("<Button-1>", self.on_click)
        self.editor = Editor(body, self)
        self.editor.pack(side="left", fill="both", expand=True)

        self.status = ttk.Label(root, text="", anchor="w", padding=(10, 4), relief="sunken")
        self.status.pack(fill="x", side="bottom")

        if path and path.exists():
            self.import_file(path)
        else:
            self.refresh()
            self.set_status("New config. Import a TOML file or read the config from the device.")
        root.after(100, self._poll_jobs)

    # -- menu / files ------------------------------------------------------

    def _build_menu(self) -> None:
        menu = tk.Menu(self.root)
        f = tk.Menu(menu, tearoff=False)
        f.add_command(label="New", command=self.new)
        f.add_command(label="Import...", command=self.import_dialog, accelerator="Ctrl+O")
        f.add_command(label="Save", command=self.save, accelerator="Ctrl+S")
        f.add_command(label="Export...", command=self.export_dialog, accelerator="Ctrl+Shift+S")
        f.add_separator()
        f.add_command(label="Quit", command=self.quit)
        menu.add_cascade(label="File", menu=f)
        d = tk.Menu(menu, tearoff=False)
        d.add_command(label="Read from device", command=self.read_device)
        d.add_command(label="Upload to device", command=self.upload)
        menu.add_cascade(label="Device", menu=d)
        self.root.config(menu=menu)
        self.root.bind("<Control-o>", lambda e: self.import_dialog())
        self.root.bind("<Control-s>", lambda e: self.save())
        self.root.bind("<Control-S>", lambda e: self.export_dialog())
        self.root.protocol("WM_DELETE_WINDOW", self.quit)

    def confirm_discard(self) -> bool:
        return not self.model.dirty or messagebox.askyesno(
            "Unsaved changes", "Discard unsaved changes?", parent=self.root)

    def new(self) -> None:
        if self.confirm_discard():
            self.model = Model()
            self.page = "main"
            self.selected = None
            self.refresh()

    def quit(self) -> None:
        if self.confirm_discard():
            self.root.destroy()

    def import_dialog(self) -> None:
        if not self.confirm_discard():
            return
        name = filedialog.askopenfilename(
            parent=self.root, title="Import config",
            filetypes=[("TOML config", "*.toml"), ("All files", "*")])
        if name:
            self.import_file(Path(name))

    def import_file(self, path: Path) -> None:
        try:
            self.model.load_file(path)
        except (OSError, tomllib.TOMLDecodeError, ConfigError) as e:
            messagebox.showerror("Import failed", f"{path}:\n{e}", parent=self.root)
            return
        self.page = self.model.start_page()
        self.selected = None
        self.refresh()
        self.set_status(f"Imported {path}")

    def export_dialog(self) -> None:
        name = filedialog.asksaveasfilename(
            parent=self.root, title="Export config", defaultextension=".toml",
            initialfile=self.model.path.name if self.model.path else "launchpad.toml",
            filetypes=[("TOML config", "*.toml")])
        if name:
            self.write(Path(name))

    def save(self) -> None:
        if self.model.path:
            self.write(self.model.path)
        else:
            self.export_dialog()

    def write(self, path: Path) -> None:
        try:
            self.model.config()  # validate before writing
            text = dump_toml(self.model.to_data())
            parse(tomllib.loads(text))  # the written file must load back
            path.write_text(text)
        except (ConfigError, OSError, tomllib.TOMLDecodeError) as e:
            messagebox.showerror("Export failed", str(e), parent=self.root)
            return
        self.model.path = path
        self.model.dirty = False
        self.refresh()
        self.set_status(f"Saved {path}")

    # -- pages -------------------------------------------------------------

    def show_page(self, name: str) -> None:
        self.page = name
        self.refresh()

    def add_page(self) -> None:
        name = simpledialog.askstring("Add page", "Page name:", parent=self.root)
        if not name:
            return
        if name in self.model.pages:
            messagebox.showerror("Add page", f"Page {name!r} already exists", parent=self.root)
            return
        common = {}
        names = list(self.model.pages)
        if len(names) > 1:  # copy buttons shared by all pages (page switches etc.)
            first = self.model.pages[names[0]]
            common = {b: dict(s) for b, s in first.items()
                      if all(self.model.pages[n].get(b) == s for n in names[1:])}
        self.page = name
        self.change(lambda m: m.pages.__setitem__(name, common))

    def rename_page(self) -> None:
        old = self.page
        new = simpledialog.askstring("Rename page", "New name:", initialvalue=old, parent=self.root)
        if not new or new == old:
            return
        if new in self.model.pages:
            messagebox.showerror("Rename page", f"Page {new!r} already exists", parent=self.root)
            return

        def do(m: Model) -> None:
            m.pages = {(new if k == old else k): v for k, v in m.pages.items()}
            for page in m.pages.values():
                for spec in page.values():
                    if spec.get("page") == old:
                        spec["page"] = new
            if m.sections.get("behavior", {}).get("start_page") == old:
                m.sections["behavior"]["start_page"] = new

        self.page = new
        self.change(do)

    def delete_page(self) -> None:
        if len(self.model.pages) == 1:
            messagebox.showinfo("Delete page", "A config needs at least one page.", parent=self.root)
            return
        name = self.page
        refs = [f"{p}: {b}" for p, page in self.model.pages.items() for b, s in page.items()
                if s.get("page") == name and p != name]
        msg = f"Delete page {name!r}?"
        if refs:
            msg += f"\n\nThese page buttons point to it and will be cleared:\n" + "\n".join(refs[:10])
        if not messagebox.askyesno("Delete page", msg, parent=self.root):
            return

        def do(m: Model) -> None:
            del m.pages[name]
            for page in m.pages.values():
                for b in [b for b, s in page.items() if s.get("page") == name]:
                    del page[b]
            if m.sections.get("behavior", {}).get("start_page") == name:
                m.sections["behavior"]["start_page"] = next(iter(m.pages))

        self.page = next(n for n in self.model.pages if n != name)
        self.change(do)

    def set_start(self, name: str) -> None:
        self.change(lambda m: m.sections.setdefault("behavior", {}).__setitem__("start_page", name))

    # -- editing -----------------------------------------------------------

    def change(self, fn) -> None:
        """Apply `fn` to the model; roll back if the result does not validate."""
        before = (json.dumps(self.model.pages), json.dumps(self.model.sections), self.page)
        fn(self.model)
        try:
            self.model.config()
        except ConfigError as e:
            self.model.pages = json.loads(before[0])
            self.model.sections = json.loads(before[1])
            self.page = before[2]
            messagebox.showerror("Not applied", str(e), parent=self.root)
        else:
            self.model.dirty = True
        if self.page not in self.model.pages:
            self.page = next(iter(self.model.pages))
        self.refresh()

    def on_click(self, event) -> None:
        item = self.canvas.find_withtag("current")
        tags = self.canvas.gettags(item[0]) if item else ()
        button = next((t[2:] for t in tags if t.startswith("b:")), None)
        if button:
            self.selected = button
            self.editor.load(button)
            self.draw()

    # -- drawing -----------------------------------------------------------

    def refresh(self) -> None:
        names = list(self.model.pages)
        if self.page not in names:
            self.page = self.model.start_page()
        self.page_box["values"] = names
        self.page_var.set(self.page)
        self.start_box["values"] = names
        self.start_var.set(self.model.start_page())
        self.editor.refresh_choices()
        if self.selected:
            self.editor.load(self.selected)
        title = str(self.model.path) if self.model.path else "untitled"
        self.root.title(f"Launchpad Remap - {title}{' *' if self.model.dirty else ''}")
        self.draw()

    def draw(self) -> None:
        c = self.canvas
        c.delete("all")
        try:
            cfg = self.model.config()
            mappings = cfg.pages.get(self.page, {})
        except ConfigError as e:
            mappings = {}
            self.set_status(f"Config error: {e}")
        for button in lp.ALL_BUTTONS:
            kind, *idx = button.split(".")
            if kind == "top":
                row, col = 0, int(idx[0])
            elif kind == "side":
                row, col = int(idx[0]) + 1, 8
            else:
                row, col = int(idx[0]) + 1, int(idx[1])
            x0 = PAD + col * (CELL + GAP)
            y0 = PAD + row * (CELL + GAP)
            x1, y1 = x0 + CELL, y0 + CELL
            m = mappings.get(button)
            # The page button of the page on screen shows its "on" colour, as on the device.
            active = bool(m and m.page == self.page)
            fill = (_rgb(m.color) if active else _rgb(m.off_color)) if m else "#202020"
            ring = _rgb(m.color) if m else "#333333"
            width = 4 if m else 1
            outline = "#4aa3ff" if button == self.selected else ring
            if button == self.selected:
                width = 4
            tag = ("b:" + button,)
            if kind == "grid":
                c.create_rectangle(x0, y0, x1, y1, fill=fill, outline=outline, width=width, tags=tag)
            else:
                inset = 6
                c.create_oval(x0 + inset, y0 + inset, x1 - inset, y1 - inset,
                              fill=fill, outline=outline, width=width, tags=tag)
            if m:
                main, sub = _summary(m)
                fg = _text_color(m.color if active else m.off_color)
                c.create_text((x0 + x1) / 2, y0 + CELL * 0.40, text=main, fill=fg,
                              font=("TkDefaultFont", 9, "bold"), tags=tag)
                if sub:
                    c.create_text((x0 + x1) / 2, y0 + CELL * 0.68, text=sub, fill=fg,
                                  font=("TkDefaultFont", 8), tags=tag)

    def set_status(self, text: str) -> None:
        self.status.configure(text=text)

    # -- device ------------------------------------------------------------

    def _run(self, title: str, work, done) -> None:
        """Run `work(log)` in a thread; call `done(result)` on the Tk thread."""
        self.set_status(f"{title} ...")
        self.root.configure(cursor="watch")

        def runner():
            try:
                result = work(lambda msg: self.jobs.put(("log", msg)))
                self.jobs.put(("done", (done, result, None)))
            except Exception as e:  # noqa: BLE001 - shown to the user
                self.jobs.put(("done", (done, None, e)))

        threading.Thread(target=runner, daemon=True).start()

    def _poll_jobs(self) -> None:
        try:
            while True:
                kind, payload = self.jobs.get_nowait()
                if kind == "log":
                    self.set_status(payload.strip())
                else:
                    done, result, error = payload
                    self.root.configure(cursor="")
                    if error:
                        self.set_status(f"Failed: {error}")
                        messagebox.showerror("Device", str(error), parent=self.root)
                    else:
                        done(result)
        except queue.Empty:
            pass
        self.root.after(100, self._poll_jobs)

    def upload(self) -> None:
        from . import device

        try:
            table = device.build_table(self.model.config())
        except (ConfigError, device.DeviceError) as e:
            messagebox.showerror("Cannot upload", str(e), parent=self.root)
            return
        summary = "\n".join(f"bank {i}: page {n!r}, {len(b)} buttons"
                            for i, (n, b) in enumerate(zip(table.pages, table.banks)))
        if not messagebox.askyesno("Upload to device",
                                   f"Write this to the Launchpad's flash?\n\n{summary}", parent=self.root):
            return

        def work(log):
            link = device.Link()
            try:
                device.upload(table, link, log=log)
            finally:
                link.close()

        self._run("Uploading", work, lambda _: self.set_status("Upload done and verified."))

    def read_device(self) -> None:
        from . import device

        if not self.confirm_discard():
            return
        try:
            cfg = self.model.config()
            names = [cfg.start_page] + [p for p in cfg.pages if p != cfg.start_page]
        except ConfigError:
            names = []

        def work(log):
            link = device.Link()
            try:
                return specs_from_device(link, names, log=log)
            finally:
                link.close()

        def done(pages):
            self.model.pages = pages
            self.model.sections.setdefault("behavior", {})["start_page"] = next(iter(pages))
            self.model.path = None
            self.model.dirty = True
            self.page = next(iter(pages))
            self.selected = None
            self.refresh()
            self.set_status(f"Read {len(pages)} page(s) from the device. Export to keep a copy.")

        self._run("Reading device", work, done)


def run(path: Path | None = None) -> None:
    root = tk.Tk()
    try:
        ttk.Style(root).theme_use("clam")
    except tk.TclError:
        pass
    App(root, path)
    root.mainloop()


def main() -> None:
    import sys

    path = Path(sys.argv[1]) if len(sys.argv) > 1 else None
    if path is None:
        from . import DEFAULT_CONFIG

        path = DEFAULT_CONFIG if DEFAULT_CONFIG.exists() else None
    run(path)
