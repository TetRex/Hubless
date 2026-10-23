# Hubless — a lightweight G Hub replacement for Windows

> A plan for Claude Code: read it in full before starting, work phase by phase, and stop after each phase to show the result.
> Status as of 2026-10-20: phases 0–6 are closed (except items waiting for a capture or a hardware check); phase 7 (other devices) is in progress.
> Protocol findings and captures are in `docs/devices/README.md`.

## 1. Goal

A Windows tray app that controls the main functions of Logitech devices without G Hub.
Tested on the user's three devices:

| Device | Connection | What Hubless can do | Not done |
|---|---|---|---|
| PRO X Superlight 2 (mouse, HID++ name "PRO X 2") | LIGHTSPEED receiver `C54D` / USB-C | battery, DPI (buttons and any value; separate Y in the CLI), polling rate 125 Hz–8 kHz, lift-off (medium/high), onboard/host, onboard memory backup | writing to onboard memory |
| PRO X TKL LIGHTSPEED (keyboard) | LIGHTSPEED receiver `C547` / USB-C | battery, brightness, lighting: static colour and built-in effects, colours of single keys, turning off when idle | Game Mode (deferred) |
| PRO X 2 LIGHTSPEED (headset) | its own adapter `0AF7`, Centurion protocol | battery, sidetone, auto power-off, built-in equalizer, appearing/disappearing in the tray | — |

The mouse and keyboard can share one receiver or use separate ones; both setups are supported.
Other Logitech devices with the same HID++ 2.0 are most likely already found and partly working; see phase 7.

## 2. Non-functional requirements (the main ones)

- **Idle (panel closed): < 15 MB RAM, ~0% CPU.** With the panel open: < 40 MB. No short-interval polling loops. Everything is event-driven.
  Measured 2026-10-20: ~13 MB in Task Manager (4 MB private), 0 CPU both idle and with the panel open.
- Poll the battery at most once every 5 minutes, plus react to HID++ notifications from the devices.
- Hotplug through `WM_DEVICECHANGE` / `RegisterDeviceNotification` in a hidden window, not through periodic enumeration.
  A device that is asleep at startup is picked up by the receiver's connection notification (`0x41`).
