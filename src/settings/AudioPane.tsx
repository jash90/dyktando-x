import { useEffect, useState } from "react";
import type { PaneProps } from "./SettingsApp";
import { api } from "../api";
import { useT } from "../i18n";

export default function AudioPane({ settings, update }: PaneProps) {
  const { t } = useT();
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
      <h1>{t("audio.title")}</h1>
      <div className="row">
        <label>{t("audio.microphone")}</label>
        <select value={selected} onChange={(e) => update({ input_device: e.target.value || null })}>
          <option value="">{def ? t("audio.system_default_named", { name: def }) : t("audio.system_default")}</option>
          {devices.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
          {missing && <option value={selected}>{t("audio.disconnected", { name: selected })}</option>}
        </select>
        <button className="link" onClick={refresh}>
          {t("audio.refresh")}
        </button>
        <p className="hint">{t("audio.hint")}</p>
      </div>
    </section>
  );
}
