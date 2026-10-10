import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Check, Circle, LoaderCircle, type LucideIcon } from "lucide-react";
import type { HudState } from "./api";
import { useT } from "./i18n";

function clock(seconds: number) {
  const s = Math.floor(seconds);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export default function Hud() {
  const { t } = useT();
  const [state, setState] = useState<HudState>({ phase: "idle" });

  useEffect(() => {
    const un = listen<HudState>("hud-state", (e) => setState(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);

  let Icon: LucideIcon = Circle;
  let text = "";
  let level = 0;
  switch (state.phase) {
    case "recording":
      Icon = Circle;
      text = t("hud.listening", { time: clock(state.seconds) });
      level = Math.min(1, state.level * 8);
      break;
    case "transcribing":
      Icon = LoaderCircle;
      text = t("hud.transcribing");
      break;
    case "done":
      Icon = Check;
      text = state.pasted ? t("hud.pasted") : t("hud.copied");
      break;
    case "error":
      Icon = AlertTriangle;
      text = state.message;
      break;
    default:
      return null;
  }

  return (
    <div className={`hud hud-${state.phase}`}>
      <Icon className="hud-icon" size={18} strokeWidth={2.5} aria-hidden />
      <span className="hud-text">{text}</span>
      {state.phase === "recording" && (
        <span className="hud-meter">
          <span style={{ transform: `scaleX(${Math.max(0.04, level)})` }} />
        </span>
      )}
    </div>
  );
}
