// Rust 側 models.rs (camelCase 直列化) と対応する型定義

export interface Persona {
  id: string;
  name: string;
  description: string;
  speechStyle: string;
  valuesText: string;
  selfIntro: string;
  createdAt: number;
  lastTalkedAt: number | null;
}

export interface TraitValue {
  key: string;
  value: number;
}

export interface Relationship {
  personaId: string;
  targetKind: "user" | "persona";
  targetId: string;
  targetName: string;
  intimacy: number;
  impressionText: string;
  updatedAt: number;
}

export interface PersonaDetail {
  persona: Persona;
  traits: TraitValue[];
  relationships: Relationship[];
}

export interface Session {
  id: string;
  kind: "user_dialogue" | "autonomous";
  theme: string;
  status: "active" | "ended" | "processed";
  startedAt: number;
  endedAt: number | null;
  participantIds: string[];
  participantNames: string[];
}

export interface Utterance {
  id: string;
  sessionId: string;
  speakerKind: "user" | "persona";
  speakerId: string;
  speakerName: string;
  content: string;
  state: "complete" | "canceled";
  createdAt: number;
}

export interface Memory {
  id: string;
  personaId: string;
  content: string;
  kind: "fact" | "event" | "promise" | "impression";
  importance: number;
  hasEmbedding: boolean;
  sourceSessionId: string | null;
  createdAt: number;
  archived: boolean;
  userEdited: boolean;
}

export interface PersonalityEvent {
  id: string;
  personaId: string;
  sessionId: string | null;
  item: string;
  oldValue: string;
  newValue: string;
  createdAt: number;
}

export interface Settings {
  endpoint: string;
  chatModel: string;
  embedModel: string;
  autoTurnLimit: number;
  inputMaxChars: number;
  recallK: number;
  wSim: number;
  wRec: number;
  wImp: number;
  traitDeltaCap: number;
  intimacyDeltaCap: number;
  memoryCap: number;
  contextChars: number;
}

export interface ConnectionTestResult {
  connected: boolean;
  models: string[];
  chatModelFound: boolean;
  embedOk: boolean;
  message: string;
}

export interface AppErrorPayload {
  kind:
    | "validation"
    | "connection"
    | "generation"
    | "data"
    | "busy"
    | "not_found"
    | "duplicate_name";
  message: string;
}

export interface PersonaInput {
  name: string;
  description: string;
  speechStyle: string;
  valuesText: string;
  selfIntro: string;
  traits: TraitValue[];
  force: boolean;
}

// イベントペイロード (設計6.2)
export interface UtteranceStartedPayload {
  sessionId: string;
  utteranceId: string;
  speakerId: string;
  speakerName: string;
}
export interface UtteranceDeltaPayload {
  sessionId: string;
  utteranceId: string;
  delta: string;
}
export interface UtteranceCompletedPayload {
  sessionId: string;
  utteranceId: string;
  state: "complete" | "canceled";
}
export interface GenerationFailedPayload {
  sessionId: string;
  kind: string;
  message: string;
}
export interface SessionStatusPayload {
  sessionId: string;
  status: string;
}
export interface PostprocessCompletedPayload {
  sessionId: string;
  memoryCount: number;
  eventCount: number;
}

export const TRAIT_LABELS: Record<string, string> = {
  sociability: "社交性",
  empathy: "共感性",
  caution: "慎重さ",
  assertiveness: "自己主張",
  cheerfulness: "明朗さ",
};

export const MEMORY_KIND_LABELS: Record<string, string> = {
  fact: "事実",
  event: "出来事",
  promise: "約束",
  impression: "感想",
};

export function isAppError(e: unknown): e is AppErrorPayload {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

export function errorMessage(e: unknown): string {
  if (isAppError(e)) return e.message;
  return String(e);
}
