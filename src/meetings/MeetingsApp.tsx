import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Circle, Download, FileAudio, FileText, FolderOpen, Mic, Pencil, Sparkles, Square, Trash2, Wand2, X } from "lucide-react";
import {
  aiApi,
  api,
  clock,
  draftList,
  ENGINE_LABELS,
  meetingsApi,
  nextDrafts,
  shortDate,
  STATE_LABELS,
  type AudioExport,
  type AudioTrack,
  type EngineId,
  type JobEvent,
  type LanguageInfo,
  type LiveDrafts,
  type LivePayload,
  type Meeting,
  type MeetingDetail,
  type ProviderInfo,
  type RecordingStatus,
  type Settings,
  type Utterance,
} from "../api";
import Markdown from "../components/Markdown";
import LanguagePicker, { languageCheck } from "./LanguagePicker";

type Tab = "summary" | "transcript";

export default function MeetingsApp() {
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [job, setJob] = useState<JobEvent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState<Utterance[]>([]);
  const [drafts, setDrafts] = useState<LiveDrafts>({});
  const [liveError, setLiveError] = useState<string | null>(null);

  const refreshList = useCallback(() => meetingsApi.list().then(setMeetings), []);
  const refreshDetail = useCallback(() => {
    if (selected) meetingsApi.get(selected).then(setDetail).catch(() => setDetail(null));
  }, [selected]);

  useEffect(() => {
    refreshList();
    meetingsApi.status().then(setStatus);
    meetingsApi.job().then(setJob);
    const uns = [
      listen("meetings-changed", () => {
        refreshList();
        meetingsApi.status().then(setStatus);
      }),
      listen<RecordingStatus>("meeting-status", (e) => setStatus(e.payload)),
      listen<JobEvent>("meeting-job", (e) => {
        setJob(e.payload.finished ? null : e.payload);
        if (e.payload.finished && e.payload.error && e.payload.error !== "Przerwano") setError(e.payload.error);
      }),
      listen<LivePayload>("meeting-live", (e) => {
        const { utterance, error: err } = e.payload;
        if (utterance) setLive((prev) => [...prev, utterance].sort((a, b) => a.start - b.start));
        setDrafts((prev) => nextDrafts(prev, e.payload));
        if (err) setLiveError(err);
      }),
    ];
    return () => uns.forEach((u) => u.then((f) => f()));
  }, [refreshList]);

  // Okno otwarte w trakcie spotkania: dociągnij to, co już przepisano; po nagraniu wyczyść.
  useEffect(() => {
    if (status?.recording) {
      meetingsApi.liveTranscript().then((u) => setLive((prev) => (prev.length > u.length ? prev : u)));
    } else {
      setLive([]);
      setDrafts({});
      setLiveError(null);
    }
  }, [status?.recording, status?.meeting_id]);

  useEffect(() => {
    if (!selected && meetings.length) setSelected(meetings[0].id);
  }, [meetings, selected]);

  useEffect(() => {
    refreshDetail();
  }, [refreshDetail, meetings]);

  const toggleRecording = async () => {
    setError(null);
    try {
      if (status?.recording) {
        const m = await meetingsApi.stop();
        setSelected(m.id);
      } else {
        const m = await meetingsApi.start();
        setSelected(m.id);
      }
    } catch (e) {
      setError(String(e));
    }
    meetingsApi.status().then(setStatus);
    refreshList();
  };

  const importFile = async () => {
    setError(null);
    try {
      const m = await meetingsApi.importFile();
      if (m) setSelected(m.id);
    } catch (e) {
      setError(String(e));
    }
    refreshList();
  };

  return (
    <div className="layout meetings">
      <nav className="sidebar meeting-list">
        <div className="brand">Spotkania</div>
        <button className={`record ${status?.recording ? "on" : ""}`} onClick={toggleRecording}>
          {status?.recording ? <Square size={14} fill="currentColor" /> : <Circle size={14} fill="currentColor" />}
          {status?.recording ? `Zatrzymaj · ${clock(status.seconds)}` : "Nagraj spotkanie"}
        </button>
        <button className="import" onClick={importFile} title="Plik z nagraniem rozmowy (MP3, M4A, WAV, FLAC, OGG, Opus…) — zostanie przepisany jak spotkanie">
          <FileAudio size={14} /> Importuj nagranie
        </button>
        {status?.recording && status.warning && <div className="hint warn-text">{status.warning}</div>}
        <div className="list">
          {meetings.length === 0 && <div className="hint">Brak nagrań. Kliknij „Nagraj spotkanie”, użyj skrótu z ustawień albo zaimportuj plik z nagraniem.</div>}
          {meetings.map((m) => (
            <button key={m.id} className={`item ${selected === m.id ? "active" : ""}`} onClick={() => setSelected(m.id)}>
              <span className="item-title">{m.title || shortDate(m.startedAt)}</span>
              <span className="item-meta">
                {m.title ? `${shortDate(m.startedAt)} · ` : ""}
                {clock(m.durationSeconds)} · <span className={`state state-${m.state}`}>{STATE_LABELS[m.state]}</span>
              </span>
            </button>
          ))}
        </div>
      </nav>
      <main className="content">
        {error && (
          <div className="banner warn closable">
            <AlertTriangle size={16} /> <span>{error}</span>
            <button className="link" onClick={() => setError(null)} aria-label="Zamknij">
              <X size={14} />
            </button>
          </div>
        )}
        {detail ? (
          <Detail
            detail={detail}
            job={job?.meeting_id === detail.meeting.id ? job : null}
            busy={!!job}
            live={status?.recording && status.meeting_id === detail.meeting.id ? { items: live, drafts: draftList(drafts), error: liveError } : null}
            onError={setError}
            onChanged={refreshList}
          />
        ) : (
          <div className="empty">
            <Mic size={40} strokeWidth={1.5} />
            <p>Nagrywaj spotkania w Meet, Zoom czy Teams: Dyktando X zapisze Twój mikrofon i głosy rozmówców, przepisze je lokalnie i — jeśli chcesz — podsumuje przez AI.</p>
          </div>
        )}
      </main>
    </div>
  );
}