- The control panel opens on a click on the tray icon **in under 150 ms** (measured: first frame 35 ms when the window is created, 6–12 ms when reopened). The window is created lazily and destroyed if the panel stays hidden for more than 60 s. The settings window is also created on demand and destroyed when closed.
- A single exe (release ~10 MB) without a separate installer. Run from anywhere else, it installs itself for the user (2026-10-22, `install.rs`): it copies itself to `%LOCALAPPDATA%\Programs\Hubless`, adds a Start menu shortcut and an entry in Settings → Apps (HKCU `...\Uninstall\Hubless`, "Uninstall" runs `hubless --uninstall`), points start with Windows to the installed copy and starts it with `--installed` (a notification says the download can be deleted). A newer download asks the running copy to quit and replaces it. Uninstalling removes the program folder, shortcut, Apps entry and start with Windows, and keeps the settings. Builds inside a Cargo `target` directory and `HUBLESS_PORTABLE=1` run in place.
- Files:
  - `%APPDATA%\Hubless\config.toml`: app settings (log level, HID tracing, language, battery notifications, update check at startup); the user edits it, the app only changes its own keys line by line;
  - `%APPDATA%\Hubless\devices.toml`: device settings, profiles and the panel layout, written by the app;
  - `%APPDATA%\Hubless\backups\`: onboard memory backups;
  - `%LOCALAPPDATA%\Hubless\logs\`: logs; `%LOCALAPPDATA%\Hubless\reports\`: device reports.
- Start with Windows through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (settings window, "General").
- Hubless doesn't go online, except for the update check: one request to the GitHub API (`releases/latest`, system WinHTTP) when you press "Check for updates" in the settings ("General") and, if you turn on "Check for updates at startup" there (`check_updates`, off by default), once 30 s after startup. There are no periodic checks.

## 3. Stack

- **Rust** (stable, target `x86_64-pc-windows-msvc`); release: `opt-level = "z"`, LTO, `codegen-units = 1`, `strip`, `panic = "abort"`.
- `hidapi` (`windows-native`): HID.
- `tray-icon` 0.26: the tray icon (its menu is not used).
- `windows` 0.62: hidden window, `WM_DEVICECHANGE`, DWM (Mica, rounded corners), system theme and accent, notifications (`Shell_NotifyIcon` balloons: WinRT toasts don't show for an unpackaged exe), foreground window hook, registry, clipboard, WinHTTP.
- **Slint** 1.18: UI. winit backend, **software renderer**, `fluent` style. Translations are built into the exe (`@tr` + `.po`).
- `serde` + `toml` + `serde_json`: configs and dumps.
- `tracing` + `tracing-appender`: logs with daily rotation.

If some crate gets in the way of the memory budget, propose a replacement, but don't change the stack silently.

## 4. Architecture

```
┌──────────────────────── hubless.exe ────────────────────────┐
│ main thread: Slint event loop (pumps Win32 messages)        │
│   ├─ App (app.rs): AppState, AppEvent handling              │
│   ├─ tray: icon, notifications                              │
│   ├─ hidden window: WM_DEVICECHANGE, WM_SETTINGCHANGE,      │
│   │                 WM_APP+n from a second instance and CLI │
│   ├─ foreground hook (SetWinEventHook): auto profiles       │
│   ├─ panel (on demand)       ├─ settings window (on demand) │
│            ▲ app::post(AppEvent) = invoke_from_event_loop   │
│ device workers (one thread per role):                       │
│   ├─ Mouse, Keyboard: hidpp::Transport + reader threads     │
│   │    for every HID collection                             │
│   └─ Headset: Centurion adapter (its own protocol)          │
│ report thread (on button press): queries all devices        │
│ update thread (on button press or at startup)               │
└─────────────────────────────────────────────────────────────┘
```

Crate layout (actual):

```
src/
  main.rs            # arguments: (none) | --dump | --report | --settings | --quit | mouse/keyboard/headset/profile
  app.rs             # main thread: state, events, panel and settings window actions
  cli.rs             # CLI subcommands (output in English)
  dump.rs            # queries all devices (text + JSON) for --dump and the report
  report.rs          # device report: header, saving, clipboard, issue form URL
  update.rs          # update check (GitHub releases API over WinHTTP)
  i18n.rs            # interface language, translations of strings built in Rust
  settings.rs        # devices.toml: device settings, profiles, colours, EQ presets
  settings_window.rs # settings window
  eq_presets.rs      # headset equalizer presets
  keyboard_layout.rs # keyboard layouts (TKL ANSI and ISO) for the key colour editor
  headset_eq.rs      # headset equalizer protocol and filter coefficients
  foreground.rs      # foreground window hook, list of running programs
  hidden_window.rs   # system messages and inter-process commands
  config.rs, logging.rs, single_instance.rs, state.rs, theme.rs, timeutil.rs, win.rs
  tray/              # icon (drawn in code), start with Windows, notifications
  panel/             # panel life cycle, placement, Mica, animations, outside-click hook
  hidpp/
    transport.rs     # collections, send/receive, reply matching, error parsing
    message.rs, root.rs, receiver.rs
    features/        # battery, brightness, device_info, device_name, dpi, onboard, per_key, report_rate, rgb_effects
  devices/
    discovery.rs     # discovery by feature table and device type; sleeping devices behind a receiver
    manager.rs       # mouse and keyboard threads, applying settings, Solaar-style lighting
    headset.rs       # headset thread
    mouse.rs, keyboard.rs, registry.rs
  centurion.rs       # headset protocol
ui/
  panel.slint, settings.slint, keyboard_editor.slint, widgets.slint, theme.slint, cards/device_card.slint
