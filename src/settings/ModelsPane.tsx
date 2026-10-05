import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { PaneProps } from "./SettingsApp";
import { api, formatBytes, type DownloadEvent, type ModelInfo } from "../api";

export default function ModelsPane({ settings, update }: PaneProps) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  const refresh = useCallback(() => api.listModels().then(setModels), []);

  useEffect(() => {
    refresh();
    const un = listen<DownloadEvent>("model-download", (e) => {
      const ev = e.payload;
      if (ev.finished) {
        setProgress((p) => {
          const rest = { ...p };
          delete rest[ev.key];
          return rest;
        });
        if (ev.error) setErrors((x) => ({ ...x, [ev.key]: ev.error ?? "" }));
        refresh();
      } else if (ev.total > 0) {
        setProgress((p) => ({ ...p, [ev.key]: ev.done / ev.total }));
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  const download = (m: ModelInfo) => {
    setErrors((x) => ({ ...x, [m.key]: "" }));
    setProgress((p) => ({ ...p, [m.key]: 0 }));
    api.downloadModel(m.id).catch(() => {});
    refresh();
  };

  const remove = (m: ModelInfo) => {
    if (!confirm(`Usunąć ${m.title} (${formatBytes(m.size)})?`)) return;
    api.deleteModel(m.id).then(refresh);
  };

  return (
    <section>
      <h1>Modele</h1>
      <p className="hint">Modele działają lokalnie — nagrania nie opuszczają komputera.</p>
      {models.map((m) => {
        const p = progress[m.key];
        const busy = m.downloading || p !== undefined;
        const engine = m.id.kind === "engine" ? m.id.engine : null;
        return (
          <div key={m.key} className="card">
            <div className="card-head">
              <div>
                <strong>{m.title}</strong>
                {engine && settings.engine === engine && <span className="badge">dyktowanie</span>}
                {engine && settings.meeting_engine === engine && <span className="badge">spotkania</span>}
                <div className="hint">
                  {m.description} · {formatBytes(m.size)}
                </div>
              </div>
              <div className="actions">
                {m.installed && engine && settings.engine !== engine && <button onClick={() => update({ engine })}>Używaj</button>}
                {m.installed && !busy && (
                  <button className="danger" onClick={() => remove(m)}>
                    Usuń
                  </button>
                )}
                {!m.installed && !busy && (
                  <button className="primary" onClick={() => download(m)}>
                    Pobierz
                  </button>
                )}
                {busy && <button onClick={() => api.cancelDownload(m.id)}>Przerwij</button>}
              </div>
            </div>
            {busy && (
              <div className="progress">
                <span style={{ width: `${Math.round((p ?? 0) * 100)}%` }} />
                <em>{Math.round((p ?? 0) * 100)}%</em>
              </div>
            )}
            {errors[m.key] && <div className="error">{errors[m.key]}</div>}
          </div>
        );
      })}
    </section>
  );
}
