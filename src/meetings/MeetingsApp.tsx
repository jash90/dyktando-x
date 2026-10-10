import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Circle, Download, LoaderCircle, FileAudio, FileText, FolderOpen, Mic, Pencil, Sparkles, Square, Trash2, Wand2, X } from "lucide-react";
import {
  aiApi,
  api,
  clock,
  draftList,
  ENGINE_LABELS,
  meetingsApi,
  nextDrafts,
  shortDate,
  isCancelled,
  stateLabel,
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
import DictationsView from "./DictationsView";
import Transcript, { speakerLabel } from "./Transcript";
import { useT, type PlainKey } from "../i18n";

type Tab = "summary" | "transcript";

export default function MeetingsApp() {
  const { t } = useT();
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [job, setJob] = useState<JobEvent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<"meetings" | "dictations">("meetings");
  const [query, setQuery] = useState("");
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
        if (e.payload.finished && e.payload.error && !isCancelled(e.payload.error)) setError(e.payload.error);
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

  // Window opened during a meeting: fetch what has already been transcribed; clear it after recording.
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
        <div className="view-switch" role="tablist">
          <button role="tab" aria-selected={view === "meetings"} className={view === "meetings" ? "on" : ""} onClick={() => setView("meetings")}>
            {t("meetings.tab_meetings")}
          </button>
          <button role="tab" aria-selected={view === "dictations"} className={view === "dictations" ? "on" : ""} onClick={() => setView("dictations")}>
            {t("meetings.tab_dictations")}
          </button>
        </div>
        {view === "dictations" ? (
          <>
            <input className="search" placeholder={t("meetings.search_dictations")} value={query} onChange={(e) => setQuery(e.target.value)} />
            <div className="hint">{t("meetings.dictations_hint")}</div>
          </>
        ) : (
          <>
            <button className={`record ${status?.recording ? "on" : ""}`} onClick={toggleRecording}>
              {status?.recording ? <Square size={14} fill="currentColor" /> : <Circle size={14} fill="currentColor" />}
              {status?.recording ? t("meetings.stop", { time: clock(status.seconds) }) : t("meetings.record")}
            </button>
            <button className="import" onClick={importFile} title={t("meetings.import_title")}>
              <FileAudio size={14} /> {t("meetings.import")}
            </button>
            {status?.recording && status.warning && <div className="hint warn-text">{status.warning}</div>}
            <div className="list">
              {meetings.length === 0 && <div className="hint">{t("meetings.empty_list")}</div>}
              {meetings.map((m) => (
                <button key={m.id} className={`item ${selected === m.id ? "active" : ""}`} onClick={() => setSelected(m.id)}>
                  <span className="item-title">{m.title || shortDate(m.startedAt)}</span>
                  <span className="item-meta">
                    {m.title ? `${shortDate(m.startedAt)} · ` : ""}
                    {clock(m.durationSeconds)} · <span className={`state state-${m.state}`}>{stateLabel(m.state)}</span>
                  </span>
                </button>
              ))}
            </div>
          </>
        )}
      </nav>
      <main className="content">
        {error && (
          <div className="banner warn closable">
            <AlertTriangle size={16} /> <span>{error}</span>
            <button className="link" onClick={() => setError(null)} aria-label={t("common.close")}>
              <X size={14} />
            </button>
          </div>
        )}
        {view === "dictations" ? (
          <DictationsView query={query} onError={setError} />
        ) : detail ? (
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
            <p>{t("meetings.empty")}</p>
          </div>
        )}
      </main>
    </div>
  );
}

/** Recording download: microphone = you, app audio = the other participants, full = both tracks mixed. */
const EXPORTS: { track: AudioExport; needs: AudioTrack[]; label: PlainKey; title: PlainKey }[] = [
  { track: "mic", needs: ["mic"], label: "export.mic", title: "export.mic_title" },
  { track: "system", needs: ["system"], label: "export.system", title: "export.system_title" },
  { track: "mixed", needs: ["mic", "system"], label: "export.mixed", title: "export.mixed_title" },
];

