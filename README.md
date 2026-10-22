# Hubless
Hubless open-source alternative of Logitech G Hub

A lightweight tray replacement for G Hub on Windows: a single exe with no installer, about 13 MB of memory
and ~0% CPU while idle. Tested with the PRO X Superlight 2 mouse, the PRO X TKL Lightspeed keyboard and
the PRO X 2 LIGHTSPEED headset.

Work in progress: see [Plan.md](Plan.md). Protocol findings are in [docs/devices](docs/devices/README.md).

## Features

- **Tray:** battery level of every device in the tooltip; a dot on the icon and a notification when a battery
  drops below the threshold you choose (15% by default), and a notification when a device has fully charged.
  There is no right-click menu: any click on the icon opens the panel.
- **Panel** (click the tray icon). The header has the settings (gear) and Exit (power) buttons.
  - mouse: DPI presets plus any value via the slider or input field, polling rate 125 Hz to 8 kHz;
  - keyboard: brightness, lighting effect (static, breathe, cycle, wave, ripple) and colour, plus your own colours;
    colours of single keys are set in Settings → Devices;
  - headset: sidetone (how loud you hear your own voice) and the equalizer preset.
- **Solaar-style keyboard lighting:** while an effect is selected, Hubless controls the lighting itself and
  turns it off when you're idle (after 5, 15 or 30 minutes, or never: Settings → Devices); on exit, control
  goes back to the keyboard.
- **Settings window** (the gear in the panel): interface language (English or Russian), start with Windows,
  battery notifications and the update check (optionally at startup); the order of devices in the panel and
  which ones it shows, device settings (mouse onboard/host mode and lift-off distance, headset auto power-off
  and 5-band equalizer with built-in and your own presets, keyboard key colours: click or drag over a keyboard
  layout); profiles and binding them to programs; logs and
  the device report (Advanced).
- **Profiles:** sets of mouse, keyboard (including key colours) and headset equalizer settings. Switch them from the panel header, or let them switch
  automatically while a bound program's window is active (`hubless profile bind <name> cs2.exe`).
- **Onboard and host:** in onboard mode the mouse settings live in the mouse's memory and work without Hubless;
  in host mode Hubless applies them every time the device connects.

Game Mode is not supported yet. Key colours show with the static effect (or with the lighting off) and are
saved in the profile. The headset equalizer is the one built into the headset;
there will be no Blue VO!CE or DTS: those are audio processing inside G Hub itself, not commands sent to the
device.

Close G Hub before starting Hubless (`lghub`, `lghub_agent`, `lghub_system_tray`; the `lghub_updater` service
doesn't get in the way): the two programs can't control the devices at the same time.

## Other Logitech devices

Hubless has been tested on three devices, but many Logitech mice and keyboards speak the same HID++ protocol.
To help add support for your device:

1. Close G Hub completely.
2. Open **Settings → Advanced → Create report** (or run `hubless --report`).
3. Click **Open GitHub issue** and attach the report file (or paste its text using the **Copy** button).

The report only reads data from the devices and contains no serial numbers or personal data; Hubless itself
sends nothing anywhere. The only network request is the update check: when you press the button, or at startup
if you turn that on.

## Command line

```
hubless                       run in the tray
hubless --dump [file]         devices and HID++ feature tables (JSON and text)
hubless --report              device report for a GitHub issue
hubless --settings [section]  open the settings window of the running Hubless (devices, profiles, eq, keys, advanced)
hubless --quit                quit the running Hubless (like Exit in the panel)
hubless mouse ...             DPI, polling rate, lift-off, onboard/host mode
hubless keyboard ...          brightness, lighting and effects, key colours (keyboard keys wasd ff0000)
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

- `%APPDATA%\Hubless\config.toml`: app settings (log level, HID tracing, language, battery notifications, update check at
  startup).
- `%APPDATA%\Hubless\devices.toml`: device settings, profiles, the panel layout, your keyboard colours and
  equalizer presets; written by Hubless itself.
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
