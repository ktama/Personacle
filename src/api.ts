import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConnectionTestResult,
  Diary,
  GenerationFailedPayload,
  Memory,
  MoodState,
  Persona,
  PersonaDetail,
  PersonaInput,
  PersonalityEvent,
  PostprocessCompletedPayload,
  RelationshipGraph,
  Series,
  Session,
  SessionStatusPayload,
  Settings,
  SpeakerSelectingPayload,
  TraitValue,
  Utterance,
  UtteranceCompletedPayload,
  UtteranceDeltaPayload,
  UtteranceStartedPayload,
} from "./types";

// ---------- コマンド (設計6.1) ----------

export const api = {
  listPersonas: () => invoke<Persona[]>("list_personas"),
  getPersona: (id: string) => invoke<PersonaDetail>("get_persona", { id }),
  createPersona: (input: PersonaInput) => invoke<Persona>("create_persona", { input }),
  updatePersona: (id: string, input: PersonaInput) => invoke<void>("update_persona", { id, input }),
  deletePersona: (id: string) => invoke<void>("delete_persona", { id }),
  suggestTraits: (description: string) => invoke<TraitValue[]>("suggest_traits", { description }),

  startSession: (kind: string, personaIds: string[], theme?: string) =>
    invoke<Session>("start_session", { kind, personaIds, theme: theme ?? "" }),
  sendMessage: (sessionId: string, text: string, targetPersonaId?: string) =>
    invoke<Utterance>("send_message", { sessionId, text, targetPersonaId: targetPersonaId ?? null }),
  requestGreeting: (sessionId: string) => invoke<boolean>("request_greeting", { sessionId }),
  cancelGeneration: (sessionId: string) => invoke<void>("cancel_generation", { sessionId }),
  endSession: (sessionId: string) => invoke<void>("end_session", { sessionId }),
  startAutonomousTurns: (sessionId: string) =>
    invoke<void>("start_autonomous_turns", { sessionId }),
  stopSession: (sessionId: string) => invoke<void>("stop_session", { sessionId }),
  listSessions: (personaId: string) => invoke<Session[]>("list_sessions", { personaId }),
  getSessionUtterances: (sessionId: string) =>
    invoke<Utterance[]>("get_session_utterances", { sessionId }),

  listMemories: (personaId: string, includeArchived: boolean) =>
    invoke<Memory[]>("list_memories", { personaId, includeArchived }),
  searchMemories: (personaId: string, query: string, kinds: string[], includeArchived: boolean) =>
    invoke<Memory[]>("search_memories", { personaId, query, kinds, includeArchived }),
  getMemorySources: (memoryId: string) => invoke<Memory[]>("get_memory_sources", { memoryId }),
  updateMemory: (id: string, content: string) => invoke<void>("update_memory", { id, content }),
  deleteMemory: (id: string, restoreSources?: boolean) =>
    invoke<void>("delete_memory", { id, restoreSources: restoreSources ?? false }),

  getPersonalityHistory: (personaId: string) =>
    invoke<PersonalityEvent[]>("get_personality_history", { personaId }),
  getMood: (personaId: string) => invoke<MoodState>("get_mood", { personaId }),
  listDiaries: (personaId: string) => invoke<Diary[]>("list_diaries", { personaId }),
  getTraitSeries: (personaId: string) => invoke<Series[]>("get_trait_series", { personaId }),
  getIntimacySeries: (personaId: string, targetName: string) =>
    invoke<Series>("get_intimacy_series", { personaId, targetName }),
  getRelationshipGraph: () => invoke<RelationshipGraph>("get_relationship_graph"),

  exportPersona: (personaId: string, includeHistory: boolean, path: string) =>
    invoke<{ memoryCount: number; sessionCount: number }>("export_persona", {
      personaId,
      includeHistory,
      path,
    }),
  importPersona: (path: string, force: boolean) =>
    invoke<Persona>("import_persona", { path, force }),

  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (settings: Settings) => invoke<void>("update_settings", { settings }),
  testConnection: () => invoke<ConnectionTestResult>("test_connection"),
};

// ---------- イベント (設計6.2) ----------

export interface EventHandlers {
  onUtteranceStarted?: (p: UtteranceStartedPayload) => void;
  onUtteranceDelta?: (p: UtteranceDeltaPayload) => void;
  onUtteranceCompleted?: (p: UtteranceCompletedPayload) => void;
  onGenerationFailed?: (p: GenerationFailedPayload) => void;
  onSessionStatusChanged?: (p: SessionStatusPayload) => void;
  onPostprocessCompleted?: (p: PostprocessCompletedPayload) => void;
  onSpeakerSelecting?: (p: SpeakerSelectingPayload) => void;
}

export async function subscribeEvents(h: EventHandlers): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];
  if (h.onUtteranceStarted)
    unlisteners.push(await listen("utterance_started", (e) => h.onUtteranceStarted!(e.payload as UtteranceStartedPayload)));
  if (h.onUtteranceDelta)
    unlisteners.push(await listen("utterance_delta", (e) => h.onUtteranceDelta!(e.payload as UtteranceDeltaPayload)));
  if (h.onUtteranceCompleted)
    unlisteners.push(await listen("utterance_completed", (e) => h.onUtteranceCompleted!(e.payload as UtteranceCompletedPayload)));
  if (h.onGenerationFailed)
    unlisteners.push(await listen("generation_failed", (e) => h.onGenerationFailed!(e.payload as GenerationFailedPayload)));
  if (h.onSessionStatusChanged)
    unlisteners.push(await listen("session_status_changed", (e) => h.onSessionStatusChanged!(e.payload as SessionStatusPayload)));
  if (h.onPostprocessCompleted)
    unlisteners.push(await listen("postprocess_completed", (e) => h.onPostprocessCompleted!(e.payload as PostprocessCompletedPayload)));
  if (h.onSpeakerSelecting)
    unlisteners.push(await listen("speaker_selecting", (e) => h.onSpeakerSelecting!(e.payload as SpeakerSelectingPayload)));
  return () => unlisteners.forEach((u) => u());
}
