import { type CSSProperties } from "react";
import Markdown from "../components/Markdown";
import { t, useT } from "../i18n";

/** An utterance from `transcript.md`: „[00:01:30] **Rozmówca 1:** tekst” (speaker, text). */
interface Line {
  time: string;
  speaker: string;
  text: string;
  /** Live translation — a "> …" line right below the utterance. */
  translation?: string;
}

const LINE = /^\[(\d{2}:\d{2}:\d{2})\]\s+\*\*(.+?):\*\*\s*(.*)$/;
// Canonical speaker labels as written by the backend into transcript.md / live events — they stay
// Polish in the files (old meetings keep parsing); only the displayed label is translated.
const ME = "Ja";
const OTHERS = "Rozmówcy";
const SPEAKER_N = /^Rozmówca (\d+)$/;
const PALETTE = 6;

/** Display label for a canonical speaker label: "Ja" → "Me", "Rozmówca 2" → "Speaker 2"; custom names unchanged. */
export function speakerLabel(name: string): string {
  if (name === ME) return t("speaker.me");
  if (name === OTHERS) return t("speaker.others");
  const n = name.match(SPEAKER_N);
  return n ? t("speaker.numbered", { n: n[1] }) : name;
}

/** One chip of the „Długość: … · Model: … · Mówcy: …” header line, with translated labels. */
function metaLabel(part: string): string {
  const m = part.match(/^(Długość|Model|Mówcy):\s*(.*)$/);
  if (!m) return part;
  if (m[1] === "Długość") return t("transcript.meta.duration", { value: m[2] });
  if (m[1] === "Model") return t("transcript.meta.model", { value: m[2] });
  return t("transcript.meta.speakers", { value: m[2].split(", ").map(speakerLabel).join(", ") });
}

/** The same speaker always gets the same colour (number from the name, otherwise the sum of its character codes). */
function speakerColor(name: string): CSSProperties {
  const n = name.match(/(\d+)$/);
  const index = n ? Number(n[1]) - 1 : [...name].reduce((a, c) => a + c.charCodeAt(0), 0);
  return { "--sp": `var(--sp-${((index % PALETTE) + PALETTE) % PALETTE})` } as CSSProperties;
}

/** "00:01:30" → "1:30", "01:02:03" → "1:02:03". */
function shortTime(t: string): string {
  const [h, m, s] = t.split(":").map(Number);
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}` : `${m}:${String(s).padStart(2, "0")}`;
}

/** Transcript as a list of utterances (time, speaker, text); an unusual file falls back to plain Markdown. */
export default function Transcript({ text }: { text: string }) {
  useT(); // re-render on locale change
  const rows = text.split("\n");
  const lines: Line[] = [];
  for (const r of rows) {
    const m = r.match(LINE);
    if (m) {
      lines.push({ time: m[1], speaker: m[2], text: m[3] });
    } else if (r.startsWith(">") && lines.length) {
      const last = lines[lines.length - 1];
      const t = r.replace(/^>\s?/, "").trim();
      last.translation = last.translation ? `${last.translation} ${t}` : t;
    }
  }
  if (lines.length === 0) return <Markdown text={text} />;
  // The „Długość: … · Model: … · Mówcy: …” (duration, model, speakers) line under the title — rendered as badges.
  const meta = rows.find((r) => r.startsWith("Długość:"))?.split(" · ") ?? [];
  return (
    <div className="transcript">
      {meta.length > 0 && (
        <div className="transcript-meta">
          {meta.map((m) => (
            <span key={m} className="chip">
              {metaLabel(m)}
            </span>
          ))}
        </div>
      )}
      <div className="utterances">
        {lines.map((l, i) => {
          const continued = i > 0 && lines[i - 1].speaker === l.speaker;
          return (
            <div key={i} className={`utt${continued ? " cont" : ""}`} style={speakerColor(l.speaker)}>
              <time>{shortTime(l.time)}</time>
              {!continued && <span className={`who${l.speaker === ME ? " me" : ""}`}>{speakerLabel(l.speaker)}</span>}
              <p>{l.text}</p>
              {l.translation && <p className="translation">{l.translation}</p>}
            </div>
          );
        })}
      </div>
    </div>
  );
}
