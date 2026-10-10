import { useEffect, useRef, useState } from "react";
import { api } from "../api";

/** Converts a keyboard event to handy-keys notation, e.g. "Ctrl+Alt+R", "F5". */
export function eventToShortcut(e: KeyboardEvent, mac: boolean): string | null {
  const code = e.code;
  if (/^(Meta|Control|Alt|Shift)(Left|Right)$/.test(code) || code === "Fn") return null;
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit\d$/.test(code)) key = code.slice(5);
  else if (/^F\d{1,2}$/.test(code)) key = code;
  else {
    const named: Record<string, string> = {
      Space: "Space", Enter: "Enter", Tab: "Tab", Escape: "Escape", Backspace: "Backspace",
      ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
      Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown", Delete: "Delete", Insert: "Insert",
      Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Semicolon: ";", Quote: "'",
      Comma: ",", Period: ".", Slash: "/", Backslash: "\\", Backquote: "`",
    };
    key = named[code] ?? null;
  }
  if (!key) return null;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push(mac ? "Opt" : "Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push(mac ? "Cmd" : "Super");
  return [...mods, key].join("+");
}

export function prettyShortcut(s: string, mac: boolean): string {
  if (!s) return "brak";
  if (!mac) return s.replace(/\+/g, " + ");
  const map: Record<string, string> = { Ctrl: "⌃", Opt: "⌥", Alt: "⌥", Shift: "⇧", Cmd: "⌘" };
  return s
    .split("+")
    .map((p) => map[p] ?? p)
    .join("");
}

export default function ShortcutRecorder({
  value,
  onChange,
  mac,
  allowEmpty = true,
}: {
  value: string;
  onChange: (v: string) => void;
  mac: boolean;
  allowEmpty?: boolean;
}) {
  const [recording, setRecording] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!recording) return;
    api.pauseHotkeys();
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.code === "Escape" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        setRecording(false);
        return;
      }
      const s = eventToShortcut(e, mac);
      if (s) {
        onChange(s);
        setRecording(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      api.reloadHotkeys();
    };
  }, [recording, mac, onChange]);

  return (
    <span className="shortcut">
      <button ref={ref} className={`key ${recording ? "recording" : ""}`} onClick={() => setRecording((r) => !r)}>
        {recording ? "Naciśnij skrót… (Esc anuluje)" : prettyShortcut(value, mac)}
      </button>
      {allowEmpty && value && !recording && (
        <button className="link" onClick={() => onChange("")}>
          Wyłącz
        </button>
      )}
    </span>
  );
}
