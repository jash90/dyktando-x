import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Copy, Download, Mic, Trash2, Wand2 } from "lucide-react";
import { api, clock, dictationsApi, ENGINE_LABELS, type DictationEntry, type EngineId, type LanguageInfo, type Settings } from "../api";
import LanguagePicker, { languageCheck } from "./LanguagePicker";

const DAY = new Intl.DateTimeFormat("pl-PL", { weekday: "long", day: "numeric", month: "long", year: "numeric" });
const TIME = new Intl.DateTimeFormat("pl-PL", { hour: "2-digit", minute: "2-digit" });

/** „1 dyktowanie”, „3 dyktowania”, „5 dyktowań”, „22 dyktowania”. */
export function countLabel(n: number): string {
  const tens = n % 100;
  const ones = n % 10;
  if (n === 1) return "1 dyktowanie";
  if (ones >= 2 && ones <= 4 && (tens < 12 || tens > 14)) return `${n} dyktowania`;
  return `${n} dyktowań`;
}

/** Historia dyktowania: wpisy od najnowszych, pogrupowane po dniach, z ponownym przepisaniem. */
export default function DictationsView({ query, onError }: { query: string; onError: (e: string | null) => void }) {
  const [entries, setEntries] = useState<DictationEntry[]>([]);
  const [engine, setEngine] = useState<EngineId | null>(null);
  const [languages, setLanguages] = useState<string[]>([]);
  const [allLanguages, setAllLanguages] = useState<LanguageInfo[]>([]);
  /** Wpis, który właśnie jest przepisywany od nowa. */
  const [retranscribing, setRetranscribing] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const refresh = useCallback(() => dictationsApi.list().then(setEntries), []);

  useEffect(() => {
    refresh();
    api.getSettings().then((s: Settings) => {
      setEngine(s.engine);
      setLanguages(s.language === "auto" ? [] : [s.language]);
    });
    api.languages().then(setAllLanguages);
    const un = listen("dictations-changed", () => refresh());
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? entries.filter((e) => e.text.toLowerCase().includes(q)) : entries;
  }, [entries, query]);

  const days = useMemo(() => {
    const groups: [string, DictationEntry[]][] = [];
    for (const e of shown) {
      const day = DAY.format(new Date(e.createdAt));
      if (groups[groups.length - 1]?.[0] !== day) groups.push([day, []]);
      groups[groups.length - 1][1].push(e);
    }
    return groups;
  }, [shown]);

  const status = languageCheck(engine, languages, allLanguages);

  const act = async (f: () => Promise<unknown>) => {
    onError(null);
    try {
      await f();
    } catch (e) {
      onError(String(e));
    }
  };

  const retranscribe = async (id: string, engine: EngineId) => {
    setRetranscribing(id);
    await act(() => dictationsApi.retranscribe(id, engine, languages));
    setRetranscribing(null);
  };

  const copy = (e: DictationEntry) => {
    navigator.clipboard.writeText(e.text);
    setCopied(e.id);
    setTimeout(() => setCopied((c) => (c === e.id ? null : c)), 1500);
  };

  if (entries.length === 0) {
    return (
      <div className="empty">
        <Mic size={40} strokeWidth={1.5} />
        <p>Tu pojawi się każde dyktowanie: tekst i nagranie. Gdy coś wyjdzie źle, przepiszesz je ponownie innym modelem albo w innym języku.</p>
      </div>
    );
  }

  return (
    <section className="detail dictations">
      <header className="detail-head">
        <h1>Dyktowania</h1>
        <div className="hint">
          {countLabel(entries.length)} · nagrania są usuwane razem z nagraniami spotkań (ustawienia), tekst zostaje
        </div>
      </header>
      <div className="toolbar">
        <div className="group">
          <span className="hint">Przepisuj ponownie:</span>
          <select value={engine ?? ""} onChange={(e) => setEngine(e.target.value as EngineId)}>
            {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
              <option key={id} value={id}>
                {ENGINE_LABELS[id]}
              </option>
            ))}
          </select>
          <LanguagePicker value={languages} languages={allLanguages} onChange={setLanguages} />
        </div>
      </div>
      {(status.error || status.warning) && <div className="lang-status error">{status.error ?? status.warning}</div>}
      {shown.length === 0 && <div className="hint">Nic nie pasuje do wyszukiwania.</div>}
      {days.map(([day, list]) => (
        <div key={day} className="dict-day">
          <h2>{day}</h2>
          {list.map((e) => (
            <article key={e.id} className="dict-entry">
              <div className="dict-meta">
                {TIME.format(new Date(e.createdAt))} · {clock(e.durationSeconds)} · {e.engine}
                {e.pasted ? "" : " · tylko do schowka"}
                {e.audioDeleted ? " · nagranie usunięte" : ""}
              </div>
              <p className="dict-text">{retranscribing === e.id ? "Przepisywanie…" : e.text}</p>
              <div className="dict-actions">
                <button className="icon" title="Kopiuj tekst" aria-label="Kopiuj tekst" onClick={() => copy(e)}>
                  <Copy size={15} /> {copied === e.id && <span className="hint">skopiowano</span>}
                </button>
                <button
                  className="icon"
                  disabled={e.audioDeleted || !!retranscribing || !engine || !!status.error}
                  title={e.audioDeleted ? "Nagranie zostało usunięte" : (status.error ?? "Przepisz ponownie wybranym modelem i językiem")}
                  aria-label="Przepisz ponownie"
                  onClick={() => engine && retranscribe(e.id, engine)}
                >
                  <Wand2 size={15} />
                </button>
                <button
                  className="icon"
                  disabled={e.audioDeleted}
                  title="Pobierz nagranie (WAV)"
                  aria-label="Pobierz nagranie"
                  onClick={() => act(() => dictationsApi.exportAudio(e.id))}
                >
                  <Download size={15} />
                </button>
                <button
                  className="icon danger"
                  title="Usuń dyktowanie"
                  aria-label="Usuń dyktowanie"
                  onClick={() => confirm("Usunąć to dyktowanie razem z nagraniem?") && act(() => dictationsApi.remove(e.id))}
                >
                  <Trash2 size={15} />
                </button>
              </div>
            </article>
          ))}
        </div>
      ))}
    </section>
  );
}
