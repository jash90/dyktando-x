# Dyktando X

[English](README.md) | Polski

Dyktowanie po polsku i nagrywanie spotkań na **macOS, Windows i Linuksa**. Rozpoznawanie mowy działa
lokalnie, a nagrania nigdy nie opuszczają komputera. Do dostawcy AI trafia wyłącznie transkrypcja,
i tylko wtedy, gdy poprosisz o podsumowanie.

Wieloplatformowa wersja [Dyktanda na macOS](https://github.com/jash90/dyktando-mac) (Swift).
Tauri 2 + Rust + React/TypeScript.

## Funkcje

- **Dyktowanie:** przytrzymaj skrót (domyślnie F5) albo sam prawy klawisz ⌘/Ctrl, powiedz zdanie,
  a tekst zostanie wklejony w aktywne pole. Polskie komendy głosowe („kropka”, „przecinek”,
  „nowy akapit”…), wielkie litery, przywracanie schowka. Gdy fokus nie jest w polu tekstowym,
  tekst trafia tylko do schowka.
- **Modele** (pobierane w aplikacji):

  | Model | Silnik | Rozmiar |
  |---|---|---|
  | Parakeet TDT 0.6B v3 (domyślny) | ONNX | 670 MB |
  | Canary 1B v2 | ONNX | 1 GB |
  | Whisper large-v3-turbo | whisper.cpp | 1,6 GB |
  | Whisper large-v3 (q5_0) | whisper.cpp | 1,1 GB |

- **Spotkania:** dwie ścieżki — Twój mikrofon i dźwięk aplikacji (pozostali uczestnicy w Meet,
  Zoomie, Teams). Transkrypcja na żywo podczas nagrywania, widoczna w małym oknie zawsze na wierzchu
  z licznikiem czasu i poziomami sygnału obu ścieżek, opcjonalnie tłumaczona przez Canary (między
  angielskim a pozostałymi obsługiwanymi językami). Po nagraniu pełna transkrypcja z podpisami mówców
  („Ja”, „Rozmówca 1”, „Rozmówca 2”…). Gdy aplikacja do wideokonferencji zaczyna używać mikrofonu,
  pojawia się pytanie „Wykryto spotkanie — nagrać?”.
  Każdą ścieżkę można pobrać jako osobny plik WAV („Mój głos” dla mikrofonu, „Rozmówcy” dla
  pozostałych uczestników) albo obie zmiksowane w jeden („Całe nagranie”).
  Jeśli transkrypcja wyszła słabo, przetranskrybuj spotkanie z ustawionym językiem (lub językami):
  jednym albo kilkoma dla rozmowy mieszanej — wtedy Whisper dla każdej wypowiedzi wybiera
  najbardziej prawdopodobny z wybranych języków. Whisper zna ~100 języków, Parakeet i Canary
  25 europejskich (Parakeet zawsze sam rozpoznaje język, Canary potrzebuje dokładnie jednego).
- **Bezpieczeństwo nagrania:** jeśli mikrofon albo dźwięk aplikacji przestanie dostarczać dźwięk
  w trakcie spotkania (zmiana urządzenia, słuchawki Bluetooth, błąd strumienia), źródło jest
  odtwarzane w ciągu kilku sekund; nieuniknione przerwy są wypisane w spotkaniu. Logi są zapisywane
  w `~/Library/Logs/Dyktando X` (macOS) lub `<katalog danych>/logs`.
- **Historia dyktowania:** każde dyktowanie (tekst i nagranie) jest przechowywane lokalnie
  w zakładce „Dyktowania” okna spotkań, gdzie można je skopiować, pobrać albo przetranskrybować
  innym modelem lub w innym języku.
- **Słownik:** nazwy i terminy, które Whisper ma pisać poprawnie (ustawienia → Spotkania).
- **Importowane nagrania:** „Importuj nagranie” zamienia plik audio z rozmową (MP3, M4A/AAC, WAV,
  FLAC, OGG Vorbis/Opus, AIFF, CAF) w spotkanie: plik jest dekodowany lokalnie, transkrybowany
  z podpisami mówców („Rozmówca 1”, „Rozmówca 2”…) i, jeśli włączone, podsumowywany.
- **Podsumowania AI:** Anthropic, OpenAI, OpenRouter, Z.AI. Klucze API są przechowywane
  w systemowym magazynie poświadczeń, a długie spotkania są podsumowywane metodą map-reduce.

## Zrzuty ekranu

| Podsumowanie spotkania | Transkrypcja |
|---|---|
| ![Spotkanie z podsumowaniem AI](docs/screenshots/meeting-summary.png) | ![Transkrypcja z mówcami](docs/screenshots/meeting-transcript.png) |
| **Historia dyktowania (ciemny motyw)** | **Ustawienia** |
| ![Historia dyktowania w ciemnym motywie](docs/screenshots/dictations-dark.png) | ![Ustawienia dyktowania](docs/screenshots/settings.png) |

## Wymagania systemowe

| | Dyktowanie | Dźwięk aplikacji w spotkaniach |
|---|---|---|
| macOS | 13+, uprawnienia Mikrofon i Dostępność | 14.4+ (Core Audio process tap), zgoda „Nagrywanie dźwięku systemowego” |
| Windows | 10/11 | Windows 11 lub Windows 10 build 20348+ (WASAPI process loopback) |
| Linux (glibc 2.38+: Ubuntu 24.04, Debian 13, Fedora 39 i nowsze) | X11 i Wayland; skróty przez `/dev/input` (reguła udev w paczce .deb/.rpm) | PulseAudio/PipeWire, narzędzie `parec` (pulseaudio-utils) |

Na Waylandzie wklejanie próbuje kolejno `wtype` (Sway, Hyprland), `dotool` i `ydotool`. Bez nich
tekst zostaje w schowku.

## Budowanie

```bash
npm ci
npm run tauri dev                          # tryb deweloperski
npm run tauri build                        # paczki dla bieżącego systemu
cd src-tauri && cargo test --lib           # testy jednostkowe
cargo test --lib -- --ignored synthetic_meeting   # test spotkania (pobiera modele)
```

Linux wymaga: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libasound2-dev
libdbus-1-dev libxdo-dev libclang-dev cmake`.

macOS: aplikacja musi być podpisana z uprawnieniem `com.apple.security.device.audio-input`
(`src-tauri/Entitlements.plist`). Bez niego, przy hardened runtime, mikrofon zwraca ciszę.

## Wydawanie

Wydania są budowane, podpisywane i publikowane lokalnie przez `scripts/release.sh`;
`scripts/ci.sh` uruchamia testy CI. Szczegóły w [RELEASING.md](RELEASING.md) (po angielsku).

## Dane

`<katalog danych>/DyktandoX/` zawiera `models/`, `settings.json` oraz `Meetings/<data>/`, każde
z `meeting.json`, `audio/*.wav`, `transcript.md|json` i `summaries/`. Dźwięk przetranskrybowanych
spotkań jest usuwany po 30 dniach (do zmiany w ustawieniach); tekst zostaje.
