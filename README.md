# Hubless

An open-source, lightweight alternative to Logitech G Hub for Windows.

Hubless is a single exe that lives in the tray: no installer, no services, no account. It uses about 13 MB of memory
and ~0% CPU while idle. It talks to Logitech mice and keyboards over HID++ 2.0 (through a LIGHTSPEED receiver or a USB
cable) and to Logitech headsets over Centurion. Devices are found automatically, and every card shows only the
settings the device actually supports.

Hubless is under active development; the roadmap is in [Plan.md](Plan.md), protocol findings are in
[docs/devices](docs/devices/README.md).

## Download

1. Download `hubless.exe` from [Releases](https://github.com/TetRex/Hubless/releases/latest).
2. Close G Hub completely (`lghub`, `lghub_agent`, `lghub_system_tray`; the `lghub_updater` service doesn't get in
   the way): the two programs can't control the devices at the same time.
3. Run `hubless.exe`. To start it with Windows, turn that on in Settings → General.

The exe is unsigned, so Windows SmartScreen may warn on first launch.

## Supported devices

| Device | Type | Connection | Status | What works |
|---|---|---|---|---|
| PRO X Superlight 2 | Mouse | LIGHTSPEED receiver, USB | Tested | Battery, DPI (presets and any value), polling rate up to 8 kHz, lift-off distance, onboard/host mode |
| PRO X TKL LIGHTSPEED | Keyboard | LIGHTSPEED receiver, USB | Tested | Battery, brightness, lighting effects and colours, colours of single keys (ANSI and ISO), lighting timeout |
| PRO X 2 LIGHTSPEED | Headset | USB adapter | Tested | Battery, sidetone, auto power-off, built-in 5-band equalizer with presets |
| G903 LIGHTSPEED | Mouse | LIGHTSPEED receiver | Partly tested by users | Battery (from voltage), DPI, polling rate |
| G815 | Keyboard | USB | Partly tested by users | Brightness, lighting effects on all zones |
| Other HID++ 2.0 mice and keyboards | Mouse, keyboard | LIGHTSPEED receiver, USB | Should work | Whatever the device supports: battery, DPI, polling rate, brightness, lighting |
| Other Centurion headsets | Headset | USB adapter | Should work | Battery, sidetone, auto power-off |

**Tested** means checked on hardware by the author; **partly tested by users** means the device is found and the main
features were confirmed through user reports; **should work** means the device uses the same protocol, but nobody has
reported on it yet. Unifying and Bolt
receivers are detected but not tested. Bluetooth is not supported.

Your device isn't in the table, or something is missing? Send a device report, it helps add support:

1. Close G Hub completely.
2. Open **Settings → Advanced → Create report** (or run `hubless --report`).
3. Click **Open GitHub issue** and attach the report file (or paste its text using the **Copy** button).

The report only reads data from the devices and contains no serial numbers or personal data.

## Features

- **Tray:** the battery level of every device in the tooltip; a dot on the icon and a notification when a battery
  drops below the threshold you choose (15% by default), and a notification when a device has fully charged.
  Any click on the icon opens the panel.
- **Panel:** quick controls for every device. The header has the profile list, the settings (gear) and Exit (power)
  buttons.
  - Mouse: DPI presets plus any value via the slider or input field, polling rate.
  - Keyboard: brightness, lighting effect (static, breathe, cycle, wave, ripple) and colour, plus your own colours.
  - Headset: sidetone (how loud you hear your own voice) and the equalizer preset.
- **Settings window** (the gear in the panel):
  - General: interface language (English or Russian), start with Windows, battery notifications, update check
    (optionally at startup).
  - Devices: the order of devices in the panel and which ones it shows, plus device settings: mouse onboard/host mode
    and lift-off; keyboard lighting timeout and an editor for the colours of single keys (click or drag over the
    keyboard layout); headset auto power-off and the 5-band equalizer with built-in and your own presets.
  - Profiles: create profiles and bind them to programs.
  - Advanced: logs and the device report.
- **Profiles:** sets of mouse, keyboard (including key colours) and headset equalizer settings. Switch them from the
  panel header, or let them switch automatically while a bound program's window is active
  (`hubless profile bind <name> cs2.exe`).
- **Solaar-style keyboard lighting:** while an effect is selected, Hubless controls the lighting itself and turns it
  off when you're idle; on exit, control goes back to the keyboard.
- **Onboard and host:** in onboard mode the mouse settings live in the mouse's memory and work without Hubless; in host
  mode Hubless applies them every time the device connects.
- **Privacy:** Hubless sends nothing anywhere. The only network request is the update check: when you press the button,
  or at startup if you turn that on.

Not supported yet: Game Mode, writing profiles to the mouse's onboard memory, macros and key remapping. Blue VO!CE and
DTS are not planned: they are audio processing inside G Hub itself, not commands sent to the device.

## Command line

```
hubless                       run in the tray
hubless --dump [file]         devices and HID++ feature tables (JSON and text)
hubless --report              device report for a GitHub issue
hubless --settings [section]  open the settings window of the running Hubless (devices, profiles, eq, keys, advanced)
hubless --quit                quit the running Hubless (like Exit in the panel)
hubless mouse ...             DPI, polling rate, lift-off, onboard/host mode
hubless keyboard ...          brightness, lighting and effects, key colours (keyboard keys wasd ff0000)
hubless headset ...           sidetone, auto power-off, equalizer
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

- `%APPDATA%\Hubless\config.toml`: app settings (log level, HID tracing, language, battery notifications, update check
  at startup).
- `%APPDATA%\Hubless\devices.toml`: device settings, profiles, the panel layout, your keyboard colours and equalizer
  presets; written by Hubless itself.
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
