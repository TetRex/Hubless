# Hubless
Hubless open-source alternative of Logitech G Hub

Лёгкая трей-замена G Hub для Windows: один exe без установщика, в простое ~14 МБ памяти и ~0% CPU.
Проверено с мышью PRO X Superlight 2, клавиатурой PRO X TKL Lightspeed и гарнитурой PRO X 2 LIGHTSPEED.

В разработке — см. [Plan.md](Plan.md). Находки по протоколам — в [docs/devices](docs/devices/README.md).

## Что умеет

- **Трей:** заряд всех устройств в подсказке, точка на иконке и уведомление при заряде ниже 15%,
  меню «Автозапуск», «Открыть папку логов», «Выход».
- **Панель** (левый клик по иконке):
  - мышь — DPI 400/800/1600/3200, режим onboard/host, lift-off;
  - клавиатура — яркость, эффект подсветки (статичный, дыхание, цикл, волна, рябь) и цвет;
  - гарнитура — громкость своего голоса (sidetone) и автоотключение.
- **Подсветка клавиатуры как в Solaar:** пока выбран эффект, Hubless управляет подсветкой сам и гасит её
  при простое; при выходе управление возвращается клавиатуре.
- **Onboard и host:** в режиме onboard настройки мыши хранятся в её памяти и работают без Hubless;
  в режиме host Hubless применяет их при каждом подключении.

Не поддерживаются: частота опроса (запись пока не работает), Game Mode, per-key подсветка, эквалайзер,
Blue VO!CE и DTS — это обработка звука в самом G Hub, а не команды устройству. Вместо эквалайзера
можно использовать [Equalizer APO](https://sourceforge.net/projects/equalizerapo/).

Перед запуском закройте G Hub (`lghub`, `lghub_agent`, `lghub_updater`): обе программы одновременно
управлять устройствами не могут.

## Командная строка

```
hubless                       запуск в трее
hubless --dump [файл]         устройства и таблицы фич HID++ (JSON и текст)
hubless --quit                завершить запущенный Hubless (как «Выход» в меню)
hubless mouse ...             DPI, lift-off, режим onboard/host
hubless keyboard ...          яркость, подсветка и эффекты
hubless headset ...           sidetone, автоотключение
hubless --help                полный список команд
```

Записи в устройство принимают `--dry-run`: показать пакеты, ничего не отправляя.

## Сборка

Нужны Rust (stable, `x86_64-pc-windows-msvc`) и Visual Studio Build Tools с C++.

```
cargo build --release
```

Результат — `target/release/hubless.exe`.

## Файлы

- `%APPDATA%\Hubless\config.toml` — настройки приложения (уровень логов, трассировка HID).
- `%APPDATA%\Hubless\devices.toml` — настройки устройств, их пишет сам Hubless.
- `%APPDATA%\Hubless\backups\` — резервные копии памяти мыши.
- `%LOCALAPPDATA%\Hubless\logs\` — логи; подробные — с переменной окружения `HUBLESS_LOG=debug`.

## Лицензия

Copyright (C) 2026 TetRex

Эта программа — свободное ПО: вы можете распространять и/или изменять её на условиях
GNU General Public License версии 3 либо (по вашему выбору) любой более поздней версии,
опубликованной Free Software Foundation. Программа распространяется БЕЗ КАКИХ-ЛИБО ГАРАНТИЙ.
Полный текст лицензии — в файле [LICENSE](LICENSE).

SPDX-License-Identifier: `GPL-3.0-or-later`
