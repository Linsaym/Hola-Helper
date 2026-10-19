# Hola Helper 🎙️

Локальный голосовой ассистент для Windows на Rust. Распознаёт фразу **«Hey Jarvis»** через [openWakeWord](https://github.com/dscripka/openWakeWord), отвечает заранее записанной голосовой фразой и слушает команду на русском через [whisper-rs / whisper.cpp](https://docs.rs/whisper-rs/0.16.0/whisper_rs/).

**Работает офлайн:** для распознавания и воспроизведения интернет не нужен.

## Быстрый старт

Из корня проекта:

```powershell
cargo run --release -- --mic
```

Сначала установи модели и инструменты сборки, описанные ниже. Скажи **«Hey Jarvis»**, дождись окончания ответа из `assets/voice/wake/` и скажи **«Запусти Steam»**. Ассистент откроет существующий ярлык `actions/steam.lnk` через Windows Shell. В Windows регистр имени файла не имеет значения: `Steam.lnk` тоже подходит.

Ожидание начала речи — до 5 секунд, максимальная запись — 8 секунд. Пауза в 0,8 секунды завершает команду. Простой порог громкости отсекает тишину и короткие щелчки; слишком тихая речь может быть пропущена. После выполнения, неизвестной команды, отсутствия речи или ошибки STT ассистент снова ждёт wake word.

Для остановки нажми **Ctrl+C**.

> Порог срабатывания: **0.25**. Чем он ниже, тем выше чувствительность, но тем больше риск ложных срабатываний.

## Голосовые ответы

Помести свои записи в папку:

```text
assets/
└── voice/
    └── wake/
        ├── listening.wav
        ├── command.wav
        └── whats-up.wav
```

После обнаружения ключевой фразы приложение случайным образом выбирает один из WAV-файлов **непосредственно в этой папке**. Подойдут обычные PCM WAV, воспроизводимые Windows.

Пока звучит ответ, входящий звук отбрасывается. После воспроизведения накопленное аудио очищается, и начинается запись команды. Это предотвращает распознавание собственного ответа ассистента.

Если записей нет или воспроизведение не удалось, программа выводит предупреждение `[voice]` и продолжает работу.

## Проверка на WAV-файле

```powershell
cargo run --release -- samples/heyjarvis.wav
```

Тестовый входной файл должен быть **WAV PCM 16-bit, mono, 16 kHz**. Этот режим проверяет только wake word и голосовой ответ, выводит факт обнаружения и максимальный `score`. STT и действия доступны в режиме `--mic`.

## Установка моделей

Для распознавания нужны три ONNX-файла в `models/openwakeword/`. Если их ещё нет, выполни в PowerShell из корня проекта:

```powershell
New-Item -ItemType Directory -Force models/openwakeword | Out-Null
$release = 'https://github.com/dscripka/openWakeWord/releases/download/v0.5.1'
foreach ($name in @('melspectrogram.onnx', 'embedding_model.onnx', 'hey_jarvis_v0.1.onnx')) {
    Invoke-WebRequest "$release/$name" -OutFile "models/openwakeword/$name"
}
```

Для команд нужна **мультиязычная Base** (не `base.en`) в `models/whisper/ggml-base.bin`, около 148 MB:

```powershell
New-Item -ItemType Directory -Force models/whisper | Out-Null
curl.exe --fail --location 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin' --output models/whisper/ggml-base.bin
```

Модели не хранятся в Git. Приложение не скачивает их автоматически и не обращается к сети. Whisper загружается один раз перед запуском микрофона, остаётся в памяти и использует CPU с максимум четырьмя потоками. Язык задан явно: `ru`; перевод и перенос текста предыдущей команды отключены. Python для запуска не требуется.

## Сборка в Windows

Для `whisper.cpp` нужны Visual Studio C++ Build Tools, CMake и `libclang` для генерации bindings: [инструкция whisper-rs](https://github.com/tazz4843/whisper-rs/blob/master/BUILDING.md). Если CMake и LLVM установлены отдельно, добавь CMake в `PATH` и укажи каталог с `libclang.dll` в `LIBCLANG_PATH`.

В текущем окружении можно использовать CMake из Visual Studio и `libclang` из существующего Python venv. Выполни в PowerShell из корня проекта перед сборкой:

```powershell
.venv/Scripts/python.exe -m pip install libclang
$vswhere = "${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
$visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$env:CMAKE = Join-Path $visualStudio 'Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe'
$env:LIBCLANG_PATH = (Resolve-Path '.venv/Lib/site-packages/clang/native').Path
cargo run --release -- --mic
```

Эти переменные действуют в текущем окне PowerShell. Если `.venv` ещё нет, создай его командой `python -m venv .venv`. Не задавай `WHISPER_DONT_GENERATE_BINDINGS`: готовые bindings версии 0.16.0 вызывают ошибки размеров структур на Windows. Инструменты сборки не нужны при запуске готового `.exe`.

## Как это устроено

```text
Микрофон → обработка аудио → openWakeWord → «Hey Jarvis»
                                              ↓
                                  Случайный WAV-ответ
                                              ↓
                                  Запись команды (mono, 16 kHz)
                                              ↓
                                  Whisper Base, ru → текст
                                              ↓
                                  Нормализация → точный match
                                              ↓
                                  actions/steam.lnk → ожидание wake word
```

- **CPAL** получает звук с микрофона Windows.
- Аудио преобразуется в mono, 16 kHz; распознавание выполняется отдельно от микрофонного callback.
- **ONNX Runtime** запускает модели openWakeWord на CPU.
- **Windows MCI** воспроизводит заранее записанные ответы.
- **Whisper** выполняет STT в отдельном потоке только после активации и записи команды. Во время STT аудиоочередь продолжает опустошаться, повторная активация отключена.
- `command_audio` ограничивает запись по длительности и паузам; `speech` отвечает за STT; `commands` сопоставляет текст; `actions` выполняет действие.

Поддерживаются точные нормализованные строки `запусти steam` и `запусти стим` (явный русский вариант названия). Регистр, пробелы и пунктуация нормализуются; нечёткого поиска и поиска подстрок нет. Неизвестный текст, тишина и сегменты с высокой вероятностью отсутствия речи ничего не запускают. Распознанный текст никогда не передаётся в командную оболочку: действие выбирается из фиксированного списка.

## Проверка проекта

```powershell
cargo fmt --check
cargo check
cargo test
```

Дополнительные тесты с моделями и примером записи:

```powershell
cargo test wake_word::tests -- --ignored
cargo test microphone::tests::recorded_audio -- --ignored
cargo test speech::tests::silence_does_not -- --ignored
```

Проверка STT на собственной записи «Запусти Steam» (**WAV PCM16, mono, 16 kHz**), затем на тишине с той же загруженной моделью:

```powershell
$env:HOLA_TEST_COMMAND_WAV = 'C:/path/to/steam-command.wav'
cargo test speech::tests::recognizes_recorded_steam_command_then_silence -- --ignored --nocapture
```

Отдельная проверка запуска существующего ярлыка **действительно открывает Steam**:

```powershell
cargo test actions::tests::bundled_steam_shortcut_launches -- --ignored
```

При проблемах с микрофоном проверь доступ Windows к микрофону и используй **release-сборку**. Предупреждение `Xrun` означает возможный пропуск аудиоданных; приложение пытается продолжить захват. При смене или отключении устройства может потребоваться перезапуск.

## Лицензии моделей

Код [openWakeWord](https://github.com/dscripka/openWakeWord) распространяется по **Apache-2.0**, а его предобученные модели — по **CC BY-NC-SA 4.0**. У моделей есть ограничения на коммерческое использование; учитывай это перед распространением проекта.