/** Pobieranie nagrania: mikrofon to Ty, dźwięk aplikacji to rozmówcy, całość to obie ścieżki zmiksowane. */
const EXPORTS: { track: AudioExport; needs: AudioTrack[]; label: string; title: string }[] = [
  { track: "mic", needs: ["mic"], label: "Mój głos", title: "Pobierz nagranie z mikrofonu (to, co mówisz) jako WAV" },
  { track: "system", needs: ["system"], label: "Rozmówcy", title: "Pobierz nagranie rozmówców (dźwięk aplikacji) jako WAV" },
  { track: "mixed", needs: ["mic", "system"], label: "Całe nagranie", title: "Pobierz całą rozmowę (Ty i rozmówcy w jednym pliku) jako WAV" },
];

function Detail({
  detail,
  job,
  busy,
  live,
  onError,
  onChanged,
}: {
  detail: MeetingDetail;
  job: JobEvent | null;
  busy: boolean;
  live: { items: Utterance[]; drafts: Utterance[]; error: string | null } | null;
  onError: (e: string | null) => void;
  onChanged: () => void;
}) {
  const m = detail.meeting;
  const [tab, setTab] = useState<Tab>(detail.summaries.length && !live ? "summary" : "transcript");
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [engine, setEngine] = useState<EngineId | null>(null);
  const [provider, setProvider] = useState<string>("");
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(m.title ?? "");
  const [summaryIndex, setSummaryIndex] = useState(0);
  const [exporting, setExporting] = useState<AudioExport | null>(null);
  const [allLanguages, setAllLanguages] = useState<LanguageInfo[]>([]);
  const [defaultLanguages, setDefaultLanguages] = useState<string[]>([]);
  // Wybór użytkownika; `null` = jak przy ostatniej transkrypcji, a bez niej jak w ustawieniach.
  const [chosenLanguages, setChosenLanguages] = useState<string[] | null>(null);

  useEffect(() => {
    api.getSettings().then((s: Settings) => {
      setEngine((e) => e ?? s.meeting_engine);
      setProvider((p) => p || s.ai_provider);
      setDefaultLanguages(s.meeting_language === "auto" ? [] : [s.meeting_language]);
    });
    aiApi.providers().then(setProviders);
    api.languages().then(setAllLanguages);
  }, []);

  useEffect(() => {
    setTitle(m.title ?? "");
    setChosenLanguages(null);
    setEditing(false);
    setSummaryIndex(0);
    setTab(detail.summaries.length && !live ? "summary" : "transcript");
  }, [m.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const recording = m.state === "recording";
  const canTranscribe = !m.audioDeleted && !recording && !busy;
  const canSummarize = !!detail.transcript && !recording && !busy;
  const languages = chosenLanguages ?? m.transcriptLanguages ?? defaultLanguages;
  const languageStatus = languageCheck(engine, languages, allLanguages);
  const canExport = !recording && m.state !== "importing" && !exporting;
  const summary = detail.summaries[summaryIndex];
  const providerInfo = useMemo(() => providers.find((p) => p.id === provider), [providers, provider]);

  const run = async (f: () => Promise<void>) => {
    onError(null);
    try {
      await f();
    } catch (e) {
      if (String(e) !== "Przerwano") onError(String(e));
    }
    onChanged();
  };

  const exportAudio = async (track: AudioExport) => {
    onError(null);
    setExporting(track);
    try {
      await meetingsApi.exportAudio(m.id, track);
    } catch (e) {
      onError(String(e));
    } finally {
      setExporting(null);
    }
  };

  const copy = () => {
    const text = tab === "summary" ? summary?.content : live ? liveText(live.items) : detail.transcript;
    if (text) navigator.clipboard.writeText(text);
  };

  return (
    <section className="detail">
      <header className="detail-head">
        {editing ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              meetingsApi.rename(m.id, title).then(() => {
                setEditing(false);
                onChanged();
              });
            }}
          >
            <input autoFocus value={title} placeholder={shortDate(m.startedAt)} onChange={(e) => setTitle(e.target.value)} />
            <button type="submit" className="primary">
              Zapisz
            </button>
          </form>
        ) : (
          <h1>
            {m.title || `Spotkanie ${shortDate(m.startedAt)}`}
            <button className="icon" onClick={() => setEditing(true)} title="Zmień nazwę" aria-label="Zmień nazwę">
              <Pencil size={14} />
            </button>
          </h1>
        )}
        <div className="hint">
          {shortDate(m.startedAt)} · {clock(m.durationSeconds)} · {m.hasSystemAudio ? "mikrofon + rozmówcy" : "tylko mikrofon"}
          {m.transcriptEngine ? ` · ${m.transcriptEngine}` : ""}
          {m.audioDeleted ? " · nagranie usunięte (zostaje tekst)" : ""} · <span className={`state state-${m.state}`}>{STATE_LABELS[m.state]}</span>
        </div>
        {m.lastError && <div className="error">{m.lastError}</div>}
      </header>

      <div className="toolbar">
        <div className="group">
          <select value={engine ?? ""} onChange={(e) => setEngine(e.target.value as EngineId)} disabled={!canTranscribe}>
            {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
              <option key={id} value={id}>
                {ENGINE_LABELS[id]}
              </option>
            ))}
          </select>
          <LanguagePicker value={languages} languages={allLanguages} disabled={!canTranscribe} onChange={setChosenLanguages} />
          <button
            disabled={!canTranscribe || !!languageStatus.error}
            title={languageStatus.error ?? ""}
            onClick={() => run(() => meetingsApi.transcribe(m.id, engine ?? undefined, languages))}
          >
            <Wand2 size={14} /> {detail.transcript ? "Przepisz ponownie" : "Przepisz"}
          </button>
        </div>
        <div className="group">
          <select value={provider} onChange={(e) => setProvider(e.target.value)} disabled={!canSummarize}>
            {providers.map((p) => (
              <option key={p.id} value={p.id} disabled={!p.has_key}>
                {p.name}
                {p.has_key ? "" : " (brak klucza)"}
              </option>
            ))}
          </select>
          <button
            disabled={!canSummarize || !providerInfo?.has_key}
            title={providerInfo?.has_key ? "" : "Dodaj klucz API w Ustawieniach → AI"}
            onClick={() => providerInfo && run(() => meetingsApi.summarize(m.id, providerInfo.id))}
          >
            <Sparkles size={14} /> Podsumuj
          </button>
        </div>
        <div className="group right">
          {EXPORTS.filter((x) => x.needs.every((t) => detail.tracks.includes(t))).map((x) => (
            <button key={x.track} disabled={!canExport} title={x.title} onClick={() => exportAudio(x.track)}>
              <Download size={14} /> {exporting === x.track ? "Zapisywanie…" : x.label}
            </button>
          ))}
          <button className="icon" onClick={() => meetingsApi.reveal(m.id)} title="Pokaż pliki" aria-label="Pokaż pliki">
            <FolderOpen size={16} />
          </button>
          <button
            className="icon danger"
            disabled={recording}
            title="Usuń spotkanie"
            aria-label="Usuń spotkanie"
            onClick={() => {
              if (confirm("Usunąć to spotkanie razem z nagraniem, transkryptem i podsumowaniami?")) run(() => meetingsApi.remove(m.id));
            }}
          >
            <Trash2 size={16} />
          </button>
        </div>
      </div>

      {canTranscribe && (languageStatus.error || languageStatus.warning || (chosenLanguages && languageStatus.hint)) && (
        <div className={languageStatus.error || languageStatus.warning ? "lang-status error" : "lang-status"}>
          {languageStatus.error ?? languageStatus.warning ?? languageStatus.hint}
        </div>
      )}

      {job && (
        <div className="job">
          <div className="progress">
            <span style={{ width: `${Math.round(job.fraction * 100)}%` }} />
            <em>
              {job.step} — {Math.round(job.fraction * 100)}%
            </em>
          </div>
          <button onClick={() => meetingsApi.cancelJob()}>Przerwij</button>
        </div>
      )}

      <div className="tabs">
        <button className={tab === "summary" ? "active" : ""} onClick={() => setTab("summary")}>
          <Sparkles size={14} /> Podsumowanie {detail.summaries.length > 1 ? `(${detail.summaries.length})` : ""}
        </button>
        <button className={tab === "transcript" ? "active" : ""} onClick={() => setTab("transcript")}>
          <FileText size={14} /> Transkrypt
        </button>
        <button className="link right" onClick={copy}>
          Kopiuj
        </button>
      </div>

      <div className="doc">
        {tab === "summary" &&
          (summary ? (
            <>
              {detail.summaries.length > 1 && (
                <select value={summaryIndex} onChange={(e) => setSummaryIndex(Number(e.target.value))}>
                  {detail.summaries.map((s, i) => (
                    <option key={s.name} value={i}>
                      {s.name.replace(/\.md$/, "")}
                    </option>
                  ))}
                </select>
              )}
              <Markdown text={summary.content} />
            </>
          ) : (
            <p className="hint">
              {detail.transcript
                ? "Brak podsumowania. Wybierz dostawcę i kliknij „Podsumuj” — transkrypt zostanie wysłany do wybranego dostawcy AI."
                : "Najpierw przepisz spotkanie."}
            </p>
          ))}
        {tab === "transcript" &&
          (live ? (
            <LiveTranscript items={live.items} drafts={live.drafts} error={live.error} />
          ) : detail.transcript ? (
            <Markdown text={detail.transcript} />
          ) : (
            <p className="hint">{recording ? "Trwa nagrywanie…" : "Brak transkryptu. Kliknij „Przepisz”."}</p>
          ))}
      </div>
    </section>
  );
}

