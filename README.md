# Hubless
Hubless open-source alternative of Logitech G Hub

A lightweight tray replacement for G Hub on Windows: a single exe with no installer, about 13 MB of memory
and ~0% CPU while idle. Tested with the PRO X Superlight 2 mouse, the PRO X TKL Lightspeed keyboard and
the PRO X 2 LIGHTSPEED headset.

Work in progress: see [Plan.md](Plan.md). Protocol findings are in [docs/devices](docs/devices/README.md).
Both are in Russian.

## Features

- **Tray:** battery level of every device in the tooltip; a dot on the icon and a notification when a battery
  drops below 15%; the right-click menu has just "Exit" (everything else is in the panel and the settings window).
- **Panel** (left-click the tray icon):
  - mouse: DPI presets plus any value via the slider or input field, polling rate 125 Hz to 8 kHz,
    onboard/host mode, lift-off distance;
  - keyboard: brightness, lighting effect (static, breathe, cycle, wave, ripple) and colour;
  - headset: sidetone (how loud you hear your own voice) and auto power-off.
- **Solaar-style keyboard lighting:** while an effect is selected, Hubless controls the lighting itself and
  turns it off when you're idle; on exit, control goes back to the keyboard.
- **Settings window** ("Settings…" in the tray menu or the gear in the panel): interface language
  (English or Russian), start with Windows, the order of devices in the panel and which ones it shows,
  creating profiles and binding them to programs.
- **Profiles:** sets of mouse and keyboard settings. Switch them from the panel header, or let them switch
  automatically while a bound program's window is active (`hubless profile bind <name> cs2.exe`).
- **Onboard and host:** in onboard mode the mouse settings live in the mouse's memory and work without Hubless;
  in host mode Hubless applies them every time the device connects.

Game Mode and per-key lighting are not supported yet. There will be no equalizer, Blue VO!CE or DTS: those are
audio processing inside G Hub itself, not commands sent to the device. For an equalizer, use
[Equalizer APO](https://sourceforge.net/projects/equalizerapo/).

Close G Hub before starting Hubless (`lghub`, `lghub_agent`, `lghub_system_tray`; the `lghub_updater` service
doesn't get in the way): the two programs can't control the devices at the same time.

## Other Logitech devices

Hubless has been tested on three devices, but many Logitech mice and keyboards speak the same HID++ protocol.
To help add support for your device:

1. Close G Hub completely.
2. Open **Settings → Advanced → Create report** (or run `hubless --report`).
3. Click **Open GitHub issue** and attach the report file (or paste its text using the **Copy** button).

The report only reads data from the devices and contains no serial numbers or personal data; Hubless itself
sends nothing anywhere.

## Command line

```
hubless                       run in the tray
hubless --dump [file]         devices and HID++ feature tables (JSON and text)
hubless --report              device report for a GitHub issue
hubless --settings [section]  open the settings window of the running Hubless (devices, profiles, eq, advanced)
hubless --quit                quit the running Hubless (like "Exit" in the menu)
hubless mouse ...             DPI, polling rate, lift-off, onboard/host mode
hubless keyboard ...          brightness, lighting and effects
hubless headset ...           sidetone, auto power-off
hubless profile ...           settings profiles, binding to exe files
hubless --help                full list of commands
```

Commands that write to a device accept `--dry-run`: show the packets without sending anything.

## Building

You need Rust (stable, `x86_64-pc-windows-msvc`) and Visual Studio Build Tools with C++.

```
cargo build --release
```

The result is `target/release/hubless.exe`.

## Files

- `%APPDATA%\Hubless\config.toml`: app settings (log level, HID tracing, language).
- `%APPDATA%\Hubless\devices.toml`: device settings, profiles and the panel layout; written by Hubless itself.
- `%APPDATA%\Hubless\backups\`: backups of the mouse's onboard memory.
- `%LOCALAPPDATA%\Hubless\logs\`: logs; set the environment variable `HUBLESS_LOG=debug` for verbose ones.
- `%LOCALAPPDATA%\Hubless\reports\`: device reports.

## License

Copyright (C) 2026 TetRex

This program is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3 of the License,
or (at your option) any later version. This program is distributed WITHOUT ANY WARRANTY.
See the [LICENSE](LICENSE) file for the full text.

SPDX-License-Identifier: `GPL-3.0-or-later`
