# Device dumps

Made with `hubless --dump <file>` (read-only: Root, FeatureSet, `0x0003`, `0x0005`, HID++ 1.0 receiver
registers; for the headset, Centurion: Root, FeatureSet, bridge state, battery).
USB serial numbers and unit IDs are replaced with `XXXXXXXX` / `XX`.

| File | Connection |
|---|---|
| [receivers.json](receivers.json) / [.txt](receivers.txt) | mouse and keyboard through their own LIGHTSPEED receivers |
| [wired.json](wired.json) / [.txt](wired.txt) | mouse and keyboard over a USB cable |
| [headset.json](headset.json) / [.txt](headset.txt) | headset through its own adapter (Centurion); raw frames in `centurion.frames` |

## Devices

| Device | HID++ name | USB PID (cable) | Receiver (PID) | wPID | HID++ | Firmware |
|---|---|---|---|---|---|---|
| PRO X Superlight 2 | `PRO X 2` | `C09B` | `C54D` | `40A9` | 4.2 | MPM 32.05.B0029 |
| PRO X TKL Lightspeed | `PRO X TKL` | `C352` | `C547` | `40B0` | 4.2 | MPK 23.00.B0008 |
| PRO X 2 LIGHTSPEED (headset) | — | `0AF7` | its own adapter | — | none (Centurion) | — |

## Findings

- **Feature indices depend on the connection.** Over the cable the keyboard has no `0x0007`, so every index
  from the 6th on is shifted; the DFU feature `0x00D0` (wireless) becomes `0x00C2` (cable). Over the cable the
  mouse's `0x1500` becomes `0x00C3`. Indices are always resolved through `Root.GetFeature` and cached for the
  session.
- The receivers don't answer an HID++ 2.0 ping at index `0xFF` (neither long nor short), but they do answer
  register reads. Slots 2–6 don't answer at all on either receiver: these are single-slot LIGHTSPEED receivers.
- While the keyboard is connected by cable, its slot on the receiver stops answering.
- Features for the next phases:
  - mouse: `1004` battery, `2202` DPI, `8061` polling rate, `8100` onboard profiles, `8090` mode status;
  - keyboard: `1004` battery, `8071` effects, `8081` per-key, `8040` brightness, `8100` onboard profiles,
    `8060` polling rate, `4523`: **no `0x4522` from the plan**; the Game Mode candidate is `0x4523` (not studied).
- Not identified (missing from the reference): mouse `2250`, `2251`, `80E0`, `1500`, `1602`; keyboard `40A3`,
  `1B10`, `4523`.
- **The headset is a PRO X 2 LIGHTSPEED, not the PRO X Wireless from the plan.** It has no HID++; the protocol is
  Logitech "Centurion" (reference: HeadsetControl): 64-byte frames with report ID `0x51` on collection
  `FFA0:0001`, interface 3.
  - Adapter: `0000`, `0001`, `0100`, `0003` (bridge, index 3), `010A`. Bridge state `03 7a 00 01 01` means the
    headset is connected.
  - Headset (through the bridge): `0000`, `0001`, `0101`, `0100`, `0104` battery, `010A`, `0108` auto power-off,
    `0604` sidetone, `0602`, `0636` onboard EQ. No `020D` (advanced EQ), `0601` (LED mute), `0621` (lighting).
  - Battery (`0104`, function 0): reply `08 08 00` → 8%; byte 2 = 0 means not charging. The meaning of byte 1 is
    unknown.
  - Sidetone (`0604`), verified 2026-10-19: read (function 0) → `01 01 <level>`; write (function 1) in the v1
    format `[01 <level>]`, level 0–100, reply `01 00 <level>`. Writing 50 → reading `01 01 32`.
  - Auto power-off (`0108`), verified 2026-10-19: read (function 0) → `<minutes>`, 0 = never; write
    (function 1) `[<minutes>]`, empty reply. Writing 30 → reading `1e`.
  - **Equalizer (`0636`, index 9)**: G Hub capture 2026-10-20 (`captures/headset_eq.pcapng`; in the capture the
    headset is USB device 1 on USBPcap4: requests are control SET_REPORTs, replies are interrupts on `0x84`):
    - fn1 `[00]` is the read: `[flag, 05, (frequency u16 BE, gain dB i8, 02) × 5]`, bands
      80/240/750/2200/6600 Hz; flag `80` means the curve is saved in the headset.
    - fn2 is the write, 253 bytes: the same header (flag `00` is an intermediate write, `80` saves; G Hub sends
      `80` last), a constant 11 bytes `05 5a e3 00 03 0e 00 02 00 00 00` and two blocks
      `[01|02, 00 35 00 05 00, 5 biquads × (b0, b1/2, b2, a1/2, a2), gain]` for 44.1 and 48 kHz. Each coefficient
      is Q1.23 in the byte order `[middle, high, 00, low]`. The filters are peaking filters from the Audio EQ
      Cookbook, Q = 2, sorted by gain in descending order; the numerator is normalized by b0 (b0 = 1), the
      product of b0 × 0x04C28F goes into "gain"; the first filter's numerator is multiplied by 0x6B9022
      (≈ −1.5 dB). The model matches all 10 G Hub writes to within 1 LSB, one of them byte for byte (test in
      `src/headset_eq.rs`).
    - A message longer than one frame is split into 60-byte chunks; frame flags are
      `(number << 1) | more_follow`: `01 03 05 07 08`.
    - Writing from Hubless verified 2026-10-21 (sliders in the settings: replies without errors, the `80` save
      goes through).
    - The read (fn1) returns the **saved** curve: after a write with flag `00` it still returns the old one; the
      new one is visible only after a write with `80` (log 2026-10-22: "Flat" over "+6 +3 0 0 0" read back as
      `80 Hz +6` before the save).
  - The layout of the version/flags bytes in the FeatureSet reply is not verified (the table may be wrong);
    the feature IDs match HeadsetControl.