function liveText(items: Utterance[]): string {
  return items.map((u) => `[${clock(u.start)}] ${u.speaker}: ${u.text}${u.translation ? `\n    ${u.translation}` : ""}`).join("\n");
}

/// Transkrypcja na żywo: domknięte wypowiedzi i (szarym) tekst roboczy trwających — odświeżany co
/// ~1,5 s, bez czekania na pauzę. Widok sam przewija do końca.
function LiveTranscript({ items, drafts, error }: { items: Utterance[]; drafts: Utterance[]; error: string | null }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "nearest" });
  }, [items.length, drafts]);
  return (
    <div className="live">
      <p className="hint">
        <span className="live-dot" /> Na żywo — tekst tymczasowy, bez rozpoznawania rozmówców. Pełny transkrypt powstanie po zakończeniu nagrania.
      </p>
      {error && <div className="error">Transkrypcja na żywo niedostępna: {error}</div>}
      {items.length === 0 && drafts.length === 0 && !error && <p className="hint">Słucham…</p>}
      {[...items, ...drafts].map((u, i) => (
        <div key={`${u.track}-${u.start}-${i >= items.length ? "draft" : ""}`} className={`live-line${i >= items.length ? " draft" : ""}`}>
          <time>{clock(u.start)}</time>
          <b className={u.track === "mic" ? "me" : ""}>{u.speaker}:</b> {u.text}
          {u.translation && <div className="live-translation">{u.translation}</div>}
        </div>
      ))}
      <div ref={end} />
    </div>
  );
}
