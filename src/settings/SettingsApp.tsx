import { useCallback, useEffect, useState } from "react";
import { Download, Keyboard, Mic, ShieldCheck, Sparkles, Users, Volume2, type LucideIcon } from "lucide-react";
import { api, type Environment, type Settings } from "../api";
import DictationPane from "./DictationPane";
import ModelsPane from "./ModelsPane";
import AudioPane from "./AudioPane";
import ShortcutsPane from "./ShortcutsPane";
import SystemPane from "./SystemPane";
import MeetingsPane from "./MeetingsPane";
import AiPane from "./AiPane";

type PaneId = "dictation" | "shortcuts" | "meetings" | "ai" | "models" | "audio" | "system";

const PANES: { id: PaneId; title: string; icon: LucideIcon }[] = [
  { id: "dictation", title: "Dyktowanie", icon: Mic },
  { id: "shortcuts", title: "Skróty", icon: Keyboard },
  { id: "meetings", title: "Spotkania", icon: Users },
  { id: "ai", title: "AI", icon: Sparkles },
  { id: "models", title: "Modele", icon: Download },
  { id: "audio", title: "Audio", icon: Volume2 },
  { id: "system", title: "Uprawnienia", icon: ShieldCheck },
];

export interface PaneProps {
  settings: Settings;
  update: (patch: Partial<Settings>) => void;
  env: Environment;
}

export default function SettingsApp() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [env, setEnv] = useState<Environment | null>(null);
  const [pane, setPane] = useState<PaneId>("dictation");
  const [warnings, setWarnings] = useState<string[]>([]);

  const refreshEnv = useCallback(() => {
    api.environment().then((e) => {
      setEnv(e);
      setWarnings(e.hotkey_warnings);
    });
  }, []);

  useEffect(() => {
    api.getSettings().then(setSettings);
    refreshEnv();
    window.addEventListener("focus", refreshEnv);
    return () => window.removeEventListener("focus", refreshEnv);
  }, [refreshEnv]);

  const update = useCallback((patch: Partial<Settings>) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      api
        .saveSettings(next)
        .then(setWarnings)
        .catch((e) => setWarnings([String(e)]));
      return next;
    });
  }, []);

  if (!settings || !env) return <div className="loading">Wczytywanie…</div>;
  const props: PaneProps = { settings, update, env };

  return (
    <div className="layout">
      <nav className="sidebar">
        <div className="brand">
          Dyktando <em>X</em>
        </div>
        {PANES.map((p) => (
          <button key={p.id} className={`nav ${pane === p.id ? "active" : ""}`} onClick={() => setPane(p.id)}>
            <p.icon className="nav-icon" size={16} strokeWidth={2} aria-hidden />
            {p.title}
          </button>
        ))}
      </nav>
      <main className="content">
        {warnings.length > 0 && (
          <div className="banner warn">
            {warnings.map((w) => (
              <div key={w}>{w}</div>
            ))}
          </div>
        )}
        {pane === "dictation" && <DictationPane {...props} />}
        {pane === "shortcuts" && <ShortcutsPane {...props} />}
        {pane === "meetings" && <MeetingsPane {...props} />}
        {pane === "ai" && <AiPane {...props} />}
        {pane === "models" && <ModelsPane {...props} />}
        {pane === "audio" && <AudioPane {...props} />}
        {pane === "system" && <SystemPane {...props} onRefresh={refreshEnv} />}
      </main>
    </div>
  );
}
