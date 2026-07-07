import { api } from "../api";
import type { Persona, Session, Utterance } from "../types";
import { errorMessage } from "../types";
import { clear, el, errorBanner, toast } from "../ui";

export interface ChatController {
  root: HTMLElement;
  /** ストリーミングイベントの配線先 (main.ts のグローバル購読から呼ばれる) */
  onUtteranceStarted(utteranceId: string, speakerName: string): void;
  onUtteranceDelta(utteranceId: string, delta: string): void;
  onUtteranceCompleted(utteranceId: string, state: string): void;
  onGenerationFailed(message: string, kind: string): void;
  sessionId(): string | null;
  dispose(): Promise<void>;
}

export interface ChatOptions {
  persona: Persona;
  openSettings: () => void;
}

/// 1対1チャット画面 (FR-05/06/07, EC-02/05/09)
export function chatView(opts: ChatOptions): ChatController {
  let session: Session | null = null;
  let generating = false;
  let lastFailedInput = "";

  const messages = el("div", { class: "chat-messages" });
  const banner = el("div", {});
  const input = el("textarea", {
    class: "input chat-input",
    rows: "2",
    placeholder: `${opts.persona.name} にメッセージを送る (Ctrl+Enter で送信)`,
  });
  const counter = el("span", { class: "char-counter", text: "0" });
  const sendBtn = el("button", { class: "btn btn-primary", text: "送信" });
  const cancelBtn = el("button", { class: "btn", text: "中断" });
  cancelBtn.style.display = "none";
  const endBtn = el("button", { class: "btn btn-small", text: "会話を終える" });

  let maxChars = 4000;
  void api.getSettings().then((s) => {
    maxChars = s.inputMaxChars;
    updateCounter();
  });

  const updateCounter = () => {
    const n = input.value.length;
    counter.textContent = `${n} / ${maxChars}`;
    counter.classList.toggle("over-limit", n > maxChars); // EC-05
  };
  input.addEventListener("input", updateCounter);

  const bubbleFor = (u: { speakerKind: string; speakerName: string; content: string; state?: string }, id?: string) => {
    const isUser = u.speakerKind === "user";
    const bubble = el("div", { class: `bubble ${isUser ? "bubble-user" : "bubble-persona"}` }, [
      el("div", { class: "bubble-name", text: u.speakerName }),
      el("div", { class: "bubble-content", text: u.content }),
    ]);
    if (id) bubble.dataset.utteranceId = id;
    if (u.state === "canceled") bubble.append(el("div", { class: "bubble-note", text: "(中断)" }));
    return bubble;
  };

  const scrollToBottom = () => {
    messages.scrollTop = messages.scrollHeight;
  };

  const setGenerating = (on: boolean) => {
    generating = on;
    sendBtn.style.display = on ? "none" : "";
    cancelBtn.style.display = on ? "" : "none";
    input.toggleAttribute("disabled", on);
  };

  const ensureSession = async (): Promise<Session> => {
    if (session && session.status === "active") return session;
    session = await api.startSession("user_dialogue", [opts.persona.id]);
    return session;
  };

  const send = async () => {
    if (generating) return; // 生成中の二重送信を防ぐ
    const text = input.value;
    if (!text.trim()) return; // EC-09
    if (text.length > maxChars) {
      toast(`メッセージが長すぎます (上限 ${maxChars} 文字)`, "error");
      return;
    }
    clear(banner);
    try {
      const s = await ensureSession();
      const preview = await api.sendMessage(s.id, text);
      lastFailedInput = text;
      messages.append(bubbleFor(preview));
      scrollToBottom();
      input.value = "";
      updateCounter();
      setGenerating(true);
    } catch (e) {
      // EC-02: 入力は消さず、エラーと設定導線を表示
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
  cancelBtn.addEventListener("click", () => {
    if (session) void api.cancelGeneration(session.id); // FR-07
  });
  endBtn.addEventListener("click", async () => {
    if (!session) return;
    try {
      await api.endSession(session.id);
      toast("会話を終えました。記憶の整理を始めます");
      session = null;
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  });

  const root = el("div", { class: "chat" }, [
    el("div", { class: "chat-header" }, [
      el("h2", { text: opts.persona.name }),
      endBtn,
    ]),
    messages,
    banner,
    el("div", { class: "chat-input-row" }, [input, el("div", { class: "chat-input-side" }, [counter, sendBtn, cancelBtn])]),
  ]);

  return {
    root,
    sessionId: () => session?.id ?? null,
    onUtteranceStarted(utteranceId, speakerName) {
      messages.append(
        bubbleFor({ speakerKind: "persona", speakerName, content: "" }, utteranceId),
      );
      scrollToBottom();
    },
    onUtteranceDelta(utteranceId, delta) {
      const bubble = messages.querySelector<HTMLElement>(`[data-utterance-id="${utteranceId}"] .bubble-content`);
      if (bubble) {
        bubble.textContent += delta; // FR-05 逐次表示
        scrollToBottom();
      }
    },
    onUtteranceCompleted(utteranceId, state) {
      setGenerating(false);
      if (state === "canceled") {
        const bubble = messages.querySelector<HTMLElement>(`[data-utterance-id="${utteranceId}"]`);
        bubble?.append(el("div", { class: "bubble-note", text: "(中断)" }));
      }
    },
    onGenerationFailed(message, kind) {
      setGenerating(false);
      clear(banner);
      banner.append(errorBanner(message, kind === "connection" ? opts.openSettings : undefined));
      // EC-02: 失敗した入力を復元する
      if (lastFailedInput && !input.value) {
        input.value = lastFailedInput;
        updateCounter();
      }
    },
    async dispose() {
      // 画面を離れるときにセッションを終了する (後処理が走る)
      if (session && session.status === "active") {
        try {
          await api.endSession(session.id);
        } catch {
          /* 終了失敗は起動時リカバリで回収される (EC-03) */
        }
      }
    },
  };
}

/// 過去セッションの読み取り専用表示 (FR-06)
export function transcriptView(utterances: Utterance[]): HTMLElement {
  const container = el("div", { class: "chat-messages readonly" });
  for (const u of utterances) {
    const isUser = u.speakerKind === "user";
    const bubble = el("div", { class: `bubble ${isUser ? "bubble-user" : "bubble-persona"}` }, [
      el("div", { class: "bubble-name", text: u.speakerName }),
      el("div", { class: "bubble-content", text: u.content }),
    ]);
    if (u.state === "canceled") bubble.append(el("div", { class: "bubble-note", text: "(中断)" }));
    container.append(bubble);
  }
  if (utterances.length === 0) {
    container.append(el("p", { class: "empty-note", text: "発話はありません" }));
  }
  return container;
}
