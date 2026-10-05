import { useEffect, useState } from "react";
import { Check, KeyRound, Plug } from "lucide-react";
import type { PaneProps } from "./SettingsApp";
import { aiApi, type ProviderId, type ProviderInfo } from "../api";

export default function AiPane({ settings, update, env }: PaneProps) {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [msg, setMsg] = useState<Record<string, { ok: boolean; text: string }>>({});
  const [defaultPrompt, setDefaultPrompt] = useState("");
  const [importMsg, setImportMsg] = useState<string | null>(null);

  const importLegacy = async () => {
    const names = await aiApi.importLegacy();
    setImportMsg(names.length ? `Zaimportowano klucze: ${names.join(", ")}` : "Nie znaleziono nowych kluczy w Dyktando dla macOS");
    refresh();
  };

  const refresh = () => aiApi.providers().then(setProviders);
  useEffect(() => {
    refresh();
    aiApi.defaultPrompt().then(setDefaultPrompt);
  }, []);

  const cfg = (id: ProviderId) => settings.ai_providers[id] ?? { model: "", base_url: "" };
  const setCfg = (id: ProviderId, patch: Partial<{ model: string; base_url: string }>) =>
    update({ ai_providers: { ...settings.ai_providers, [id]: { ...cfg(id), ...patch } } });

  const saveKey = async (id: ProviderId) => {
    try {
      await aiApi.setKey(id, keys[id] ?? "");
      setKeys((k) => ({ ...k, [id]: "" }));
      setMsg((m) => ({ ...m, [id]: { ok: true, text: (keys[id] ?? "").trim() ? "Klucz zapisany w systemowym magazynie haseł" : "Klucz usunięty" } }));
      refresh();
    } catch (e) {
      setMsg((m) => ({ ...m, [id]: { ok: false, text: String(e) } }));
    }
  };

  const test = async (id: ProviderId) => {
    setMsg((m) => ({ ...m, [id]: { ok: true, text: "Łączenie…" } }));
    try {
      const list = await aiApi.test(id);
      setModels((x) => ({ ...x, [id]: list }));
      setMsg((m) => ({ ...m, [id]: { ok: true, text: `Połączono — ${list.length} modeli` } }));
    } catch (e) {
      setMsg((m) => ({ ...m, [id]: { ok: false, text: String(e) } }));
    }
  };

  return (
    <section>
      <h1>AI — podsumowania spotkań</h1>
      <p className="hint">
        Podsumowanie wysyła transkrypt (sam tekst, nie nagranie) do wybranego dostawcy. Klucze są przechowywane w systemowym magazynie haseł
        (pęk kluczy macOS, Menedżer poświadczeń Windows, Secret Service w Linuksie) i nigdy nie trafiają do plików ustawień.
      </p>
      {env.os === "macos" && providers.some((p) => !p.has_key) && (
        <div className="row">
          <button onClick={importLegacy}>
            <KeyRound size={14} /> Importuj klucze z Dyktando dla macOS
          </button>
          <p className="hint">{importMsg ?? "Jednorazowo skopiuje klucze zapisane w poprzedniej aplikacji. macOS zapyta o zgodę na dostęp do pęku kluczy."}</p>
        </div>
      )}
      <div className="row">
        <label>Domyślny dostawca</label>
        <select value={settings.ai_provider} onChange={(e) => update({ ai_provider: e.target.value as ProviderId })}>
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
              {p.has_key ? "" : " (brak klucza)"}
            </option>
          ))}
        </select>
      </div>
      {providers.map((p) => (
        <div key={p.id} className="card">
          <div className="card-head">
            <strong>{p.name}</strong>
            {p.has_key ? (
              <span className="ok">
                <Check size={14} /> klucz zapisany
              </span>
            ) : (
              <span className="hint">brak klucza</span>
            )}
          </div>
          <div className="grid">
            <label>Klucz API</label>
            <span className="inline">
              <input
                type="password"
                autoComplete="off"
                placeholder={p.has_key ? "•••••••• (wpisz nowy, aby zmienić; pusty + Zapisz usuwa)" : "wklej klucz"}
                value={keys[p.id] ?? ""}
                onChange={(e) => setKeys((k) => ({ ...k, [p.id]: e.target.value }))}
              />
              <button onClick={() => saveKey(p.id)}>
                <KeyRound size={14} /> Zapisz
              </button>
            </span>
            <label>Model</label>
            <span className="inline">
              {models[p.id]?.length ? (
                <select value={cfg(p.id).model || p.default_model} onChange={(e) => setCfg(p.id, { model: e.target.value })}>
                  {!(cfg(p.id).model || p.default_model) && <option value="">— wybierz —</option>}
                  {[...new Set([cfg(p.id).model || p.default_model, ...models[p.id]].filter(Boolean))].map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              ) : (
                <input value={cfg(p.id).model} placeholder={p.model_placeholder} onChange={(e) => setCfg(p.id, { model: e.target.value })} />
              )}
              <button disabled={!p.has_key} onClick={() => test(p.id)}>
                <Plug size={14} /> Testuj połączenie
              </button>
            </span>
            <label>Adres API</label>
            <input value={cfg(p.id).base_url} placeholder={p.default_base_url} onChange={(e) => setCfg(p.id, { base_url: e.target.value })} />
          </div>
          {msg[p.id] && <div className={msg[p.id].ok ? "hint" : "error"}>{msg[p.id].text}</div>}
        </div>
      ))}
      <h2>Prompt podsumowania</h2>
      <textarea
        rows={10}
        value={settings.ai_prompt || defaultPrompt}
        onChange={(e) => update({ ai_prompt: e.target.value === defaultPrompt ? "" : e.target.value })}
      />
      <button className="link" disabled={!settings.ai_prompt} onClick={() => update({ ai_prompt: "" })}>
        Przywróć domyślny
      </button>
    </section>
  );
}
