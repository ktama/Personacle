import { api } from "../api";
import type { Persona, Session } from "../types";
import { errorMessage } from "../types";
import { clear, el, errorBanner, toast } from "../ui";

const MIN_PARTICIPANTS = 2;
const MAX_PARTICIPANTS = 6; // ADR-15

export interface GroupController {
  root: HTMLElement;
  onUtteranceStarted(utteranceId: string, speakerName: string): void;
  onUtteranceDelta(utteranceId: string, delta: string): void;
  onUtteranceCompleted(utteranceId: string, state: string): void;
  onGenerationFailed(message: string, kind: string): void;
  onSpeakerSelecting(): void;
  sessionId(): string | null;
  dispose(): Promise<void>;
}

export interface GroupOptions {
  personas: Persona[];
  openSettings: () => void;
}

/// グループチャット画面 (FR-31〜34, EC-19)
export function groupView(opts: GroupOptions): GroupController {
  let session: Session | null = null;
  let participants: Persona[] = [];
  let selecting: HTMLElement | null = null;

  // 参加ペルソナ選択
  const checks = opts.personas.map((p) => ({ persona: p, box: el("input", { type: "checkbox" }) }));
  checks.slice(0, 2).forEach((c) => (c.box.checked = true));
  const checkList = el(
    "div",
    { class: "auto-checks" },
    checks.map((c) => el("label", { class: "auto-check" }, [c.box, c.persona.name])),
  );

  const messages = el("div", { class: "chat-messages" });
  const banner = el("div", {});
  const input = el("textarea", { class: "input chat-input", rows: "2", placeholder: "みんなにメッセージを送る (Ctrl+Enter)" });
  const targetSelect = el("select", { class: "input group-target" });
  const sendBtn = el("button", { class: "btn btn-primary", text: "送信" });
  const startBtn = el("button", { class: "btn btn-primary", text: "グループを始める" });
  const endBtn = el("button", { class: "btn btn-small", text: "会話を終える" });
  endBtn.style.display = "none";

  const setup = el("div", { class: "auto-controls" }, [
    el("h2", { text: "グループチャット" }),
    el("p", { class: "muted", text: `${MIN_PARTICIPANTS}〜${MAX_PARTICIPANTS}体のペルソナとユーザーで会話します` }),
    checkList,
    el("div", { class: "auto-row" }, [startBtn]),
  ]);

  const inputRow = el("div", { class: "chat-input-row" }, [
    input,
    el("div", { class: "chat-input-side" }, [targetSelect, sendBtn]),
  ]);
  inputRow.style.display = "none";

  const bubbleFor = (speakerKind: string, speakerName: string, content: string, id?: string) => {
    const cls = speakerKind === "user" ? "bubble-user" : speakerKind === "system" ? "bubble-system" : "bubble-persona";
    const bubble = el("div", { class: `bubble ${cls}` }, [
      el("div", { class: "bubble-name", text: speakerName }),
      el("div", { class: "bubble-content", text: content }),
    ]);
    if (id) bubble.dataset.utteranceId = id;
    return bubble;
  };
  const scrollToBottom = () => (messages.scrollTop = messages.scrollHeight);

  const rebuildTargets = () => {
    clear(targetSelect);
    targetSelect.append(el("option", { value: "" }, ["宛先: おまかせ"]));
    for (const p of participants) {
      targetSelect.append(el("option", { value: p.id }, [`@${p.name}`]));
    }
  };

  startBtn.addEventListener("click", async () => {
    participants = checks.filter((c) => c.box.checked).map((c) => c.persona);
    if (participants.length < MIN_PARTICIPANTS || participants.length > MAX_PARTICIPANTS) {
      toast(`${MIN_PARTICIPANTS}〜${MAX_PARTICIPANTS}体を選んでください`, "error");
      return;
    }
    try {
      session = await api.startSession("group", participants.map((p) => p.id));
      rebuildTargets();
      setup.style.display = "none";
      inputRow.style.display = "";
      endBtn.style.display = "";
    } catch (e) {
      toast(errorMessage(e), "error"); // EC-19 の busy もここ
    }
  });

  const send = async () => {
    if (!session) return;
    const text = input.value;
    if (!text.trim()) return;
    clear(banner);
    try {
      const target = targetSelect.value || undefined;
      const preview = await api.sendMessage(session.id, text, target);
      messages.append(bubbleFor("user", preview.speakerName, preview.content));
      scrollToBottom();
      input.value = "";
    } catch (e) {
      banner.append(errorBanner(errorMessage(e), opts.openSettings));
    }
  };
  sendBtn.addEventListener("click", () => void send());
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.ctrlKey) {
      e.preventDefault();
      void send();
    }
  });
  endBtn.addEventListener("click", async () => {
    if (!session) return;
    try {
      await api.endSession(session.id);
      toast("会話を終えました。記憶の整理を始めます");
      session = null;
      endBtn.style.display = "none";
      inputRow.style.display = "none";
      setup.style.display = "";
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  });

  const clearSelecting = () => {
    selecting?.remove();
    selecting = null;
  };

  const root = el("div", { class: "chat" }, [
    el("div", { class: "chat-header" }, [el("h2", { text: "グループチャット" }), endBtn]),
    setup,
    messages,
    banner,
    inputRow,
  ]);

  return {
    root,
    sessionId: () => session?.id ?? null,
    onUtteranceStarted(utteranceId, speakerName) {
      clearSelecting();
      // system(司会) は speakerId ではなく名前で判別する
      const kind = speakerName === "司会" ? "system" : "persona";
      messages.append(bubbleFor(kind, speakerName, "", utteranceId));
      scrollToBottom();
    },
    onUtteranceDelta(utteranceId, delta) {
      const content = messages.querySelector<HTMLElement>(`[data-utterance-id="${utteranceId}"] .bubble-content`);
      if (content) {
        content.textContent += delta;
        scrollToBottom();
      }
    },
    onUtteranceCompleted() {
      clearSelecting();
    },
    onGenerationFailed(message, kind) {
      clearSelecting();
      clear(banner);
      banner.append(errorBanner(message, kind === "connection" ? opts.openSettings : undefined));
    },
    onSpeakerSelecting() {
      clearSelecting();
      selecting = el("div", { class: "speaker-selecting", text: "…誰が話すか考えています" });
      messages.append(selecting);
      scrollToBottom();
    },
    async dispose() {
      if (session && session.status === "active") {
        try {
          await api.endSession(session.id);
        } catch {
          /* 起動時リカバリで回収 (EC-03) */
        }
      }
    },
  };
}
