import { invoke } from "@tauri-apps/api/core";
import { intlLocale, t, type PlainKey } from "./i18n";

export type EngineId = "parakeet_v3" | "canary_v2" | "whisper_turbo" | "whisper_large_v3";
export type Language = "pl" | "en" | "auto";
export type PasteMode = "auto" | "always" | "clipboard_only";
export type ProviderId = "openai" | "openrouter" | "anthropic" | "zai";

export interface ProviderConfig {
  model: string;
  base_url: string;
}

export interface Settings {
  engine: EngineId;
  /** Dictation language. */
  language: Language;
  input_device: string | null;
  shortcut_push_to_talk: string;
  shortcut_toggle: string;
  modifier_push_to_talk: string;
  paste_mode: PasteMode;
  hud_enabled: boolean;
  /** Save every dictation (text and recording) in the history. */
  dictation_history: boolean;
  /** Model for transcription after recording and for imports. */
  meeting_engine: EngineId;
  /** Live transcription model. */
  meeting_live_engine: EngineId;
  /** Vocabulary of names and terms — a prompt hint for Whisper (dictation and meetings). */
  vocabulary: string;
  /** Meetings language (independent of dictation). */
  meeting_language: Language;
  meeting_live_transcription: boolean;
  meeting_live_translate_to: string;
  meeting_live_window: boolean;
  meeting_diarization: boolean;
  meeting_auto_transcribe: boolean;
  meeting_auto_summarize: boolean;
  meeting_audio_retention_days: number;
  meeting_detection_prompt: boolean;
  meeting_consent_reminder: boolean;
  shortcut_meeting: string;
  ai_provider: ProviderId;
  ai_providers: Partial<Record<ProviderId, ProviderConfig>>;
  ai_prompt: string;
  /** UI language: `system` follows the OS (Polish → pl, otherwise en). Independent of the dictation language. */
  ui_language: "system" | "en" | "pl";
}

export type AssetId = { kind: "engine"; engine: EngineId } | { kind: "silero_vad" };

export interface ModelInfo {
  id: AssetId;
  key: string;
  title: string;
  description: string;
  size: number;
  installed: boolean;
  downloading: boolean;
}

export interface DownloadEvent {
  key: string;
  done: number;
  total: number;
  finished: boolean;
  error: string | null;
}

export interface Environment {
  os: "macos" | "windows" | "linux" | string;
  wayland: boolean;
  can_send_keys: boolean;
  hotkey_warnings: string[];
  data_dir: string;
}

export type HudState =
  | { phase: "idle" }
  | { phase: "recording"; level: number; seconds: number }
  | { phase: "transcribing" }
  | { phase: "done"; text: string; pasted: boolean }
  | { phase: "error"; message: string };

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  /** Resolved UI locale (backend is the source of truth for `ui_language: "system"`). */
  uiLocale: () => invoke<"en" | "pl">("ui_locale"),
  languages: () => invoke<LanguageInfo[]>("list_languages"),
  /** Reveals the app log file in the file manager. */
  revealLogs: () => invoke<void>("reveal_logs"),
  saveSettings: (settings: Settings) => invoke<string[]>("save_settings", { settings }),
  listInputDevices: () => invoke<{ devices: string[]; default: string | null }>("list_input_devices"),
  listModels: () => invoke<ModelInfo[]>("list_models"),
  downloadModel: (id: AssetId) => invoke<void>("download_model", { id }),
  cancelDownload: (id: AssetId) => invoke<void>("cancel_download", { id }),
  deleteModel: (id: AssetId) => invoke<void>("delete_model", { id }),
  environment: () => invoke<Environment>("environment"),
  openAccessibilitySettings: () => invoke<string[]>("open_accessibility_settings"),
  reloadHotkeys: () => invoke<string[]>("reload_hotkeys"),
  pauseHotkeys: () => invoke<void>("pause_hotkeys"),
  autostartEnabled: () => invoke<boolean>("autostart_enabled"),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  installUpdate: () => invoke<void>("install_update"),
  knownUpdate: () => invoke<UpdateInfo | null>("known_update"),
};

