import { useEffect, useState } from "react";
import type { PaneProps } from "./SettingsApp";
import { api } from "../api";
import { ENGINE_LABELS, type EngineId, type Language, type PasteMode } from "../api";

function AutostartToggle() {
  const [on, setOn] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.autostartEnabled().then(setOn);
  }, []);
  return (
    <>
      <label>
        <input
          type="checkbox"
          checked={!!on}
          disabled={on === null}
          onChange={(e) => {
            const v = e.target.checked;
            api
              .setAutostart(v)
              .then(() => setOn(v))
              .catch((err) => setError(String(err)));
          }}
        />
        Uruchamiaj Dyktando X po zalogowaniu
      </label>
      {error && <div className="error">{error}</div>}
    </>
  );
}

export default function DictationPane({ settings, update }: PaneProps) {
  return (
    <section>
      <h1>Dyktowanie</h1>
      <div className="row">
        <label>Model</label>
        <select value={settings.engine} onChange={(e) => update({ engine: e.target.value as EngineId })}>
          {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
            <option key={id} value={id}>
              {ENGINE_LABELS[id]}
            </option>
          ))}
        </select>
        <p className="hint">Model musi być pobrany w zakładce Modele. Parakeet jest najszybszy, Whisper sam stawia interpunkcję.</p>
      </div>
      <div className="row">
        <label>Język dyktowania</label>
        <select value={settings.language} onChange={(e) => update({ language: e.target.value as Language })}>
          <option value="pl">Polski</option>
          <option value="en">Angielski</option>
          <option value="auto">Automatycznie</option>
        </select>
        <p className="hint">
          Parakeet zawsze rozpoznaje język sam; wybór dotyczy Whispera i Canary (Canary przy „Automatycznie” zakłada polski). Język spotkań
          ustawisz osobno w zakładce Spotkania.
        </p>
      </div>
      <div className="row">
        <label>Wstawianie tekstu</label>
        <select value={settings.paste_mode} onChange={(e) => update({ paste_mode: e.target.value as PasteMode })}>
          <option value="auto">Wklejaj, gdy fokus jest w polu tekstowym</option>
          <option value="always">Zawsze wklejaj</option>
          <option value="clipboard_only">Tylko kopiuj do schowka</option>
        </select>
        <p className="hint">Po wklejeniu poprzednia zawartość schowka (tekst) wraca na miejsce.</p>
      </div>
      <div className="row check">
        <label>
          <input type="checkbox" checked={settings.hud_enabled} onChange={(e) => update({ hud_enabled: e.target.checked })} />
          Pokazuj dymek ze stanem nagrywania
        </label>
        <label>
          <input type="checkbox" checked={settings.dictation_history} onChange={(e) => update({ dictation_history: e.target.checked })} />
          Zapisuj historię dyktowania (tekst i nagranie, tylko na tym komputerze) — w oknie Spotkania → Dyktowania
        </label>
        <AutostartToggle />
      </div>
      <h2>Polecenia w trakcie dyktowania</h2>
      <table className="commands">
        <tbody>
          {[
            ["kropka", "."],
            ["przecinek", ","],
            ["znak zapytania", "?"],
            ["wykrzyknik", "!"],
            ["dwukropek", ":"],
            ["średnik", ";"],
            ["nowa linia", "↵"],
            ["nowy akapit", "↵↵"],
          ].map(([w, s]) => (
            <tr key={w}>
              <td>„{w}”</td>
              <td>
                <code>{s}</code>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
