import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { FileText, Square, X } from "lucide-react";
import { clock, draftList, meetingsApi, nextDrafts, type LiveDrafts, type LivePayload, type RecordingStatus, type Utterance } from "../api";

/** How many recent utterances to keep in the small window. */
const KEEP = 60;

/** The "live" window during recording: timer, levels of both tracks, recent utterances. */
export default function LiveWindow() {
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [items, setItems] = useState<Utterance[]>([]);
  const [drafts, setDrafts] = useState<LiveDrafts>({});
  const [liveError, setLiveError] = useState<string | null>(null);
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    meetingsApi.status().then(setStatus);
    meetingsApi.liveTranscript().then((u) => setItems((prev) => (prev.length > u.length ? prev : u)));
    const uns = [
      listen<RecordingStatus>("meeting-status", (e) => setStatus(e.payload)),
      listen<LivePayload>("meeting-live", (e) => {
        const { utterance, error } = e.payload;
        if (utterance) setItems((prev) => [...prev, utterance].sort((a, b) => a.start - b.start).slice(-KEEP));
        setDrafts((prev) => nextDrafts(prev, e.payload));
        if (error) setLiveError(error);
      }),
      listen("meetings-changed", () => {
        meetingsApi.status().then((s) => {
          setStatus(s);
          if (!s.recording) {
            setItems([]);
            setDrafts({});
            setLiveError(null);
          } else {
            meetingsApi.liveTranscript().then(setItems);
          }
        });
      }),
    ];
    return () => uns.forEach((u) => u.then((f) => f()));
  }, []);

  useEffect(() => {
    end.current?.scrollIntoView({ block: "nearest" });
  }, [items.length, drafts]);

  const recording = !!status?.recording;
  return (
    <div className="live-win">
      <div className="live-head" data-tauri-drag-region>
        <span className="live-dot" data-tauri-drag-region />
        <span className="clock" data-tauri-drag-region>
          {recording ? clock(status!.seconds) : "—"}
        </span>
        <div className="live-meters" data-tauri-drag-region>
          <Meter label="Ja" level={status?.mic_level ?? 0} />
          {status?.has_system_audio ? <Meter label="Rozmówcy" level={status.system_level} /> : <span className="live-meter">tylko mikrofon</span>}
        </div>
        <span className="spacer" data-tauri-drag-region />
        <button className="icon" title="Okno spotkań" aria-label="Okno spotkań" onClick={() => meetingsApi.open()}>
          <FileText size={15} />
        </button>
        <button className="icon stop" title="Zatrzymaj nagranie" aria-label="Zatrzymaj nagranie" disabled={!recording} onClick={() => meetingsApi.stop().catch(() => {})}>
          <Square size={15} fill="currentColor" />
        </button>
        <button className="icon" title="Schowaj okno" aria-label="Schowaj okno" onClick={() => meetingsApi.hideLiveWindow()}>
          <X size={15} />
        </button>
      </div>
      {status?.warning && <div className="live-warn">{status.warning}</div>}
      {liveError && <div className="live-warn">Transkrypcja na żywo niedostępna: {liveError}</div>}
      <div className="live-body">
        {items.length === 0 && draftList(drafts).length === 0 && !liveError && <p className="hint">{recording ? "Słucham…" : "Nic nie jest nagrywane."}</p>}
        {[...items, ...draftList(drafts)].map((u, i) => (
          <div key={`${u.track}-${u.start}-${i >= items.length ? "draft" : ""}`} className={`live-line${i >= items.length ? " draft" : ""}`}>
            <b className={u.track === "mic" ? "me" : ""}>{u.speaker}:</b>
            {u.text}
            {u.translation && <div className="live-translation">{u.translation}</div>}
          </div>
        ))}
        <div ref={end} />
      </div>
    </div>
  );
}

function Meter({ label, level }: { label: string; level: number }) {
  const scaled = Math.min(1, level * 8);
  return (
    <span className={`live-meter ${scaled < 0.03 ? "silent" : ""}`} data-tauri-drag-region>
      {label}
      <span>
        <i style={{ transform: `scaleX(${Math.max(0.04, scaled)})` }} />
      </span>
    </span>
  );
}
