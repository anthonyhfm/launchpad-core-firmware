# SPDX-License-Identifier: GPL-3.0-only
"""Upload a config to the on-device remap mode.

The Launchpad stores up to 3 banks of 100 entries in its own flash (see
src/sys/remap.rs). Pages in the config become banks;
the start page always becomes bank 0, which is active after power-up.

SysEx protocol (header F0 7D 4C 52, sent to the "MINI (MIDI)" port):
    00                         info
    01 <bank> <slot> <8 bytes> set entry (RAM)
    02 <bank>                  commit bank to flash   -> reply 02 <bank> <ok>
    03 <bank> <slot>           read entry             -> reply 03 <bank> <slot> <8 bytes>
    04 <bank>                  clear bank (RAM)
    05 <bank>                  select active bank
    06                         reload from flash
"""

from __future__ import annotations

import time
from dataclasses import dataclass

import mido

from .config import Config, Mapping

HEADER = [0x7D, 0x4C, 0x52]  # mido sysex data excludes F0/F7
PROTOCOL_VERSION = 1
BANKS = 3
SLOTS = 100

KIND_NONE, KIND_NOTE, KIND_CC, KIND_PROGRAM, KIND_BANK, KIND_NATIVE = range(6)
FLAG_TOGGLE = 0x01

ENTRY_NONE = bytes(8)


class DeviceError(Exception):
    pass


def corefw_index(button: str) -> int:
    """Map button names to CoreFW surface indices (programmer layout).

    CoreFW numbers pads 11..89 from the bottom-left (row*10 + col, both
    1-based), the right-hand column is x9 and the top row is 91..98.
    """
    kind, *idx = button.split(".")
    if kind == "top":
        return 91 + int(idx[0])
    if kind == "side":
        return (8 - int(idx[0])) * 10 + 9
    row, col = int(idx[0]), int(idx[1])
    return (8 - row) * 10 + col + 1


def color_byte(velocity: int) -> int:
    """Launchpad mk1 LED velocity (16*g + r + flags) -> remap colour byte (r | g<<2)."""
    red = velocity & 0x03
    green = (velocity >> 4) & 0x03
    return red | (green << 2)


def encode_entry(m: Mapping, bank_of: dict[str, int]) -> bytes:
    flags = FLAG_TOGGLE if m.mode == "toggle" else 0
    on, off = color_byte(m.color), color_byte(m.off_color)
    if m.page:
        return bytes([KIND_BANK, 0, 0, bank_of[m.page], 0, 0, on, off])
    if m.action == "note":
        return bytes([KIND_NOTE, flags, m.channel, m.number, m.on_value, 0, on, off])
    if m.action == "cc":
        return bytes([KIND_CC, flags, m.channel, m.number, m.on_value, m.off_value, on, off])
    if m.action == "program":
        return bytes([KIND_PROGRAM, 0, m.channel, m.number, 0, 0, on, off])
    raise DeviceError(
        f"action {m.action!r} is not supported on the device (raw 'send' bytes and sysex "
        "only work with the computer-side remapper)"
    )


@dataclass
class Table:
    pages: list[str]  # page name per bank
    banks: list[dict[int, bytes]]  # slot -> 8-byte entry, per bank


def build_table(cfg: Config) -> Table:
    order = [cfg.start_page] + [p for p in cfg.pages if p != cfg.start_page]
    if len(order) > BANKS:
        raise DeviceError(f"the device holds {BANKS} pages, config has {len(order)}: {order}")
    bank_of = {name: i for i, name in enumerate(order)}
    native = cfg.unmapped == "passthrough"
    banks = []
    for name in order:
        entries: dict[int, bytes] = {}
        for button, m in cfg.pages[name].items():
            try:
                entries[corefw_index(button)] = encode_entry(m, bank_of)
            except DeviceError as e:
                raise DeviceError(f"page {name!r}, {button}: {e}") from None
        if native:
            from . import launchpad as lp

            for button in lp.ALL_BUTTONS:
                entries.setdefault(
                    corefw_index(button), bytes([KIND_NATIVE, 0, 0, 0, 0, 0, 0, 0])
                )
        banks.append(entries)
    return Table(pages=order, banks=banks)


