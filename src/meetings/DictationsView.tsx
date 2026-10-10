import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Copy, Download, Mic, Trash2, Wand2 } from "lucide-react";
import { api, clock, dictationsApi, ENGINE_LABELS, type DictationEntry, type EngineId, type LanguageInfo, type Settings } from "../api";
import LanguagePicker, { languageCheck } from "./LanguagePicker";
import { intlLocale, tn, useT } from "../i18n";

/** "1 dictation" / "5 dictations"; Polish: „1 dyktowanie”, „3 dyktowania”, „5 dyktowań”, „22 dyktowania”. */
export function countLabel(n: number): string {
  return tn("dictations.count", n);
}

/** Dictation history: entries newest first, grouped by day, with re-transcription. */
export default function DictationsView({ query, onError }: { query: string; onError: (e: string | null) => void }) {
  const { t, locale } = useT();
  const [DAY, TIME] = useMemo(
    () => [
      new Intl.DateTimeFormat(intlLocale(), { weekday: "long", day: "numeric", month: "long", year: "numeric" }),
      new Intl.DateTimeFormat(intlLocale(), { hour: "2-digit", minute: "2-digit" }),
    ],
    [locale], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const [entries, setEntries] = useState<DictationEntry[]>([]);
  const [engine, setEngine] = useState<EngineId | null>(null);
  const [languages, setLanguages] = useState<string[]>([]);
  const [allLanguages, setAllLanguages] = useState<LanguageInfo[]>([]);
  /** The entry currently being re-transcribed. */
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
  }, [shown, DAY]);

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
        <p>{t("dictations.empty")}</p>
      </div>
    );
  }

  return (
    <section className="detail dictations">
      <header className="detail-head">
        <h1>{t("dictations.title")}</h1>
        <div className="hint">
          {countLabel(entries.length)} · {t("dictations.retention_note")}
        </div>
      </header>
      <div className="toolbar">
        <div className="group">
          <span className="hint">{t("dictations.retranscribe_with")}</span>
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
      {shown.length === 0 && <div className="hint">{t("dictations.no_match")}</div>}
      {days.map(([day, list]) => (
        <div key={day} className="dict-day">
          <h2>{day}</h2>
          {list.map((e) => (
            <article key={e.id} className="dict-entry">
              <time className="dict-time">{TIME.format(new Date(e.createdAt))}</time>
              <div className="dict-body">
                <p className="dict-text">{retranscribing === e.id ? t("dictations.retranscribing") : e.text}</p>
                <div className="dict-meta">
                  {clock(e.durationSeconds)} · {e.engine}
                  {e.pasted ? "" : ` · ${t("dictations.clipboard_only")}`}
                  {e.audioDeleted ? ` · ${t("dictations.audio_deleted")}` : ""}
                </div>
              </div>
              <div className="dict-actions">
                <button className="icon" title={t("dictations.copy")} aria-label={t("dictations.copy")} onClick={() => copy(e)}>
                  <Copy size={15} /> {copied === e.id && <span className="hint">{t("dictations.copied")}</span>}
                </button>
                <button
                  className="icon"
                  disabled={e.audioDeleted || !!retranscribing || !engine || !!status.error}
                  title={e.audioDeleted ? t("dictations.audio_was_deleted") : (status.error ?? t("common.retranscribe_hint"))}
                  aria-label={t("dictations.retranscribe")}
                  onClick={() => engine && retranscribe(e.id, engine)}
                >
                  <Wand2 size={15} />
                </button>
                <button
                  className="icon"
                  disabled={e.audioDeleted}
                  title={t("common.download_wav")}
                  aria-label={t("dictations.download")}
                  onClick={() => act(() => dictationsApi.exportAudio(e.id))}
                >
                  <Download size={15} />
                </button>
                <button
                  className="icon danger"
                  title={t("dictations.delete")}
                  aria-label={t("dictations.delete")}
                  onClick={() => confirm(t("dictations.confirm_delete")) && act(() => dictationsApi.remove(e.id))}
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