translations/ru/LC_MESSAGES/hubless.po
docs/devices/        # dumps and protocol findings
```

## 5. HID++ 2.0: notes and pitfalls

- Logitech VID `0x046D`. HID++ lives on vendor collections with usage page `0xFF00` (`0xFF43` on G-series keyboards over the cable).
- Reports: short `0x10` (7 bytes), long `0x11` (20 bytes), very long `0x12` (64 bytes).
- Format: `[report_id][device_index][feature_index][function<<4 | sw_id][params...]`.
- `device_index`: `0xFF` is a direct wired connection or the receiver itself; `0x01…0x06` are devices behind a receiver.
- **Windows:** hidapi opens every top-level collection as a separate path; short and long reports are in different collections. All collections of the interface are opened, and replies are read from all of them.
- A non-zero `sw_id` tells replies to our requests apart from notifications (`sw_id = 0`).
- **Errors:** layout `[report][device][0xFF or 0x8F][feature_index or sub_id][function|sw_id][code]`, for both HID++ 2.0 and 1.0. Until 2026-10-19 it was parsed wrong, and every error looked like a 2 s timeout.
- Timeout ~1 s with one retry; a quick mode of 150 ms without a retry is used for empty receiver slots. A sleeping device is an "offline" state, not an error.
- Feature indices differ between the cable and the receiver: always resolve them through Root `GetFeature`, cached for the session.
- LIGHTSPEED receivers don't answer a ping at `0xFF` at all.
- Devices connecting/disconnecting behind a receiver: HID++ 1.0 notification `0x41`.

### Found on hardware

- `0x2202` DPI: the list pages have odd lengths, so concatenate the bytes of all pages before splitting them into words. The PRO X 2 mouse rejects lift-off `00` (LOW per Solaar) with INVALID_ARGUMENT, so the panel offers only medium/high.
- `0x8061` polling rate: the read (function 2) takes the link type, `00` cable / `01` wireless, and each has its own rate. The write (function 3) is for the current link; the mouse confirms it with an event `[link, index]`. Verified: 500 Hz, 1 kHz, 8 kHz through the receiver.
- The LIGHTSPEED receiver sometimes rejects a report write (`os error 31`, 2026-10-22, while the sleeping mouse was reconnecting), and discovery lost the mouse or its name. The transport retries the write up to 3 times, 50 ms apart; a sleeping device without a name from the receiver keeps its previous name.
- After the cable is unplugged, the mouse returns to wireless before Hubless starts listening to the receiver: the `0x41` connection notification is missed, and the card stayed "inactive" (2026-10-22). Now, while waiting for a sleeping device, there are three ping rechecks (after 1, 3 and 8 s), and any message from the device through the receiver (not only `0x41`) wakes it up.
- After a reboot the mouse disappeared for good once it woke up (2026-10-23): the search right after the wake-up read the receiver's pairing info in the 150 ms quick mode, the receiver answered late, and the paired mouse looked absent, so the card was removed and nothing searched again. Empty slots answer at once with error `0x09`, so pairing info is now read with the normal timeout and a retry (the quick mode dates from before error replies were parsed), and after a search that found nothing the worker searches again after 1, 3 and 8 s.
- `0x8071` lighting: when the lighting is awake, the keyboard answers `power_on` with error `0x05`, so the command is best-effort. The keyboard's effects are the versions with saturation (`0x15`–`0x17`).
- `0x4523` (Game Mode) and `0x8081` (per-key): see phase 3 and `docs/devices/README.md`.

### Features (from real dumps)

| ID | Purpose | Where it's used |
|---|---|---|
| `0x0000` / `0x0001` | Root / FeatureSet | all |
| `0x0003` / `0x0005` | firmware / name and type | dump, device discovery |
| `0x1004` | battery (unified) | mouse, keyboard |
| `0x2202` | extended DPI (X/Y, lift-off) | mouse |
| `0x8061` | polling rate (up to 8 kHz) | mouse |
| `0x8060` | polling rate (old) | keyboard (read only) |
| `0x8040` | brightness | keyboard |
| `0x8071` | lighting effects | keyboard |
| `0x8081` | per-key lighting | keyboard: colours of single keys (editor in the settings, CLI) |
| `0x8100` | onboard profiles | mouse, keyboard |
| `0x4523` | disabling keys (Game Mode) | keyboard (partly decoded) |
| `0x4540` | keyboard layout | keyboard (read in `keyboard status`, the code is not decoded) |
| `0x8110` | mouse button events | not used |

**Blacklist:** DFU and firmware update (`0x00C0`–`0x00C3`, `0x00D0`): never call them; `Device` rejects them.

## 6. Onboard and host

- G Hub often leaves devices in host mode.
- Two modes for the mouse and keyboard:
  1. **Onboard**: the device runs its own profile from memory; Hubless applies nothing.
  2. **Host**: Hubless applies the settings from `devices.toml` every time the device connects. Any change of DPI, polling rate or lift-off from the panel switches the mouse to host; lighting with an effect switches the keyboard to host, and it goes back to onboard on exit.
- **Take a full backup before the first write to onboard memory** (`hubless mouse backup` → `%APPDATA%\Hubless\backups`). Writing to onboard memory is not implemented yet.

## 7. Control panel

The model is the Windows 11 flyouts: compact, fast, no "Apply" buttons.

### Behaviour
- **A click on the icon** (left or right) opens/closes the panel. There is no context menu (since 2026-10-22): "Exit" is the ⏻ button in the panel header next to ⚙. Battery levels are in the icon's tooltip; settings, start with Windows and the log folder are in the settings window.
- The panel sits above the icon at the edge of the taskbar (`Shell_NotifyIconGetRect`, `ABM_GETTASKBARPOS`), taking into account the taskbar side, monitors and DPI (PerMonitorV2). The position is recalculated when the height changes.
- No title bar and no taskbar button (`WS_EX_TOOLWINDOW`, no `WS_SYSMENU`), always on top. It closes on focus loss, `Esc`, a second click on the icon and a click on another app's window (a `WH_MOUSE_LL` hook only while the panel is shown: Windows doesn't always give the panel focus, and then there is no focus loss; 2026-10-20).
- Animations: the content fades in over 150 ms, the window slides in 10 px from the taskbar; closing takes 100 ms. With Windows "Animation effects" turned off, there is no animation.
- Changes apply immediately; sliders after a 150 ms pause; a write error shows a short message in the card, and the value goes back to what was read.

### Appearance
- Width 360 px, height by content up to the height of the work area; if it doesn't fit, the cards scroll and the header stays in place.
- Theme and accent follow the system and switch on the fly. Background: **Mica** (`DWMSBT_MAINWINDOW`); Acrylic doesn't render on this window (verified). Without system support, a solid colour.
- System fonts and icons (`Segoe UI Variable`, `Segoe Fluent Icons` with a fallback to the older ones).
- All colours, sizes, radii and durations come only from `theme.slint`.

### Layout (actual)
```
┌─────────────────────────────────────┐
│ Hubless          [profile ▾]  ⚙  ⏻  │
├─────────────────────────────────────┤
│ 🖱 PRO X 2               📶 95% ▭ │
│ DPI  [400][800][1600][3200]         │
│ ━━━━━━○━━━━━━━━━  [ 800 ]           │
│ Polling  [1K][2K][4K][8K]           │
├─────────────────────────────────────┤
│ ⌨ PRO X TKL              📶 97% ▭ │
│ Brightness ━━━━━━━━━○               │
│ Effect [Static ▾]                   │
│                    ● ● ● ● ● ●  +   │
├─────────────────────────────────────┤
│ 🎧 PRO X 2 LIGHTSPEED    📶 32% ▭ │
│ Sidetone ○━━━━━━━━━                 │
│ Equalizer [Bass boost ▾]            │
└─────────────────────────────────────┘
```
- The profile list in the header appears when there is more than one profile; ⚙ opens the settings window.
- DPI: buttons plus a slider on a logarithmic scale and an input field; the value snaps to the nearest one the mouse allows.
- Keyboard palette: six standard colours and your own ones (up to 10, `colors` in `devices.toml`, shared by all profiles) in a second row. The "+" circle at the end of the row opens an editor in the card: a preview and R/G/B sliders starting from the current colour; "Add colour" puts it in the palette and turns it on. If one of your own colours is selected, the editor has a button to remove it from the palette.
- The keyboard effect list has no "Keyboard" item (2026-10-22): when the keyboard controls the lighting itself or the lighting is off, nothing is selected.
- Card header: the device icon and name, a connection icon (USB plug, or an antenna for wireless), and at the end the battery: percentage and a small battery with a fill (red when low), plus a charging icon; there is no separate battery row (2026-10-22).
- Headset: the built-in equalizer preset, built-in ones ("Flat", "Bass boost", "Voice", "FPS") and your own ones; the selected one is recognized from the curve read from the headset, and if none matches, the item is "Custom" (2026-10-22). Your own presets are created in the settings.
- A disconnected device is a grey "Inactive" card without controls.
- Only what the device supports according to its feature table is shown.
- Rare settings are not in the panel but in the settings, "Devices" section (2026-10-22): the chevron button in the mouse row expands host/onboard and lift-off, in the keyboard row the idle lighting timeout and the key colour editor, and in the headset row auto power-off and the equalizer. One row is expanded at a time; the rows below move down by the height of the settings, and dragging or the arrow keys collapse it.
- **Not verified:** keyboard navigation (Tab, arrows, Enter/Space). The light theme is verified (`HUBLESS_THEME=light|dark` overrides the Windows theme for testing).

### Tray icon
- Monochrome, for light and dark taskbars, with a dot and a notification when a battery is below the threshold. The threshold is in the settings ("General", 5–30%, 15% by default; `low_battery` in `config.toml`).
- A "Fully charged" notification when a device that was charging while Hubless was running has finished charging (one per charge; can be turned off in "General", `notify_charged`).
- Tooltip: the battery level of every connected device, one line per device.

## 8. Settings window

- A normal framed window (its own class `Hubless.Settings`), opened by ⚙ in the panel or `hubless --settings [devices|profiles|eq|keys|advanced]` (`eq` and `keys` open "Devices" with the headset or the keyboard expanded).
- Sections in a vertical list on the left:
  - **General:** language (English / Русский; until one is chosen, the Windows language), start with Windows, the low battery threshold, the "fully charged" notification (stored in `config.toml`); the version and "Check for updates" (a new version shows a "Download" button that opens the release page), "Check for updates at startup" (a new version gives a notification and the same "Download" button). The updates moved here from "Advanced" on 2026-10-22.
  - **Devices:** all devices in the panel's order (the keyboard row expands the idle lighting timeout and the key colour editor, see phase 3): a grip, icon, name, "Connected / Inactive" and a "Show in the panel" switch. The order is changed by dragging a card (the others move apart with an animation) or with ↑/↓ on the focused grip. It is stored in `devices.toml`, `[panel]` (`order`, `hidden`: the keys `mouse`/`keyboard`/`headset`), shared by all profiles. A hidden device keeps working (its settings are applied, its battery is in the tooltip and the low battery warning); the order also applies to the tooltip. If everything is hidden, the panel shows a hint. The headset row has an equalizer button: it expands the equalizer under the row (the rows below move down by a fixed height; dragging collapses it first). Above the sliders are the presets: choosing one, "Save as preset" (a name; the same name overwrites) and deleting your own. Your own presets are `[[eq_preset]]` in `devices.toml` (up to 20, shared by all profiles).
  - **Profiles:** cards; the selected one expands: programs (from the list of running ones or by exe name), "Make active", rename, delete. "New profile" is a copy of the current one named "Profile N".
  - **Advanced** (`hubless --settings advanced`, formerly "Device report"): logs, "Open log folder" and "Delete all logs" (all files are deleted, the current one is truncated because the process writes to it), and the device report (see phase 7).
- Edits in the panel and in the settings window go to the active profile, the one applied to the devices: switched on by the foreground program, otherwise the one selected by hand. Hubless's own windows don't switch profiles (the foreground hook skips them), so with the settings window open over a game, the game's profile is the one being edited; the key colour editor shows its name.

## 9. Languages

- The interface (panel, settings window, icon tooltip, notifications) is in English and Russian, switching on the fly.
- Source strings in the code are English: `@tr("...")` in `.slint` + `translations/ru/LC_MESSAGES/hubless.po`; strings built in Rust use `i18n::tr("...")` + the table in `src/i18n.rs`. A new interface string comes with its translation right away.
- The CLI is English only. Code comments are in English (since 2026-10-22, always, including new ones); log messages are in Russian. Documentation (this plan, `README.md`, `docs/`) is in English.

## 10. Headset

- PRO X 2 LIGHTSPEED: the Centurion protocol (not HID++): 64-byte reports `0x51` on usage page `0xFFA0`, requests to the headset go through the adapter's bridge feature `0x0003`. Reference: HeadsetControl.
- Sidetone `0x0604`: read `01 01 <level>`, write `[01 <level>]`, 0–100. Auto power-off `0x0108`: `[minutes]`, 0 = never.
- Blue VO!CE and DTS:X are **out of scope**: they are audio processing on the computer. The only equalizer is the one built into the headset (below).
- **The headset's built-in equalizer** `0x0636` (ONBOARD_EQ), done 2026-10-20 from a G Hub capture: 5 bands (80/240/750/2200/6600 Hz), in the headset card of the "Devices" section (a button expands sliders from −12 to +12 dB and the presets; `hubless --settings eq`), `hubless headset eq`. A preset can also be chosen in the headset card in the panel. **The curve is part of the profile** (2026-10-22): `[headset] eq` in `devices.toml`; a change (preset or sliders) is saved in the active profile together with the save in the headset, and on a profile switch, as well as when the headset connects, Hubless writes the new profile's curve (its own, otherwise Default's; a profile without one leaves the headset as is). The first curve saved in another profile also gives "Default" the curve that was on before. Hubless computes the filter coefficients itself; the protocol is in `docs/devices/README.md`. Writing verified 2026-10-21; while the sliders are moving, the curve from the headset is not put into them, and the save (`80`) happens after 2 s of quiet. The read returns the saved curve: a write without `80` isn't visible to it (2026-10-22), so until the save Hubless keeps its own edit in the state.

## 11. Licenses

Solaar (GPL-2.0), HeadsetControl (GPL-3.0), OpenRGB (GPL-2.0) and libratbag (MIT) are **protocol references**.

**Decision:** `GPL-3.0-or-later`, Copyright (C) 2026 TetRex. The text is in `LICENSE`; `license = "GPL-3.0-or-later"` in `Cargo.toml`; source files carry `// SPDX-License-Identifier: GPL-3.0-or-later`. When porting code from Solaar/HeadsetControl, keep their copyright in the ported fragments.

