import type { PaneProps } from "./SettingsApp";
import { api, ENGINE_LABELS, meetingsApi, TRANSLATION_TARGETS, translationTargetLabel, type EngineId, type Language } from "../api";
import { useT } from "../i18n";
import ShortcutRecorder from "../components/ShortcutRecorder";

export default function MeetingsPane({ settings, update, env }: PaneProps) {
  const { t, tn } = useT();
  const mac = env.os === "macos";
  return (
    <section>
      <h1>{t("meetings_settings.title")}</h1>
      <p className="hint">{t("meetings_settings.intro")}</p>
      <div className="row">
        <button className="primary" onClick={() => meetingsApi.open()}>
          {t("meetings_settings.open_window")}
        </button>
      </div>
      <div className="row">
        <label>{t("meetings_settings.shortcut")}</label>
        <ShortcutRecorder value={settings.shortcut_meeting} onChange={(v) => update({ shortcut_meeting: v })} mac={mac} />
      </div>
      <div className="row">
        <label>{t("meetings_settings.engine")}</label>
        <select value={settings.meeting_engine} onChange={(e) => update({ meeting_engine: e.target.value as EngineId })}>
          {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
            <option key={id} value={id}>
              {ENGINE_LABELS[id]}
            </option>
          ))}
        </select>
        <p className="hint">{t("meetings_settings.engine_hint")}</p>
      </div>
      <div className="row">
        <label>{t("meetings_settings.vocabulary")}</label>
        <textarea
          rows={3}
          value={settings.vocabulary}
          placeholder={t("meetings_settings.vocabulary_placeholder")}
          onChange={(e) => update({ vocabulary: e.target.value })}
        />
        <p className="hint">{t("meetings_settings.vocabulary_hint")}</p>
      </div>
      <div className="row">
        <label>{t("meetings_settings.language")}</label>
        <select value={settings.meeting_language} onChange={(e) => update({ meeting_language: e.target.value as Language })}>
          <option value="pl">{t("language_option.pl")}</option>
          <option value="en">{t("language_option.en")}</option>
          <option value="auto">{t("language_option.auto")}</option>
        </select>
        <p className="hint">{t("meetings_settings.language_hint")}</p>
      </div>
      <div className="row check">
        <label>
          <input
            type="checkbox"
            checked={settings.meeting_live_transcription}
            onChange={(e) => update({ meeting_live_transcription: e.target.checked })}
          />
          {t("meetings_settings.live_transcription")}
        </label>
        <label>
          <input type="checkbox" checked={settings.meeting_live_window} onChange={(e) => update({ meeting_live_window: e.target.checked })} />
          {t("meetings_settings.live_window")}
        </label>
      </div>
      <div className="row">
        <label>{t("meetings_settings.live_engine")}</label>
        <select
          value={settings.meeting_live_engine}
          disabled={!settings.meeting_live_transcription}
          onChange={(e) => update({ meeting_live_engine: e.target.value as EngineId })}
        >
          {(Object.keys(ENGINE_LABELS) as EngineId[]).map((id) => (
            <option key={id} value={id}>
              {ENGINE_LABELS[id]}
            </option>
          ))}
        </select>
        <p className="hint">{t("meetings_settings.live_engine_hint")}</p>
      </div>
      <div className="row">
        <label>{t("meetings_settings.translate_to")}</label>
        <select
          value={settings.meeting_live_translate_to}
          disabled={!settings.meeting_live_transcription}
          onChange={(e) => update({ meeting_live_translate_to: e.target.value })}
        >
          {TRANSLATION_TARGETS.map((code) => (
            <option key={code} value={code}>
              {translationTargetLabel(code)}
            </option>
          ))}
        </select>
        <p className="hint">{t("meetings_settings.translate_hint")}</p>
      </div>
      <div className="row check">
        <label>
          <input type="checkbox" checked={settings.meeting_diarization} onChange={(e) => update({ meeting_diarization: e.target.checked })} />
          {t("meetings_settings.diarization")}
        </label>
        <label>
          <input type="checkbox" checked={settings.meeting_auto_transcribe} onChange={(e) => update({ meeting_auto_transcribe: e.target.checked })} />
          {t("meetings_settings.auto_transcribe")}
        </label>
        <label>
          <input
            type="checkbox"
            checked={settings.meeting_auto_summarize}
            disabled={!settings.meeting_auto_transcribe}
            onChange={(e) => update({ meeting_auto_summarize: e.target.checked })}
          />
          {t("meetings_settings.auto_summarize")}
        </label>
        <label>
          <input type="checkbox" checked={settings.meeting_consent_reminder} onChange={(e) => update({ meeting_consent_reminder: e.target.checked })} />
          {t("meetings_settings.consent_reminder")}
        </label>
      </div>
      <div className="row">
        <label>{t("meetings_settings.retention")}</label>
        <select
          value={settings.meeting_audio_retention_days}
          onChange={(e) => update({ meeting_audio_retention_days: Number(e.target.value) })}
        >
          {[7, 14, 30, 60, 90, 0].map((d) => (
            <option key={d} value={d}>
              {d === 0 ? t("meetings_settings.retention_never") : tn("meetings_settings.retention_days", d)}
            </option>
          ))}
        </select>
        <p className="hint">{t("meetings_settings.retention_hint")}</p>
      </div>
      {env.os === "macos" && (
        <p className="hint">{t("meetings_settings.macos_hint")}</p>
      )}
      {env.os === "windows" && <p className="hint">{t("meetings_settings.windows_hint")}</p>}
      {env.os === "linux" && (
        <p className="hint">{t("meetings_settings.linux_hint")}</p>
      )}
      <div className="row">
        <label>{t("meetings_settings.diagnostics")}</label>
        <button onClick={() => api.revealLogs()}>{t("common.show_logs")}</button>
        <p className="hint">{t("meetings_settings.diagnostics_hint")}</p>
      </div>
    </section>
  );
}
