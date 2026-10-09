import { invoke } from "@tauri-apps/api/core";

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
  /** Język dyktowania. */
  language: Language;
  input_device: string | null;
  shortcut_push_to_talk: string;
  shortcut_toggle: string;
  modifier_push_to_talk: string;
  paste_mode: PasteMode;
  hud_enabled: boolean;
  meeting_engine: EngineId;
  /** Język spotkań (niezależny od dyktowania). */
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
};

export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1).replace(".", ",")} GB`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} MB`;
  return `${Math.round(n / 1e3)} kB`;
}

/** Języki tłumaczenia na żywo (Canary 1B v2 tłumaczy między angielskim a pozostałymi). */
export const TRANSLATION_TARGETS: [string, string][] = [
  ["", "nie tłumacz"],
  ["en", "angielski"],
  ["pl", "polski"],
  ["de", "niemiecki"],
  ["fr", "francuski"],
  ["es", "hiszpański"],
  ["it", "włoski"],
  ["uk", "ukraiński"],
  ["cs", "czeski"],
  ["pt", "portugalski"],
  ["nl", "niderlandzki"],
  ["sv", "szwedzki"],
];

export const ENGINE_LABELS: Record<EngineId, string> = {
  parakeet_v3: "Parakeet TDT 0.6B v3",
  canary_v2: "Canary 1B v2",
  whisper_turbo: "Whisper large-v3-turbo",
  whisper_large_v3: "Whisper large-v3",
};

// MARK: - Spotkania i AI

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
  lastError: string | null;
}

export interface MeetingDetail {
  meeting: Meeting;
  transcript: string | null;
  summaries: { name: string; content: string }[];
  folder: string;
  /** Ścieżki z nagraniem do pobrania: `mic` = Ty, `system` = rozmówcy. */
  tracks: AudioTrack[];
}

export type AudioTrack = "mic" | "system";

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
  /** Tekst roboczy trwającej wypowiedzi danej ścieżki; pusty tekst = usuń roboczy. */
  partial: Utterance | null;
  error: string | null;
}

/** Teksty robocze trwających wypowiedzi, po jednym na ścieżkę. */
export type LiveDrafts = Partial<Record<Utterance["track"], Utterance>>;

/** Nowy stan tekstów roboczych po zdarzeniu `meeting-live`: domknięta wypowiedź zastępuje
 *  roboczy swojej ścieżki, roboczy z pustym tekstem go usuwa. */
export function nextDrafts(prev: LiveDrafts, { utterance, partial }: LivePayload): LiveDrafts {
  const done = utterance ?? (partial && !partial.text ? partial : null);
  if (done) {
    const { [done.track]: _, ...rest } = prev;
    return rest;
  }
  return partial ? { ...prev, [partial.track]: partial } : prev;
}

/** Robocze w kolejności czasu — do wyświetlenia za domkniętymi wypowiedziami. */
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
  /** Okno zapisu i eksport jednej ścieżki do WAV; zwraca ścieżkę pliku, `null` = anulowano. */
  exportAudio: (id: string, track: AudioTrack) => invoke<string | null>("export_meeting_audio", { id, track }),
  /** Okno wyboru pliku z nagraniem; `null` = anulowano. Wczytanie i transkrypcja idą w tle. */
  importFile: () => invoke<Meeting | null>("import_meeting"),
  transcribe: (id: string, engine?: EngineId) => invoke<void>("transcribe_meeting", { id, engine: engine ?? null }),
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

export const STATE_LABELS: Record<MeetingState, string> = {
  recording: "nagrywane",
  importing: "wczytywanie…",
  interrupted: "przerwane",
  recorded: "nagrane",
  transcribing: "przepisywanie…",
  transcribed: "przepisane",
  summarizing: "podsumowywanie…",
  summarized: "podsumowane",
  failed: "błąd",
};

const MONTHS = ["sty", "lut", "mar", "kwi", "maj", "cze", "lip", "sie", "wrz", "paź", "lis", "gru"];

export function shortDate(iso: string): string {
  const d = new Date(iso);
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()}, ${hm}`;
}

export function clock(seconds: number): string {
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}
