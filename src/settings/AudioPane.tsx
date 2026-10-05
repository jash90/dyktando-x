import { useEffect, useState } from "react";
import type { PaneProps } from "./SettingsApp";
import { api } from "../api";

export default function AudioPane({ settings, update }: PaneProps) {
  const [devices, setDevices] = useState<string[]>([]);
  const [def, setDef] = useState<string | null>(null);

  const refresh = () =>
    api.listInputDevices().then((d) => {
      setDevices(d.devices);
      setDef(d.default);
    });

  useEffect(() => {
    refresh();
  }, []);

  const selected = settings.input_device ?? "";
  const missing = selected !== "" && !devices.includes(selected);

  return (
    <section>
      <h1>Audio</h1>
      <div className="row">
        <label>Mikrofon</label>
        <select value={selected} onChange={(e) => update({ input_device: e.target.value || null })}>
          <option value="">Domyślny systemowy{def ? ` (${def})` : ""}</option>
          {devices.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
          {missing && <option value={selected}>{selected} (niepodłączony)</option>}
        </select>
        <button className="link" onClick={refresh}>
          Odśwież listę
        </button>
        <p className="hint">Gdy wybrany mikrofon jest odłączony, nagrywa domyślny — wybór zostaje zapamiętany.</p>
      </div>
    </section>
  );
}