## 12. Working rules for Claude Code

- Work in phases. At the end of a phase: what was done, how to check it, what questions remain.
- **Reads from devices may be run on your own** (`--dump`, `--report`, `status`).
- **Any write to a device** must first be shown as hex packets (`--dry-run`), and you wait for confirmation until the feature has been verified by hand. For one-off experiments: `hubless mouse raw <feature>:<function>[s] <bytes> , …`.
- Don't guess the parameters of undocumented functions; propose sniffing (section 15).
- Check hardware test results in the log (`HUBLESS_LOG=debug`), not only by the user's words.
- Before working with devices, make sure G Hub is closed (`lghub`, `lghub_agent`, `lghub_system_tray`; the `lghub_updater` service is always there and doesn't get in the way). Don't kill it without asking.
- Restart the tray with `hubless --quit` (a normal exit that hands the lighting back to the keyboard), not by killing the process.
- UI: after noticeable changes, take a screenshot (or ask the user) and compare with sections 7–8. Don't send keystrokes or clicks while the user is working at the computer.
- Commit and push only when the user asks.

## 13. Phases

### Phase 0 — Skeleton ✅
Tray, menu, placeholder panel, config, logs, start with Windows, single instance, CI (fmt, clippy without warnings, build).

### Phase 1 — HID++ core and diagnostics ✅
Transport, discovery of receivers and wired devices, `--dump` (text + JSON). The dumps are in `docs/devices/`.

### Phase 2 — Mouse ✅ (except writing to onboard memory)
- Battery, DPI, polling rate, lift-off, onboard/host, onboard memory backup, CLI `hubless mouse`, battery in the tray and the low battery notification.
- **Remaining:** writing to onboard memory (mouse profiles that work without Hubless), after a backup and with packet confirmation.
  The PRO X 2 uses profile format 7 (backup 2026-10-19): DPI in the profile sector as groups of five `[X(2) Y(2) lift-off]`
  from byte 4 (800/1200/1600/2400/3200); Solaar has decoded formats up to 5. The other fields and the write order
  (`0x8100` memoryWriteStart/Write/End, CRC) are not guessed: a capture is needed (G Hub, onboard memory mode,
  changing one DPI level and saving it to the mouse).

### Phase 3 — Keyboard ✅ (except the deferred items)
- Battery, brightness, Solaar-style lighting (Hubless holds `0x8071` and turns the lighting off when idle: idle → sleep → wake verified), static colour and built-in effects (breathe, cycle, wave, ripple: verified), CLI `hubless keyboard`.
- The "off" effect (Disabled) verified 2026-10-22: from the CLI and by the tray on a profile switch (the keyboard goes dark,
  and in a window with another profile the lighting comes back).
- **Turning the lighting off after idle** (2026-10-22): "Settings → Devices → keyboard": 5, 15 or 30 min, or
  never (`lighting_off_min` in the profile; unset means the firmware sleep timer). Hubless only reads the firmware timers
  (`0x8071` fn7): the write format is unknown. So the timeout can't be shorter than the firmware idle timer (5 min on the
  PRO X TKL): the idle event starts Hubless's own timer, and `idle_mode` and sleep are sent together when it runs out; a key
  press before that sends nothing. **Not verified:** whether the firmware turns the lighting off by itself on its own sleep
  timer when Hubless waits longer (15, 30 min, "never").