export interface UpdateInfo {
  version: string;
  current_version: string;
  /** Release notes (Markdown from the GitHub Release). */
  notes: string | null;
  date: string | null;
}

export interface UpdateProgress {
  done: number;
  total: number;
  finished: boolean;
  error: string | null;
}

export function formatBytes(n: number): string {
  const fmt = (v: number, digits: number) =>
    new Intl.NumberFormat(intlLocale(), { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(v);
  if (n >= 1e9) return `${fmt(n / 1e9, 1)} GB`;
  if (n >= 1e6) return `${fmt(Math.round(n / 1e6), 0)} MB`;
  return `${fmt(Math.round(n / 1e3), 0)} kB`;
}

/** Live translation languages (Canary 1B v2 translates between English and the others). Labels: `translationTargetLabel`. */
export const TRANSLATION_TARGETS: string[] = ["", "en", "pl", "de", "fr", "es", "it", "uk", "cs", "pt", "nl", "sv"];

export function translationTargetLabel(code: string): string {
  return t((code ? `lang.${code}` : "lang.none") as PlainKey);
}

export const ENGINE_LABELS: Record<EngineId, string> = {
  parakeet_v3: "Parakeet TDT 0.6B v3",
  canary_v2: "Canary 1B v2",
  whisper_turbo: "Whisper large-v3-turbo",
  whisper_large_v3: "Whisper large-v3",
};

// MARK: - Meetings and AI

export type MeetingState =
  | "recording"
  | "importing"
  | "interrupted"
  | "recorded"
  | "transcribing"
  | "transcribed"
  | "summarizing"
  | "summarized"
  | "failed";

export interface Meeting {
  id: string;
  startedAt: string;
  endedAt: string | null;
  durationSeconds: number;
  state: MeetingState;
  hasSystemAudio: boolean;
  title: string | null;
  audioDeleted: boolean;
  transcriptEngine: string | null;
  /** Languages of the last transcription (empty = detected by the model); `null` = predates this option. */
  transcriptLanguages: string[] | null;
  lastError: string | null;
  /** Gaps in the recording: the track's audio dropped out and was resumed. */
  audioGaps: AudioGap[];
}

export interface AudioGap {
  track: AudioTrack;
  /** Seconds since the start of the recording. */
  start: number;
  seconds: number;
}

export interface LanguageInfo {
  code: string;
  name: string;
  /** Also supported by Parakeet and Canary (otherwise Whisper only). */
  european: boolean;
}

export interface MeetingDetail {
  meeting: Meeting;
  transcript: string | null;
  summaries: { name: string; content: string }[];
  folder: string;
  /** Recorded tracks available for download: `mic` = you, `system` = the other participants. */
  tracks: AudioTrack[];
}

export type AudioTrack = "mic" | "system";
/** What to download: a single track or both mixed together (`mixed`). */
export type AudioExport = AudioTrack | "mixed";

export interface RecordingStatus {
  recording: boolean;
  meeting_id: string | null;
  seconds: number;
  has_system_audio: boolean;
  warning: string | null;
  mic_level: number;
  system_level: number;
}

export interface Utterance {
  start: number;
  end: number;
  track: "mic" | "system";
  text: string;
  speaker: string;
  translation?: string | null;
}

export interface LivePayload {
  meeting_id: string;
  utterance: Utterance | null;
  /** Draft text of the ongoing utterance on a given track; empty text = remove the draft. */
  partial: Utterance | null;
  error: string | null;
}

/** Draft texts of ongoing utterances, one per track. */
export type LiveDrafts = Partial<Record<Utterance["track"], Utterance>>;

/** New draft-text state after a `meeting-live` event: a finalized utterance replaces
 *  its track's draft, a draft with empty text removes it. */
export function nextDrafts(prev: LiveDrafts, { utterance, partial }: LivePayload): LiveDrafts {
  const done = utterance ?? (partial && !partial.text ? partial : null);
  if (done) {
    const { [done.track]: _, ...rest } = prev;
    return rest;
  }
  return partial ? { ...prev, [partial.track]: partial } : prev;
}

/** Drafts in chronological order — to display after the finalized utterances. */
export function draftList(drafts: LiveDrafts): Utterance[] {
  return Object.values(drafts)
    .filter((u): u is Utterance => !!u)
    .sort((a, b) => a.start - b.start);
}

export interface JobEvent {
  meeting_id: string;
  kind: "import" | "transcribe" | "summarize";
  step: string;
  fraction: number;
  finished: boolean;
  error: string | null;
}

export interface ProviderInfo {
  id: ProviderId;
  name: string;
  has_key: boolean;
  default_base_url: string;
  default_model: string;
  model_placeholder: string;
}

/** A dictation saved in the history. */
export interface DictationEntry {
  id: string;
  createdAt: string;
  durationSeconds: number;
  /** Text after corrections — what ended up in the field. */
  text: string;
  /** Model output before corrections. */
  raw: string;
  engine: string;
  languages: string[];
  /** Pasted into the active field (otherwise only copied to the clipboard). */
  pasted: boolean;
  audioDeleted: boolean;
}

export const dictationsApi = {
  list: () => invoke<DictationEntry[]>("list_dictations"),
  remove: (id: string) => invoke<void>("delete_dictation", { id }),
  /** Save dialog for the recording; file path or `null` = cancelled. */
  exportAudio: (id: string) => invoke<string | null>("export_dictation_audio", { id }),
  retranscribe: (id: string, engine: EngineId, languages: string[]) =>
    invoke<DictationEntry>("retranscribe_dictation", { id, engine, languages }),
};

export const meetingsApi = {
  status: () => invoke<RecordingStatus>("meeting_status"),
  liveTranscript: () => invoke<Utterance[]>("live_transcript"),
  hideLiveWindow: () => invoke<void>("hide_live_window"),
  start: () => invoke<Meeting>("start_meeting"),
  stop: () => invoke<Meeting>("stop_meeting"),
  list: () => invoke<Meeting[]>("list_meetings"),
  get: (id: string) => invoke<MeetingDetail>("get_meeting", { id }),
  rename: (id: string, title: string) => invoke<void>("rename_meeting", { id, title }),
  remove: (id: string) => invoke<void>("delete_meeting", { id }),
  reveal: (id: string) => invoke<void>("reveal_meeting", { id }),
  /** Save dialog and export of a track (or both mixed) to WAV; returns the file path, `null` = cancelled. */
  exportAudio: (id: string, track: AudioExport) => invoke<string | null>("export_meeting_audio", { id, track }),
  /** File picker for a recording; `null` = cancelled. Loading and transcription run in the background. */
  importFile: () => invoke<Meeting | null>("import_meeting"),
  /** `languages`: language codes (empty = the model detects them, several = mixed-language conversation); absent = from settings. */
  transcribe: (id: string, engine?: EngineId, languages?: string[]) =>
    invoke<void>("transcribe_meeting", { id, engine: engine ?? null, languages: languages ?? null }),
  summarize: (id: string, provider: ProviderId, model?: string) =>
    invoke<void>("summarize_meeting", { id, provider, model: model ?? null }),
  cancelJob: () => invoke<void>("cancel_job"),
  job: () => invoke<JobEvent | null>("job_status"),
  open: () => invoke<void>("open_meetings"),
};

export const aiApi = {
  providers: () => invoke<ProviderInfo[]>("ai_providers"),
  setKey: (provider: ProviderId, key: string) => invoke<void>("set_ai_key", { provider, key }),
  test: (provider: ProviderId) => invoke<string[]>("test_ai", { provider }),
  defaultPrompt: () => invoke<string>("default_summary_prompt"),
  importLegacy: () => invoke<string[]>("import_legacy_keys"),
};

export function stateLabel(state: MeetingState): string {
  return t(`meeting.state.${state}`);
}

export function shortDate(iso: string): string {
  return new Intl.DateTimeFormat(intlLocale(), {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(iso));
}

/** Backend cancellation sentinel (`"Przerwano"`; also accepts an English rendering). Not an error to show. */
export function isCancelled(e: unknown): boolean {
  return ["Przerwano", "Cancelled", "Canceled"].includes(String(e).trim());
}

export function clock(seconds: number): string {
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}
