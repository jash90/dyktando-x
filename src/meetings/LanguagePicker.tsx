import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, Languages } from "lucide-react";
import type { EngineId, LanguageInfo } from "../api";
import { t, useT } from "../i18n";

/** Short summary of the selection on the button: "Language: auto", "Polish", "Polish + English", "Polish + 2". */
export function languagesLabel(codes: string[], all: LanguageInfo[]): string {
  const name = (c: string) => all.find((l) => l.code === c)?.name ?? c;
  if (codes.length === 0) return t("lang_picker.auto_label");
  if (codes.length <= 2) return codes.map(name).join(" + ");
  return `${name(codes[0])} + ${codes.length - 1}`;
}

/**
 * Whether the selected model can transcribe in these languages (mirrors `Engine::check_languages`).
 * `error` blocks transcription, `warning` always warns, `hint` only explains the choice
 * (shown only once the user has picked languages themselves).
 */
export function languageCheck(engine: EngineId | null, codes: string[], all: LanguageInfo[]): { error?: string; warning?: string; hint?: string } {
  if (!engine || engine.startsWith("whisper")) {
    return codes.length > 1 ? { hint: t("lang_check.whisper_multi") } : {};
  }
  const model = engine === "canary_v2" ? "Canary" : "Parakeet";
  if (engine === "canary_v2" && codes.length > 1) {
    return { error: t("lang_check.canary_multi") };
  }
  if (engine === "canary_v2" && !codes.length) {
    return { warning: t("lang_check.canary_none") };
  }
  const unknown = codes.filter((c) => !all.find((l) => l.code === c)?.european);
  if (unknown.length) {
    const names = unknown.map((c) => all.find((l) => l.code === c)?.name ?? c).join(", ");
    return { error: t("lang_check.unsupported", { model, names }) };
  }
  if (engine === "parakeet_v3" && codes.length) {
    return { hint: t("lang_check.parakeet") };
  }
  return {};
}

/** Transcription language picker: none (the model detects it), one, or several (mixed-language conversation). */
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
  const { t } = useT();
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
        title={t("lang_picker.title", { languages: languagesLabel(value, languages) })}
        onClick={() => {
          setQuery("");
          setOpen((o) => !o);
        }}
      >
        <Languages size={14} /> <span className="label">{languagesLabel(value, languages)}</span> <ChevronDown size={12} />
      </button>
      {open && (
        <div className="lang-pop">
          <input autoFocus placeholder={t("lang_picker.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
          <div className="lang-list" role="listbox" aria-multiselectable="true">
            {!query && (
              <button role="option" aria-selected={value.length === 0} className={value.length === 0 ? "on" : ""} onClick={() => onChange([])}>
                <span className="tick">{value.length === 0 && <Check size={13} />}</span> {t("lang_picker.auto")}
              </button>
            )}
            {shown.map((l) => {
              const on = value.includes(l.code);
              return (
                <button key={l.code} role="option" aria-selected={on} className={on ? "on" : ""} onClick={() => toggle(l.code)}>
                  <span className="tick">{on && <Check size={13} />}</span> {l.name}
                  {!l.european && <span className="lang-note">{t("lang_picker.whisper_only")}</span>}
                </button>
              );
            })}
            {shown.length === 0 && <div className="lang-empty">{t("lang_picker.empty")}</div>}
          </div>
          <div className="lang-foot">{t("lang_picker.foot")}</div>
        </div>
      )}
    </div>
  );
}
