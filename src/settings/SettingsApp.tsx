import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Download, Keyboard, Mic, ShieldCheck, Sparkles, Users, Volume2, type LucideIcon } from "lucide-react";
import { api, type Environment, type Settings } from "../api";
import { setLocale, useT, type PlainKey } from "../i18n";
import DictationPane from "./DictationPane";
import ModelsPane from "./ModelsPane";
import AudioPane from "./AudioPane";
import ShortcutsPane from "./ShortcutsPane";
import SystemPane from "./SystemPane";
import MeetingsPane from "./MeetingsPane";
import AiPane from "./AiPane";

type PaneId = "dictation" | "shortcuts" | "meetings" | "ai" | "models" | "audio" | "system";

const PANES: { id: PaneId; title: PlainKey; icon: LucideIcon }[] = [
  { id: "dictation", title: "settings.nav.dictation", icon: Mic },
  { id: "shortcuts", title: "settings.nav.shortcuts", icon: Keyboard },
  { id: "meetings", title: "settings.nav.meetings", icon: Users },
  { id: "ai", title: "settings.nav.ai", icon: Sparkles },
  { id: "models", title: "settings.nav.models", icon: Download },
  { id: "audio", title: "settings.nav.audio", icon: Volume2 },
  { id: "system", title: "settings.nav.system", icon: ShieldCheck },
];

/** Pane from the URL (`index.html#system`) — this is how the tray opens a new window directly on a given pane. */
function initialPane(): PaneId {
  const hash = window.location.hash.slice(1);
  return PANES.some((p) => p.id === hash) ? (hash as PaneId) : "dictation";
}

export interface PaneProps {
  settings: Settings;
  update: (patch: Partial<Settings>) => void;
  env: Environment;
}

export default function SettingsApp() {
  const { t } = useT();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [env, setEnv] = useState<Environment | null>(null);
  const [pane, setPane] = useState<PaneId>(initialPane);
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
    // Window already open: the tray switches the pane via an event.
    const un = listen<string>("open-pane", (e) => {
      if (PANES.some((p) => p.id === e.payload)) setPane(e.payload as PaneId);
    });
    return () => {
      window.removeEventListener("focus", refreshEnv);
      un.then((f) => f());
    };
  }, [refreshEnv]);

  const update = useCallback((patch: Partial<Settings>) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      api
        .saveSettings(next)
        .then((w) => {
          setWarnings(w);
          // The backend also emits `ui-locale-changed`; ask directly too so this window switches right away.
          if ("ui_language" in patch) api.uiLocale().then(setLocale).catch(() => {});
        })
        .catch((e) => setWarnings([String(e)]));
      return next;
    });
  }, []);

  if (!settings || !env) return <div className="loading">{t("common.loading")}</div>;
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
            {t(p.title)}
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
