import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, Languages } from "lucide-react";
import type { EngineId, LanguageInfo } from "../api";

/** Krótki opis wyboru na przycisku: „auto”, „polski”, „polski + angielski”, „polski + 2”. */
export function languagesLabel(codes: string[], all: LanguageInfo[]): string {
  const name = (c: string) => all.find((l) => l.code === c)?.name ?? c;
  if (codes.length === 0) return "Język: auto";
  if (codes.length <= 2) return codes.map(name).join(" + ");
  return `${name(codes[0])} + ${codes.length - 1}`;
}

/**
 * Czy wybrany model przepisze w tych językach (lustro `Engine::check_languages`).
 * `error` blokuje przepisywanie, `hint` tylko wyjaśnia.
 */
export function languageCheck(engine: EngineId | null, codes: string[], all: LanguageInfo[]): { error?: string; hint?: string } {
  if (!engine || engine.startsWith("whisper")) {
    return codes.length > 1 ? { hint: "Każda wypowiedź zostanie przepisana w tym z wybranych języków, w którym jest." } : {};
  }
  const model = engine === "canary_v2" ? "Canary" : "Parakeet";
  if (engine === "canary_v2" && codes.length > 1) {
    return { error: "Canary nie rozpoznaje języka sam — wybierz jeden język albo Whispera." };
  }
  const unknown = codes.filter((c) => !all.find((l) => l.code === c)?.european);
  if (unknown.length) {
    const names = unknown.map((c) => all.find((l) => l.code === c)?.name ?? c).join(", ");
    return { error: `${model} nie zna: ${names} — wybierz Whispera.` };
  }
  if (engine === "parakeet_v3" && codes.length) {
    return { hint: "Parakeet sam rozpoznaje język — wybór języka działa z Whisperem i Canary." };
  }
  return {};
}

/** Wybór języków przepisywania: żaden (model rozpoznaje sam), jeden albo kilka (rozmowa mieszana). */
export default function LanguagePicker({
  value,
  languages,
  disabled,
  onChange,
}: {
  value: string[];
  languages: LanguageInfo[];
  disabled?: boolean;
  onChange: (codes: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const escape = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? languages.filter((l) => l.name.toLowerCase().includes(q) || l.code === q) : languages;
  }, [languages, query]);

  const toggle = (code: string) => onChange(value.includes(code) ? value.filter((c) => c !== code) : [...value, code]);

  return (
    <div className="lang-picker" ref={root}>
      <button
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        title="Języki rozmowy — przy słabej transkrypcji wskaż je i przepisz ponownie"
        onClick={() => {
          setQuery("");
          setOpen((o) => !o);
        }}
      >
        <Languages size={14} /> {languagesLabel(value, languages)} <ChevronDown size={12} />
      </button>
      {open && (
        <div className="lang-pop">
          <input autoFocus placeholder="Szukaj języka…" value={query} onChange={(e) => setQuery(e.target.value)} />
          <div className="lang-list" role="listbox" aria-multiselectable="true">
            {!query && (
              <button role="option" aria-selected={value.length === 0} className={value.length === 0 ? "on" : ""} onClick={() => onChange([])}>
                <span className="tick">{value.length === 0 && <Check size={13} />}</span> Automatycznie (model rozpozna sam)
              </button>
            )}
            {shown.map((l) => {
              const on = value.includes(l.code);
              return (
                <button key={l.code} role="option" aria-selected={on} className={on ? "on" : ""} onClick={() => toggle(l.code)}>
                  <span className="tick">{on && <Check size={13} />}</span> {l.name}
                  {!l.european && <span className="lang-note">tylko Whisper</span>}
                </button>
              );
            })}
            {shown.length === 0 && <div className="lang-empty">Brak takiego języka</div>}
          </div>
          <div className="lang-foot">Zaznacz kilka, jeśli rozmowa była w kilku językach.</div>
        </div>
      )}
    </div>
  );
}
