import { api } from "../api";
import type { Persona, Session } from "../types";
import { errorMessage } from "../types";
import { el, toast } from "../ui";

const MIN_PARTICIPANTS = 2;
const MAX_PARTICIPANTS = 6; // FR-19 / ADR-08

export interface AutonomousController {
  root: HTMLElement;
  onUtteranceStarted(utteranceId: string, speakerName: string): void;
  onUtteranceDelta(utteranceId: string, delta: string): void;
  onSessionStatusChanged(sessionId: string, status: string): void;
  sessionId(): string | null;
}

/// 自律会話画面 (FR-14/15/19, EC-08/12)
export function autonomousView(personas: Persona[]): AutonomousController {
  let session: Session | null = null;
  let participantNames: string[] = [];

  // FR-19: チェックボックスで2〜6体を選ぶ。発話順は一覧の並び順 (ラウンドロビン)
  const checks: { persona: Persona; box: HTMLInputElement }[] = personas.map((p) => ({
    persona: p,
    box: el("input", { type: "checkbox" }),
  }));
  checks.slice(0, 2).forEach((c) => (c.box.checked = true));

  const countLabel = el("span", { class: "muted", text: "" });
  const updateCount = () => {
    const n = checks.filter((c) => c.box.checked).length;
    countLabel.textContent = `${n} 体選択中 (${MIN_PARTICIPANTS}〜${MAX_PARTICIPANTS}体)`;
  };
  checks.forEach((c) => c.box.addEventListener("change", updateCount));
  updateCount();

  const checkList = el(
    "div",
    { class: "auto-checks" },
    checks.map((c) =>
      el("label", { class: "auto-check" }, [c.box, c.persona.name]),
    ),
  );

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
    checks.forEach((c) => c.box.toggleAttribute("disabled", on));
    themeInput.toggleAttribute("disabled", on);
  };

  startBtn.addEventListener("click", async () => {
    const selected = checks.filter((c) => c.box.checked).map((c) => c.persona);
    if (selected.length < MIN_PARTICIPANTS || selected.length > MAX_PARTICIPANTS) {
      toast(`${MIN_PARTICIPANTS}〜${MAX_PARTICIPANTS}体のペルソナを選んでください`, "error");
      return;
    }
    messages.replaceChildren();
    try {
      session = await api.startSession(
        "autonomous",
        selected.map((p) => p.id),
        themeInput.value,
      );
      participantNames = selected.map((p) => p.name);
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
      el("div", { class: "auto-row" }, [checkList, countLabel]),
      themeInput,
      el("div", { class: "auto-row" }, [startBtn, stopBtn, statusLabel]),
    ]),
    messages,
  ]);

  return {
    root,
    sessionId: () => session?.id ?? null,
    onUtteranceStarted(utteranceId, speakerName) {
      // 参加者の並び順で左右を振り分ける (偶数=左、奇数=右)
      const idx = participantNames.indexOf(speakerName);
      const side = idx % 2 === 0 ? "bubble-persona" : "bubble-user";
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
