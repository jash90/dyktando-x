import type { PaneProps } from "./SettingsApp";
import { ENGINE_LABELS, type EngineId, type Language, type PasteMode } from "../api";

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
        <label>Język</label>
        <select value={settings.language} onChange={(e) => update({ language: e.target.value as Language })}>
          <option value="pl">Polski</option>
          <option value="en">Angielski</option>
          <option value="auto">Automatycznie</option>
        </select>
        <p className="hint">Parakeet zawsze rozpoznaje język sam; wybór dotyczy Whispera.</p>
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