def find_port(names: list[str], match: str | None) -> str | None:
    if match:
        return next((n for n in names if match.lower() in n.lower()), None)
    # CoreFW mk1 names its second USB MIDI port "MINI (MIDI)"; ALSA may show
    # the jack name or a numbered fallback.
    for pattern in ("mini (midi)", "launchpad mini midi 2", "launchpad mini:"):
        for n in names:
            if pattern in n.lower() and "remap" not in n.lower() and "daw" not in n.lower():
                return n
    return None


class Link:
    def __init__(self, port_match: str | None = None, delay: float = 0.003):
        outs, ins = mido.get_output_names(), mido.get_input_names()
        oname, iname = find_port(outs, port_match), find_port(ins, port_match)
        if not oname or not iname:
            raise DeviceError(
                "no CoreFW Launchpad MIDI port found (looking for 'MINI (MIDI)'); "
                f"outputs: {outs}"
            )
        self.name = oname
        self.out = mido.open_output(oname)
        self.inp = mido.open_input(iname)
        self.delay = delay

    def close(self) -> None:
        self.out.close()
        self.inp.close()

    def send(self, *data: int) -> None:
        self.out.send(mido.Message("sysex", data=HEADER + list(data)))
        time.sleep(self.delay)

    def request(self, cmd: int, *args: int, timeout: float = 1.0) -> list[int]:
        for _ in self.inp.iter_pending():
            pass
        self.send(cmd, *args)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for msg in self.inp.iter_pending():
                if msg.type == "sysex" and list(msg.data[:3]) == HEADER and msg.data[3] == cmd:
                    return list(msg.data[4:])
            time.sleep(0.002)
        raise DeviceError(f"no reply to command 0x{cmd:02x} (is the Launchpad running the remap firmware?)")

    def info(self) -> dict:
        r = self.request(0x00)
        return {
            "protocol": r[0],
            "banks": r[1],
            "slots": r[2],
            "active": r[3],
            "stored_mask": r[4],
        }

    def read_entry(self, bank: int, slot: int) -> bytes:
        r = self.request(0x03, bank, slot)
        return bytes(r[2:10])


def upload(table: Table, link: Link, log=print, retries: int = 3) -> None:
    info = link.info()
    if info["protocol"] != PROTOCOL_VERSION:
        raise DeviceError(f"device speaks remap protocol {info['protocol']}, tool speaks {PROTOCOL_VERSION}")
    used = len(table.banks)
    for bank in range(BANKS):
        entries = table.banks[bank] if bank < used else {}
        name = table.pages[bank] if bank < used else "(empty)"
        expected = {slot: entries.get(slot, ENTRY_NONE) for slot in range(SLOTS)}
        for attempt in range(1, retries + 1):
            link.send(0x04, bank)
            for slot, entry in sorted(entries.items()):
                link.send(0x01, bank, slot, *entry)
            # Verify the staged copy before writing flash (SysEx can drop under load).
            bad = [s for s in range(SLOTS) if link.read_entry(bank, s) != expected[s]]
            if not bad:
                break
            log(f"  bank {bank}: {len(bad)} entries did not arrive, retry {attempt}")
        else:
            raise DeviceError(f"bank {bank}: entries keep failing to arrive: {bad[:8]}")
        r = link.request(0x02, bank, timeout=2.0)
        if r[1] != 1:
            raise DeviceError(f"bank {bank}: flash commit failed")
        log(f"  bank {bank} <- page {name!r}: {len(entries)} buttons, written to flash")
    link.send(0x06)  # Reload from flash: verify what the device runs
    bad = [
        (b, s)
        for b in range(used)
        for s in range(SLOTS)
        if link.read_entry(b, s) != table.banks[b].get(s, ENTRY_NONE)
    ]
    if bad:
        raise DeviceError(f"flash read-back mismatch at {bad[:8]}")
    link.send(0x05, 0)
    log("  verified flash contents; bank 0 active")
