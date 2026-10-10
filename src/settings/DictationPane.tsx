import { useEffect, useState } from "react";
import type { PaneProps } from "./SettingsApp";
import { api } from "../api";
import { ENGINE_LABELS, type EngineId, type Language, type PasteMode } from "../api";
import { useT } from "../i18n";

function AutostartToggle() {
  const { t } = useT();
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
        {t("dictation.autostart")}
      </label>
      {error && <div className="error">{error}</div>}
    </>
  );
}

export default function DictationPane({ settings, update }: PaneProps) {
  const { t } = useT();
  return (
    <section>
      <h1>{t("dictation.title")}</h1>
      <div className="row">
        <label>{t("common.model")}</label>
        <select value={settings.engine} onChange={(e) => update({ engine: e.target.value as EngineId })}>
          {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
            <option key={id} value={id}>
              {ENGINE_LABELS[id]}
            </option>
          ))}
        </select>
        <p className="hint">{t("dictation.model_hint")}</p>
      </div>
      <div className="row">
        <label>{t("dictation.language")}</label>
        <select value={settings.language} onChange={(e) => update({ language: e.target.value as Language })}>
          <option value="pl">{t("language_option.pl")}</option>
          <option value="en">{t("language_option.en")}</option>
          <option value="auto">{t("language_option.auto")}</option>
        </select>
        <p className="hint">{t("dictation.language_hint")}</p>
      </div>
      <div className="row">
        <label>{t("dictation.paste_mode")}</label>
        <select value={settings.paste_mode} onChange={(e) => update({ paste_mode: e.target.value as PasteMode })}>
          <option value="auto">{t("dictation.paste_mode.auto")}</option>
          <option value="always">{t("dictation.paste_mode.always")}</option>
          <option value="clipboard_only">{t("dictation.paste_mode.clipboard_only")}</option>
        </select>
        <p className="hint">{t("dictation.paste_mode_hint")}</p>
      </div>
      <div className="row check">
        <label>
          <input type="checkbox" checked={settings.hud_enabled} onChange={(e) => update({ hud_enabled: e.target.checked })} />
          {t("dictation.hud")}
        </label>
        <label>
          <input type="checkbox" checked={settings.dictation_history} onChange={(e) => update({ dictation_history: e.target.checked })} />
          {t("dictation.history")}
        </label>
        <AutostartToggle />
      </div>
      <h2>{t("dictation.commands")}</h2>
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
