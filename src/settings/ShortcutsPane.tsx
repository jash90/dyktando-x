import type { PaneProps } from "./SettingsApp";
import ShortcutRecorder from "../components/ShortcutRecorder";
import { useT } from "../i18n";

export default function ShortcutsPane({ settings, update, env }: PaneProps) {
  const { t } = useT();
  const mac = env.os === "macos";
  const modifiers = mac
    ? [
        ["", t("shortcuts.modifier_off")],
        ["CmdRight", t("shortcuts.right_key", { key: "⌘" })],
        ["OptRight", t("shortcuts.right_key", { key: "⌥" })],
        ["CtrlRight", t("shortcuts.right_key", { key: "⌃" })],
        ["ShiftRight", t("shortcuts.right_key", { key: "⇧" })],
        ["Fn", "fn"],
      ]
    : [
        ["", t("shortcuts.modifier_off")],
        ["CtrlRight", t("shortcuts.right_key", { key: "Ctrl" })],
        ["ShiftRight", t("shortcuts.right_key", { key: "Shift" })],
        ["SuperRight", t("shortcuts.right_key", { key: env.os === "windows" ? "Win" : "Super" })],
      ];
  return (
    <section>
      <h1>{t("shortcuts.title")}</h1>
      <div className="row">
        <label>{t("shortcuts.push_to_talk")}</label>
        <ShortcutRecorder
          value={settings.shortcut_push_to_talk}
          onChange={(v) => update({ shortcut_push_to_talk: v })}
          mac={mac}
        />
        <p className="hint">{t("shortcuts.push_to_talk_hint")}</p>
      </div>
      <div className="row">
        <label>{t("shortcuts.toggle")}</label>
        <ShortcutRecorder value={settings.shortcut_toggle} onChange={(v) => update({ shortcut_toggle: v })} mac={mac} />
      </div>
      <div className="row">
        <label>{t("shortcuts.modifier")}</label>
        <select value={settings.modifier_push_to_talk} onChange={(e) => update({ modifier_push_to_talk: e.target.value })}>
          {modifiers.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <p className="hint">
          {t("shortcuts.modifier_hint", { example: mac ? "⌘C" : "Ctrl+C" })}
          {!mac && ` ${t("shortcuts.altgr_hint")}`}
        </p>
      </div>
      <div className="row">
        <label>{t("shortcuts.meeting")}</label>
        <ShortcutRecorder value={settings.shortcut_meeting} onChange={(v) => update({ shortcut_meeting: v })} mac={mac} />
      </div>
      {env.os === "linux" && (
        <p className="hint">
          {t("shortcuts.linux_hint")}
        </p>
      )}
    </section>
  );
}
