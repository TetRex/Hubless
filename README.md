# Hubless
Hubless open-source alternative of Logitech G Hub

Лёгкая трей-замена G Hub для Windows: PRO X Superlight 2, PRO X TKL Lightspeed, PRO X Wireless.

В разработке — см. [Plan.md](Plan.md).

## Сборка

Нужны Rust (stable, `x86_64-pc-windows-msvc`) и Visual Studio Build Tools с C++.

```
cargo build --release
```

Результат — `target/release/hubless.exe`. Конфиг: `%APPDATA%\Hubless\config.toml`, логи: `%LOCALAPPDATA%\Hubless\logs`.

## Лицензия

Copyright (C) 2026 TetRex

Эта программа — свободное ПО: вы можете распространять и/или изменять её на условиях
GNU General Public License версии 3 либо (по вашему выбору) любой более поздней версии,
опубликованной Free Software Foundation. Программа распространяется БЕЗ КАКИХ-ЛИБО ГАРАНТИЙ.
Полный текст лицензии — в файле [LICENSE](LICENSE).

SPDX-License-Identifier: `GPL-3.0-or-later`