/** „Nagranie ▾” ("Recording ▾") — download a single track or the whole conversation as WAV. */
function DownloadMenu({
  tracks,
  disabled,
  exporting,
  onPick,
}: {
  tracks: AudioTrack[];
  disabled: boolean;
  exporting: AudioExport | null;
  onPick: (track: AudioExport) => void;
}) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const escape = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  const options = EXPORTS.filter((x) => x.needs.every((t) => tracks.includes(t)));
  if (options.length === 0) return null;
  return (
    <div className="menu" ref={root}>
      <button
        className="icon"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-busy={!!exporting}
        title={exporting ? t("export.saving") : t("common.download_wav")}
        onClick={() => setOpen((o) => !o)}
      >
        {exporting ? <LoaderCircle size={17} className="spin" /> : <Download size={17} />}
      </button>
      {open && (
        <div className="menu-pop" role="menu">
          {options.map((x) => (
            <button
              key={x.track}
              role="menuitem"
              disabled={!!exporting}
              onClick={() => {
                setOpen(false);
                onPick(x.track);
              }}
            >
              <Download size={14} />
              <span>
                {t(x.label)}
                <small>{t(x.title)}</small>
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

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
  const { t } = useT();
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
  // User's choice; `null` = same as the last transcription, or the settings if there was none.
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
      if (!isCancelled(e)) onError(String(e));
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
              {t("common.save")}
            </button>
          </form>
        ) : (
          <div className="title-row">
            <h1>
              {m.title || t("meeting.default_title", { date: shortDate(m.startedAt) })}
              <button className="icon" onClick={() => setEditing(true)} title={t("meeting.rename")} aria-label={t("meeting.rename")}>
                <Pencil size={15} />
              </button>
            </h1>
            <div className="title-actions">
              <DownloadMenu tracks={detail.tracks} disabled={!canExport} exporting={exporting} onPick={exportAudio} />
              <button className="icon" onClick={() => meetingsApi.reveal(m.id)} title={t("meeting.reveal")} aria-label={t("meeting.reveal")}>
                <FolderOpen size={17} />
              </button>
              <button
                className="icon danger"
                disabled={recording}
                title={t("meeting.delete")}
                aria-label={t("meeting.delete")}
                onClick={() => {
                  if (confirm(t("meeting.confirm_delete"))) run(() => meetingsApi.remove(m.id));
                }}
              >
                <Trash2 size={17} />
              </button>
            </div>
          </div>
        )}
        <div className="meta">
          <span className="chip mono">{shortDate(m.startedAt)}</span>
          <span className="chip mono">{clock(m.durationSeconds)}</span>
          <span className="chip">{m.hasSystemAudio ? t("meeting.mic_and_others") : t("meeting.mic_only")}</span>
          {m.transcriptEngine && <span className="chip">{m.transcriptEngine}</span>}
          {m.audioDeleted && <span className="chip">{t("meeting.audio_deleted")}</span>}
          <span className="chip">
            <span className={`state state-${m.state}`}>{stateLabel(m.state)}</span>
          </span>
        </div>
        {m.lastError && <div className="error">{m.lastError}</div>}
        {m.audioGaps?.length > 0 && (
          <div className="error">
            {t("meeting.gaps")}{" "}
            {m.audioGaps
              .map((g) => t("meeting.gap", { track: g.track === "mic" ? t("meeting.track_mic") : t("meeting.track_system"), at: clock(g.start), seconds: Math.round(g.seconds) }))
              .join(", ")}
            .{" "}
            <button className="link" onClick={() => api.revealLogs()}>
              {t("common.show_logs")}
            </button>
          </div>
        )}
      </header>

      <div className="controls">
        <div className="control">
          <div className="control-label">
            <Wand2 size={12} /> {t("meeting.transcription")}
          </div>
          <div className="control-row">
            <select value={engine ?? ""} onChange={(e) => setEngine(e.target.value as EngineId)} disabled={!canTranscribe} aria-label={t("common.model")}>
              {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
                <option key={id} value={id}>
                  {ENGINE_LABELS[id]}
                </option>
              ))}
            </select>
            <LanguagePicker value={languages} languages={allLanguages} disabled={!canTranscribe} onChange={setChosenLanguages} />
            <button
              className="go primary"
              disabled={!canTranscribe || !!languageStatus.error}
              title={languageStatus.error ?? (detail.transcript ? t("common.retranscribe_hint") : t("meeting.transcribe_hint"))}
              onClick={() => run(() => meetingsApi.transcribe(m.id, engine ?? undefined, languages))}
            >
              {t("meeting.transcribe")}
            </button>
          </div>
        </div>
        <div className="control">
          <div className="control-label">
            <Sparkles size={12} /> {t("meeting.ai_summary")}
          </div>
          <div className="control-row">
            <select value={provider} onChange={(e) => setProvider(e.target.value)} disabled={!canSummarize} aria-label={t("meeting.ai_provider")}>
              {providers.map((p) => (
                <option key={p.id} value={p.id} disabled={!p.has_key}>
                  {p.name}
                  {p.has_key ? "" : ` ${t("ai.no_key_suffix")}`}
                </option>
              ))}
            </select>
            <button
              className="go primary"
              disabled={!canSummarize || !providerInfo?.has_key}
              title={providerInfo?.has_key ? "" : t("meeting.add_key")}
              onClick={() => providerInfo && run(() => meetingsApi.summarize(m.id, providerInfo.id))}
            >
              {t("meeting.summarize")}
            </button>
          </div>
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
          <button onClick={() => meetingsApi.cancelJob()}>{t("common.cancel")}</button>
        </div>
      )}

      <div className="tabs">
        <button className={tab === "summary" ? "active" : ""} onClick={() => setTab("summary")}>
          <Sparkles size={14} /> {t("meeting.tab_summary")} {detail.summaries.length > 1 ? `(${detail.summaries.length})` : ""}
        </button>
        <button className={tab === "transcript" ? "active" : ""} onClick={() => setTab("transcript")}>
          <FileText size={14} /> {t("meeting.tab_transcript")}
        </button>
        <button className="link right" onClick={copy}>
          {t("meeting.copy")}
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
                ? t("meeting.no_summary")
                : t("meeting.transcribe_first")}
            </p>
          ))}
        {tab === "transcript" &&
          (live ? (
            <LiveTranscript items={live.items} drafts={live.drafts} error={live.error} />
          ) : detail.transcript ? (
            <Transcript text={detail.transcript} />
          ) : (
            <p className="hint">{recording ? t("meeting.recording_now") : t("meeting.no_transcript")}</p>
          ))}
      </div>
    </section>
  );
}

