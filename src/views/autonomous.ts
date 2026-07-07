import { api } from "../api";
import type { Persona, Session } from "../types";
import { errorMessage } from "../types";
import { el, toast } from "../ui";

export interface AutonomousController {
  root: HTMLElement;
  onUtteranceStarted(utteranceId: string, speakerName: string): void;
  onUtteranceDelta(utteranceId: string, delta: string): void;
  onSessionStatusChanged(sessionId: string, status: string): void;
  sessionId(): string | null;
}

/// 自律会話画面 (FR-14/15, EC-08/12)
export function autonomousView(personas: Persona[]): AutonomousController {
  let session: Session | null = null;

  const selectA = el("select", { class: "input" });
  const selectB = el("select", { class: "input" });
  for (const sel of [selectA, selectB]) {
    for (const p of personas) {
      sel.append(el("option", { value: p.id, text: p.name }));
    }
  }
  if (personas.length > 1) selectB.selectedIndex = 1;

  const themeInput = el("input", {
    class: "input",
    type: "text",
    placeholder: "会話のテーマ (例: 休日の過ごし方)",
  });
  const statusLabel = el("span", { class: "auto-status", text: "" });
  const messages = el("div", { class: "chat-messages" });

  const startBtn = el("button", { class: "btn btn-primary", text: "会話を開始" });
  const stopBtn = el("button", { class: "btn btn-danger", text: "停止" });
  stopBtn.style.display = "none";

  const setRunning = (on: boolean) => {
    startBtn.style.display = on ? "none" : "";
    stopBtn.style.display = on ? "" : "none";
    selectA.toggleAttribute("disabled", on);
    selectB.toggleAttribute("disabled", on);
    themeInput.toggleAttribute("disabled", on);
  };

  startBtn.addEventListener("click", async () => {
    const a = selectA.value;
    const b = selectB.value;
    if (!a || !b || a === b) {
      toast("異なる2体のペルソナを選んでください", "error");
      return;
    }
    messages.replaceChildren();
    try {
      session = await api.startSession("autonomous", [a, b], themeInput.value);
      await api.startAutonomousTurns(session.id);
      setRunning(true);
      statusLabel.textContent = "会話中...";
    } catch (e) {
      toast(errorMessage(e), "error"); // EC-08 の busy もここに出る
    }
  });

  stopBtn.addEventListener("click", async () => {
    if (!session) return;
    try {
      await api.stopSession(session.id); // FR-14: 次の発話生成前に停止
      statusLabel.textContent = "停止中...";
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  });

  const root = el("div", { class: "chat" }, [
    el("div", { class: "auto-controls" }, [
      el("h2", { text: "自律会話" }),
      el("div", { class: "auto-row" }, [
        selectA,
        el("span", { class: "auto-x", text: "×" }),
        selectB,
      ]),
      themeInput,
      el("div", { class: "auto-row" }, [startBtn, stopBtn, statusLabel]),
    ]),
    messages,
  ]);

  return {
    root,
    sessionId: () => session?.id ?? null,
    onUtteranceStarted(utteranceId, speakerName) {
      const side = speakerName === selectA.selectedOptions[0]?.text ? "bubble-persona" : "bubble-user";
      const bubble = el("div", { class: `bubble ${side}` }, [
        el("div", { class: "bubble-name", text: speakerName }),
        el("div", { class: "bubble-content", text: "" }),
      ]);
      bubble.dataset.utteranceId = utteranceId;
      messages.append(bubble);
      messages.scrollTop = messages.scrollHeight;
    },
    onUtteranceDelta(utteranceId, delta) {
      const content = messages.querySelector<HTMLElement>(`[data-utterance-id="${utteranceId}"] .bubble-content`);
      if (content) {
        content.textContent += delta;
        messages.scrollTop = messages.scrollHeight;
      }
    },
    onSessionStatusChanged(sessionId, status) {
      if (session?.id !== sessionId) return;
      if (status === "ended") {
        setRunning(false);
        statusLabel.textContent = "会話終了。記憶の整理中...";
      } else if (status === "processed") {
        statusLabel.textContent = "記憶と関係性に反映されました";
      }
    },
  };
}
