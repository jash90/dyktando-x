import type { PaneProps } from "./SettingsApp";
import { ENGINE_LABELS, meetingsApi, TRANSLATION_TARGETS, type EngineId, type Language } from "../api";
import ShortcutRecorder from "../components/ShortcutRecorder";

export default function MeetingsPane({ settings, update, env }: PaneProps) {
  const mac = env.os === "macos";
  return (
    <section>
      <h1>Spotkania</h1>
      <p className="hint">
        Nagrywa dwie ścieżki: Twój mikrofon i dźwięk aplikacji (rozmówcy w Meet, Zoom, Teams). Audio i transkrypcja zostają na komputerze;
        do dostawcy AI trafia tylko transkrypt — i tylko gdy poprosisz o podsumowanie.
      </p>
      <div className="row">
        <button className="primary" onClick={() => meetingsApi.open()}>
          Otwórz okno spotkań
        </button>
      </div>
      <div className="row">
        <label>Skrót: nagraj / zatrzymaj</label>
        <ShortcutRecorder value={settings.shortcut_meeting} onChange={(v) => update({ shortcut_meeting: v })} mac={mac} />
      </div>
      <div className="row">
        <label>Model do spotkań</label>
        <select value={settings.meeting_engine} onChange={(e) => update({ meeting_engine: e.target.value as EngineId })}>
          {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
            <option key={id} value={id}>
              {ENGINE_LABELS[id]}
            </option>
          ))}
        </select>
        <p className="hint">
          Godzina nagrania: Parakeet ok. 5 min, Whisper turbo ok. 12 min (Mac z Apple Silicon; na innych komputerach dłużej). Whisper turbo
          robi w polskich rozmowach wyraźnie mniej błędów (ok. 8% słów zamiast 12%) i nie wtrąca angielskich słów — warto go wybrać, jeśli
          czas nie gra roli.
        </p>
      </div>
      <div className="row">
        <label>Język spotkań</label>
        <select value={settings.meeting_language} onChange={(e) => update({ meeting_language: e.target.value as Language })}>
          <option value="pl">Polski</option>
          <option value="en">Angielski</option>
          <option value="auto">Automatycznie</option>
        </select>
        <p className="hint">
          Niezależny od języka dyktowania. Dotyczy Whispera i Canary — przepisywania po nagraniu, na żywo i importowanych plików; Parakeet zawsze
          rozpoznaje język sam. Przy rozmowach w jednym języku lepiej wybrać go wprost niż „Automatycznie”.
        </p>
      </div>
      <div className="row check">
        <label>
          <input
            type="checkbox"
            checked={settings.meeting_live_transcription}
            onChange={(e) => update({ meeting_live_transcription: e.target.checked })}
          />
          Przepisuj na żywo w trakcie nagrania (tekst roboczy co ok. 1,5 s; najszybciej z Parakeetem)
        </label>
        <label>
          <input type="checkbox" checked={settings.meeting_live_window} onChange={(e) => update({ meeting_live_window: e.target.checked })} />
          Pokazuj w trakcie nagrania małe okno na wierzchu z licznikiem, poziomami dźwięku i tekstem na żywo
        </label>
      </div>
      <div className="row">
        <label>Tłumacz na żywo na</label>
        <select
          value={settings.meeting_live_translate_to}
          disabled={!settings.meeting_live_transcription}
          onChange={(e) => update({ meeting_live_translate_to: e.target.value })}
        >
          {TRANSLATION_TARGETS.map(([code, name]) => (
            <option key={code} value={code}>
              {name}
            </option>
          ))}
        </select>
        <p className="hint">
          Tłumaczy Canary 1B v2 (musi być pobrany) — między angielskim a pozostałymi językami, np. polski → angielski albo angielski → polski.
          Język źródłowy to „Język spotkań” powyżej. Każda wypowiedź jest dekodowana dwa razy, więc tekst pojawia się później
          niż bez tłumaczenia.
        </p>
      </div>
      <div className="row check">
        <label>
          <input type="checkbox" checked={settings.meeting_diarization} onChange={(e) => update({ meeting_diarization: e.target.checked })} />
          Rozpoznawaj rozmówców („Rozmówca 1”, „Rozmówca 2”…)
        </label>
        <label>
          <input type="checkbox" checked={settings.meeting_auto_transcribe} onChange={(e) => update({ meeting_auto_transcribe: e.target.checked })} />
          Przepisuj automatycznie po zakończeniu nagrania
        </label>
        <label>
          <input
            type="checkbox"
            checked={settings.meeting_auto_summarize}
            disabled={!settings.meeting_auto_transcribe}
            onChange={(e) => update({ meeting_auto_summarize: e.target.checked })}
          />
          Potem podsumowuj domyślnym dostawcą AI (zakładka AI)
        </label>
        <label>
          <input type="checkbox" checked={settings.meeting_consent_reminder} onChange={(e) => update({ meeting_consent_reminder: e.target.checked })} />
          Przypominaj przy starcie nagrania o poinformowaniu rozmówców
        </label>
      </div>
      <div className="row">
        <label>Usuwaj nagrania audio po</label>
        <select
          value={settings.meeting_audio_retention_days}
          onChange={(e) => update({ meeting_audio_retention_days: Number(e.target.value) })}
        >
          {[7, 14, 30, 60, 90, 0].map((d) => (
            <option key={d} value={d}>
              {d === 0 ? "nigdy" : `${d} dniach`}
            </option>
          ))}
        </select>
        <p className="hint">Dotyczy tylko spotkań już przepisanych — transkrypt i podsumowania zostają.</p>
      </div>
      {env.os === "macos" && (
        <p className="hint">
          macOS 14.4+: przy pierwszym nagraniu system zapyta o „Nagrywanie dźwięku systemowego”. Jeśli rozmówców nie słychać w nagraniu, włącz
          Dyktando X w Ustawieniach systemowych → Prywatność → Nagrywanie ekranu i dźwięku systemowego.
        </p>
      )}
      {env.os === "windows" && <p className="hint">Dźwięk aplikacji wymaga Windows 11 albo Windows 10 (kompilacja 20348+).</p>}
      {env.os === "linux" && (
        <p className="hint">Dźwięk aplikacji nagrywany jest z monitora domyślnego wyjścia (PulseAudio/PipeWire, program parec z pakietu pulseaudio-utils).</p>
      )}
    </section>
  );
}