function liveText(items: Utterance[]): string {
  return items.map((u) => `[${clock(u.start)}] ${speakerLabel(u.speaker)}: ${u.text}${u.translation ? `\n    ${u.translation}` : ""}`).join("\n");
}

/// Live transcription: finalized utterances plus (in grey) draft text of ongoing ones — refreshed every
/// ~1.5 s, without waiting for a pause. The view auto-scrolls to the end.
function LiveTranscript({ items, drafts, error }: { items: Utterance[]; drafts: Utterance[]; error: string | null }) {
  const { t } = useT();
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "nearest" });
  }, [items.length, drafts]);
  return (
    <div className="live">
      <p className="hint">
        <span className="live-dot" /> {t("meeting.live_note")}
      </p>
      {error && <div className="error">{t("live.unavailable", { error })}</div>}
      {items.length === 0 && drafts.length === 0 && !error && <p className="hint">{t("live.listening")}</p>}
      {[...items, ...drafts].map((u, i) => (
        <div key={`${u.track}-${u.start}-${i >= items.length ? "draft" : ""}`} className={`live-line${i >= items.length ? " draft" : ""}`}>
          <time>{clock(u.start)}</time>
          <b className={u.track === "mic" ? "me" : ""}>{speakerLabel(u.speaker)}:</b> {u.text}
          {u.translation && <div className="live-translation">{u.translation}</div>}
        </div>
      ))}
      <div ref={end} />
    </div>
  );
}
