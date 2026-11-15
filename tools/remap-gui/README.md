# Remap GUI

Config editor for the remap mode of the Launchpad Mini mk1 build.

- Show each page as a Launchpad: off colour as fill, on colour as ring, short label
  (`N 36 ch10`, `CC 64 ch1 T` for toggle, `> keys` for a page switch).
- Edit a button: click it, set the fields, press **Apply**.
- Add, rename and delete pages, choose the start page.
- Import and export configs as TOML.
- Read the table stored on the device, upload a config to it.

## Install

Python 3.11 or newer with Tk (`python3-tk` on Debian/Ubuntu).

```sh
cd tools/remap-gui
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
```

## Run

```sh
.venv/bin/python remap-gui.py                 # opens ~/.config/launchpad-remap/config.toml if present
.venv/bin/python remap-gui.py example.toml    # or a specific file
```

## Device

- Flash the `minimk1` build (`cargo minimk1`) via the bootloader.
- Connect over the `MINI (MIDI)` port, the GUI finds it automatically.
- Device limits: 3 pages, actions `note`, `cc`, `program`, `page`. Raw `send` buttons and
  extra pages stay editable but block the upload.
- Page order on the device: start page first (bank 0, active at power-up).
- Select remap mode on the Launchpad: hold top button 5 for 0.5 s, press pad 14, press top
  button 5 to leave setup. With a stored table the device starts in remap mode.

## Config format

`example.toml` lists every option. Button names are 0-based from the top-left:

| Name | Button |
|---|---|
| `top.0` .. `top.7` | round buttons above the grid |
| `grid.R.C` | pads, row R, column C |
| `side.0` .. `side.7` | round buttons right of the grid |

Colours: `off`, `red_low`, `red_mid`, `red`, `green_low`, `green_mid`, `green`, `amber_low`,
`amber_mid`, `amber`, `yellow`, `lime`, `orange`, `orange_low`, or `[red, green]` with 0..3 each.

## SysEx protocol

Header `F0 7D 4C 52`, documented in `src/sys/remap.rs`.