- **Deferred (the user's decision):**
  - **Game Mode**: `0x4523` function 0, a bitmask of keys (16 bytes); the "key → bit" table is known for 10 keys, with no formula. A capture is needed: all keys one by one, row by row. The firmware blocks Win and Menu.
- **Per-key**: `0x8081` function 5 `[from zone, to zone, R, G, B]` + function 7 `[00]`, zone = HID usage − 3; also works wirelessly.
  Done (2026-10-20): `features/per_key.rs`, CLI `hubless keyboard keys <KEYS> <RRGGBB>` (adjacent zones as one range).
  **Verified 2026-10-22** wirelessly (`keys wasd ff0000`): all packets acknowledged (the fn5 reply is the first zone number),
  WASD red, the other keys keep the current effect.
  **Done 2026-10-22: key colours in the profile, and an editor:**
  - The keyboard's zone map: fn0 `[00, page]`, pages 0–2, a bitmap (as in Solaar). The PRO X TKL has 99 zones:
    1–79 (HID usage − 3), 97–103, 104–111 (modifiers `0xE0`–`0xE7` are not "usage − 3" but 104–111) and
    150, 152–155 (probably the media keys and the buttons above the F row; which ones exactly is not verified).
  - The colours are stored in the profile: `[keyboard.keys]` in `devices.toml`, key name → `rrggbb`. The tray applies the
    frame after the effect: when it takes over the lighting, after waking up and on a profile switch (writing an effect
    resets the per-key frame). Only with the static effect or the lighting off; over animations it's not verified.
  - The editor: "Settings → Devices → keyboard" (`hubless --settings keys`): a TKL keyboard layout
    (`keyboard_layout.rs`, only keys whose zones the keyboard has), the brush is a palette colour or the eraser
    (back to the effect colour), click and drag, "Fill all" / "Clear all". Rust works out which key was hit from the
    layout units.
  - CLI: `hubless keyboard keys <KEYS> <RRGGBB>` saves the colours in the profile; `keys <KEYS> none` and `keys clear`
    remove them. Verified 2026-10-22: painting from the editor, restoring after a tray restart.
  - ANSI and ISO layouts: a switch in the editor (`keyboard_layout` in `devices.toml`, shared by all profiles):
    ISO has a tall Enter, the `0x32` key to its left (zone 47) and `0x64` next to the short left Shift (zone 97).
    It was needed after the user's photo: on an ISO keyboard the `\|` key next to Enter didn't change colour.
  - Not done: a full-size layout (numpad, G keys) and the keys 99–103, 150–155.

### Phase 4 — Headset ✅
Centurion adapter, battery, sidetone, auto power-off (verified), CLI `hubless headset`, appearing/disappearing in the tray on the bridge event. Built-in equalizer with presets (section 10).

### Phase 5 — Control panel ✅
Cards for all devices, slider pauses, errors in the card, scrolling, Mica, animations. Measurements are in section 2.
The light theme verified 2026-10-20 with `HUBLESS_THEME=light` (screenshots of the panel and the settings; `PrintWindow` doesn't capture the Mica background).
**Not verified:** keyboard navigation.

### Phase 6 — Polish ✅
- Profiles in `devices.toml` (`[[profile]]`, the root is "Default"), choosing one in the panel, CLI `hubless profile`, automatic switching by the foreground exe: verified. A profile holds the mouse and keyboard settings (with key colours) and the headset equalizer curve.
- Settings window, RU/EN languages, CLI in English, release build, README.
- Order and visibility of the panel cards: the "Devices" section of the settings window; hiding and reordering (with buttons) verified 2026-10-20. **Not verified by hand:** dragging and the arrows on the grip.
- The exe and settings window icon (2026-10-20): a white mouse outline on blue, drawn by the tray code (`tray/icon.rs`);
  `res/hubless.ico` is generated by `cargo test --release write_exe_icon -- --ignored`.

### Phase 7 — Other Logitech devices (started)
The goal: other people's HID++ 2.0 mice and keyboards should work without per-model changes, and verified models fully.
- **Done:** the device report: a section of the settings window and `hubless --report`: the same query as `--dump` (read-only; serial numbers and unit IDs are masked), plus the Hubless and Windows versions and whether G Hub is running. It is saved to `%LOCALAPPDATA%\Hubless\reports`, with a preview, copying, and a button for the issue form (`.github/ISSUE_TEMPLATE/device-report.yml`). Instructions are in the README.
- **Done (2026-10-20): automatic detection.** There are no hard-coded cards: a card exists while the device is connected or paired with a connected receiver/adapter (asleep: a grey "Inactive" card); when the receiver/cable/adapter disappears, the card disappears. Names come from the device (`0x0005`); for a sleeping device, from the receiver register `0xB5`/`0x4N` (verified on both LIGHTSPEED receivers); otherwise the type name. A receiver is recognized without a PID table: a product string "…Receiver" or an HID++ 1.0 reply to a ping at `0xFF`; without pairing info (Bolt), slots 1–6 get a quick ping. The headset is any Logitech adapter with a `0xFFA0` collection and a Centurion bridge; its name is the product string. The connection label is "Wireless"/USB. Nothing is written to new users' devices until they change a setting themselves. **Not verified on hardware:** Unifying, Bolt, other Centurion headsets, a device asleep at startup.
- **Next:**
  1. Unknown models: full controls for now; an "experimental" mark is needed (item 5).
  2. Several devices of the same type (now the first mouse, the first keyboard and one headset are taken): a card for each. The key in `[panel]` then becomes unique per device (for example, type + PID); old type keys are read as before.
  3. Old feature versions: `0x1000` (battery), `0x2201` (DPI), `0x8060` (polling rate), `0x8070` (lighting).
     Done after the G903 report (2026-10-20, `docs/devices/README.md`): battery `0x1001` (estimate from voltage),
     reading and writing DPI `0x2201` (fn3) and polling rate `0x8060` (fn2) per the Logitech spec and Solaar,
     without a switch (2026-10-20, the user's request; at first there was an "experimental" switch).
     Host mode is never turned on for such mice: in it the mouse disables its DPI button (G903 on 0.1.3), so the writes
     go in onboard mode and the host/onboard switch is hidden in the card.
     The mouse card rereads DPI/polling rate on events from `0x8100`/`0x2201`/`0x2202`/`0x8060`/`0x8061`;
     according to the G903 user, changing DPI with the mouse button shows in Hubless.
     HID++ on usage page `0xFF43` (G815 and other G-series keyboards over the cable) is found; the lighting effect goes to
     all zones (on the G815 zone 0 is the logo).
  4. Unifying and Bolt receivers (Bolt reads its list of paired devices differently); the table now has only the two LIGHTSPEED ones.
  5. Writes on unverified models: only safe and reversible ones (DPI, polling rate, onboard/host), with an "experimental" mark and a switch in the settings. Don't touch the onboard memory of other models.
     There is no switch yet: the user asked for `0x2201`/`0x8060` to be written on the G903 right away.
  6. Headsets from other lines, one at a time, per HeadsetControl (Centurion, HID++ on G533/G733/G535, Astro).
  7. Every verified model: a dump in `docs/devices/` and an entry in the list of verified models.

## 14. Device report: what's in it

- Header: Hubless version, Windows version, whether G Hub is running, time.
- For every Logitech interface: PID, collections, protocol, feature tables, firmware, receiver registers (pairing without serial numbers); for the headset, the adapter and headset features and raw frames.
- It doesn't contain USB serial numbers, unit IDs, paths or the user name.

## 15. Sniffing procedure (for unknown functions)

1. Install Wireshark with the USBPcap component and restart Windows; start G Hub, quit Hubless (`hubless --quit`), turn off automatic firmware updates in G Hub.
2. Find the bus: the gear next to `USBPcapN` → list of devices; the receiver is "LIGHTSPEED Receiver" (for the user: keyboard `USBPcap1` [2], mouse `USBPcap4` [7]). Select only the device you need.
3. Change **exactly one** setting, wait 2 s, then change the same setting to another value; don't move the mouse during the capture.
4. Save the `.pcapng` to `captures/` (the folder is in `.gitignore`) with a clear name.
5. Decoding: `tshark -r <file> -T fields -e frame.time_relative -e usb.endpoint_address -e usbhid.data -e usb.capdata -e usb.data_fragment`: host requests are in `usb.data_fragment` (endpoint `0x00`), replies in `usbhid.data`; feature indices come from the dumps in `docs/devices/`. A `tools/pcap2hidpp.py` script isn't written yet; it will be needed if there are many captures.

## 16. Out of scope

Macros, OS-level key remapping, Bluetooth transport, firmware updates, Blue VO!CE / DTS / software EQ, lighting sync across devices, macOS/Linux.
