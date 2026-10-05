# Dyktando X

Dyktowanie po polsku i nagrywanie spotkań na **macOS, Windows i Linuksie**. Rozpoznawanie mowy
działa lokalnie, nagrania nie opuszczają komputera. Do dostawcy AI trafia tylko transkrypt i tylko
wtedy, gdy poprosisz o podsumowanie.

Wieloplatformowa wersja [Dyktando dla macOS](https://github.com/jash90/dyktando-mac) (Swift).
Tauri 2 + Rust + React/TypeScript.

## Co umie

- **Dyktowanie:** przytrzymaj skrót (domyślnie F5) albo sam prawy ⌘/Ctrl, powiedz zdanie, a tekst
  wklei się w aktywne pole. Polskie polecenia („kropka”, „przecinek”, „nowy akapit”…), wielkie
  litery, przywracanie schowka. Gdy fokus nie jest w polu tekstowym, tekst trafia tylko do schowka.
- **Modele** (pobierane w aplikacji):

  | Model | Silnik | Rozmiar |
  |---|---|---|
  | Parakeet TDT 0.6B v3 (domyślny) | ONNX | 670 MB |
  | Canary 1B v2 | ONNX | 1 GB |
  | Whisper large-v3-turbo | whisper.cpp | 1,6 GB |
  | Whisper large-v3 (q5_0) | whisper.cpp | 1,1 GB |

- **Spotkania:** dwie ścieżki, czyli Twój mikrofon i dźwięk aplikacji (rozmówcy w Meet, Zoom, Teams).
  Po nagraniu transkrypcja z podziałem na mówców („Ja”, „Rozmówca 1”, „Rozmówca 2”…). Podpowiedź
  „Wykryto spotkanie — nagrać?” pojawia się, gdy komunikator używa mikrofonu.
- **Podsumowania AI:** Anthropic, OpenAI, OpenRouter, Z.AI. Klucze są w systemowym magazynie haseł,
  a długie spotkania podsumowywane map-reduce.

## Wymagania systemowe

| | Dyktowanie | Dźwięk aplikacji na spotkaniach |
|---|---|---|
| macOS | 13+, uprawnienia Mikrofon i Dostępność | 14.4+ (Core Audio process tap), zgoda „Nagrywanie dźwięku systemowego” |
| Windows | 10/11 | Windows 11 albo Windows 10 kompilacja 20348+ (WASAPI process loopback) |
| Linux (glibc 2.38+: Ubuntu 24.04, Debian 13, Fedora 39 i nowsze) | X11 i Wayland; skróty przez `/dev/input` (reguła udev w paczce .deb/.rpm) | PulseAudio/PipeWire, program `parec` (pulseaudio-utils) |

Na Waylandzie wklejanie używa po kolei `wtype` (Sway, Hyprland), `dotool` i `ydotool`. Bez nich
tekst zostaje w schowku.

## Budowanie

```bash
npm ci
npm run tauri dev                          # tryb deweloperski
npm run tauri build                        # paczki dla bieżącego systemu
cd src-tauri && cargo test --lib           # testy jednostkowe
cargo test --lib -- --ignored synthetic_meeting   # test spotkania (pobiera modele)
```

Linux potrzebuje: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libasound2-dev
libdbus-1-dev libxdo-dev libclang-dev cmake`.

macOS: aplikację trzeba podpisać z uprawnieniem `com.apple.security.device.audio-input`
(`src-tauri/Entitlements.plist`). Bez niego przy hardened runtime mikrofon oddaje samą ciszę.

## Dane

`<katalog danych>/DyktandoX/` zawiera `models/`, `settings.json` i `Meetings/<data>/`, a w nich
`meeting.json`, `audio/*.wav`, `transcript.md|json` i `summaries/`. Audio przepisanych spotkań jest
usuwane po 30 dniach (do zmiany w ustawieniach); tekst zostaje.