- PRO X 2 mouse, `0x2202` setDpi (function 6), 2026-10-19 through the receiver: lift-off `00` (LOW per Solaar) is
  rejected with error `11 01 ff 09 6x 02` (INVALID_ARGUMENT); `01` (medium) and `02` (high) are accepted. The panel
  has no "low".
- **G Hub captures (2026-10-20, through the receivers; files in `captures/`, not in git):**
  - **Game Mode: `0x4523`** (in Solaar `KEYBOARD_DISABLE_CONTROLS`, not implemented), index `0x0D` on the
    keyboard:
    - G Hub sets the list of blocked keys with function 0: `11 01 0d 0x <bitmask>`, the reply is all zeros.
      Caps Lock → `00 00 00 00 00 01`, Caps Lock + F1 → `00 00 01 00 00 01`, empty list → zeros.
      One key = one bit of the mask (16 bytes). The numbering follows neither HID usages nor lighting zones; it
      looks like positions in the keyboard matrix, with no formula. Capture `keyboard_gamemode_sequence` (keys one
      by one): Esc — byte 2 bit 0, F1 — byte 0 bit 1, F2 — byte 3 bit 0, `1` — byte 6 bit 0, Q — byte 7 bit 0,
      A — byte 8 bit 2, Z — byte 9 bit 2, space — byte 9 bit 0, left Ctrl — byte 11 bit 6; Caps Lock (another
      capture) — byte 5 bit 0. A mismatch: in `keyboard_gamemode_add_two` "F1" gave byte 2 bit 0, like Esc here,
      so in one of the captures the key was a different one. A full table needs a capture of all keys, row by row.
      Win and Menu are not available in G Hub: the firmware blocks them.
    - The Game Mode button on the keyboard sends a function 0 event (sw_id 0): `11 01 0d 00 01` on,
      `… 00 00` off.
  - **Per-key: `0x8081`**, index `0x09`: function 5 `[first zone, last zone, R, G, B]` paints a range
    (`14 14 f8 2f 25`), then function 7 `[00]` applies the frame. Matches Solaar; also works over the receiver.
    Zone `0x14` is the T key: zone number = the key's HID usage − 3 (T = `0x17`; A = 1 … Z = 26).
    Verified from Hubless 2026-10-22 (`hubless keyboard keys wasd ff0000`, wireless): fn5 replies with the first
    zone number (`11 01 09 5x 17 00 …`), fn7 with an empty reply; only the given keys change colour, the others
    keep the current `0x8071` effect. Writing a `0x8071` effect resets the frame (after "off" and back to a static
    colour, WASD were no longer red).
    The zone map is fn0 `[00, page]` (pages 0–2, as in Solaar); after 2 bytes the reply is a bitmap.
    The PRO X TKL has 99 zones: 1–79, 97–111, 150, 152–155. Modifiers (`0xE0`–`0xE7`) are zones 104–111, not
    "usage − 3" (221–228 are not in the map); the Menu key (`0x65`) is 98. What 97, 99–103 and 150–155 are on this
    keyboard is not verified (97 is the ISO key, which ANSI doesn't have).
    The zone map is the same for ANSI and ISO (46 `\`, 47 and 97 are all present), so the user picks the layout.
    The user's PRO X TKL is ISO (tall Enter, `\|` to its left): "Fill all" in the ANSI layout didn't paint the key
    next to Enter (photo 2026-10-22). It is `0x32` (non-US #), zone 47; in the ISO layout it does change colour
    (verified by the user 2026-10-22).
    `0x4540` KEYBOARD_LAYOUT_2, fn0 without parameters → `02 00 …` on this ISO keyboard; no table of layout codes
    was found, so the code is not used.
  - **Polling rate: `0x8061`**, index `0x0C` on the mouse: G Hub sends the same function 3, but as a **short**
    report `10 01 0c 3x 02` (500 Hz), preceded by `0x2202` function 6 (current DPI) and function 7 `[00 01]` (not
    studied). The mouse confirms with the event `11 01 0c 00 <link> <index>`. **Solved (2026-10-20):** function 2
    (read) takes the link type, `[00]` cable and `[01]` wireless, and each has its own rate. Without the parameter
    the cable rate (`03`) was read, so it looked like the 500 Hz write wasn't applied. After the write:
    `[01]` → `02` (500 Hz), `[00]` → `03`. The write works with both long and short reports; `0x2202` function 7
    `[00 01]` is not needed.
  - `0x8110` (index `0x0E`): mouse button press events (`… 0e 00 00 01` / `… 00 00`), unrelated to settings.
- `HID VHF Driver` with a PID equal to the keyboard's wPID/PID is a virtual HID device (probably Windows Dynamic
  Lighting).

## User reports

### 2026-10-20 — G903 + G815 ([reports/2026-10-20-g903-g815.txt](reports/2026-10-20-g903-g815.txt))

Hubless 0.1.0, Windows 11 25H2, G Hub closed. The user called the keyboard a "G915 X", but in the report it is a
`C33F` "G815 RGB MECHANICAL GAMING KEYBOARD" over the cable; there is no G915 X (receiver, Bluetooth) in the report.

- **G903 LIGHTSPEED** (wPID `4087`, receiver `C539` "USB Receiver", HID++ 4.2, MPM 23.01): found.
  Old features: battery `1001` (voltage, no `1004`), DPI `2201` (no `2202`), polling rate `8060` (no `8061`),
  lighting `8071`, onboard `8100`. Supported after the report: battery level from voltage (Solaar's curve),
  showing the current DPI and polling rate (read-only).
  - Receiver `C539` answers a ping at `0xFF` as HID++ 1.0; empty pairing info slots return error `0x03`.
  - A stream of events `11 01 07 10 00…`: feature `8071` (index 7), function 1; meaning not studied (on the
    PRO X TKL the `8071` event is user activity). There is a `HID VHF Driver` with the mouse's wPID (Dynamic
    Lighting).
- **G815** over the cable: **not found**. Its HID++ is on usage page `FF43` (collections `0602` long, `0604` very
  long, no short), and Hubless 0.1.0 only looked at `FF00`. After the report `FF43` is supported (per Solaar); a new
  report is needed to see the feature table.

### 2026-10-20 — the same set on 0.1.1 ([reports/2026-10-20-g903-g815-v0.1.1.txt](reports/2026-10-20-g903-g815-v0.1.1.txt))

- **G815** over the cable is now found through `FF43`: HID++ 4.2, U1 31.02.B0018. Features: `8040` brightness,
  `8060` polling rate, `8071` effects, `8081` per-key v2, `8100` onboard, `4522` KEYBOARD_DISABLE_BY_USAGE (Game
  Mode by HID usage, simpler than `4523` on the PRO X TKL), `4540` layout, `8010`/`8020`/`8030` G keys, M keys,
  MR. No battery.
  - Bug: the card was always "Inactive", because the link state came from reading the battery, and a wired
    keyboard has none. Fixed: without a battery feature, the link is checked with a ping.
- **G903**: receiver `C539` reports the name `G903 LS` (register `B5`/`40`), used as the name of the sleeping mouse.

### 2026-10-20 — the same set on 0.1.2 ([reports/2026-10-20-g903-g815-v0.1.2.txt](reports/2026-10-20-g903-g815-v0.1.2.txt))

- **G815:** the lighting effect only changed the logo. Hubless sent the effect to zone 0 only, and on the G815
  that zone is, judging by the photo, the logo (the keys are another zone). Fixed: the effect goes to every zone
  that supports it (in the CLI too).
- **G903:** according to the user, changing DPI with the mouse button now shows in Hubless, so the mouse sends an
  event from one of the features `8100`/`2201`/`8060`. Writing DPI and polling rate is enabled without a switch
  (the user's request).
- **G903 on 0.1.3:** after changing DPI in Hubless, the mouse's DPI button stops working until the mouse is
  restarted. Cause: Hubless put the mouse into host mode (`8100` setMode), and in that mode the onboard profile that
  handles the button is turned off. Fixed: mice with old features (`2201`/`8060`) are never put into host mode; DPI
  and polling rate are written in onboard mode, a leftover host mode is cleared, and the host/onboard switch is
  hidden in the card. Whether the G903 accepts DPI writes in onboard mode is still to be checked.
