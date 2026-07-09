import { api } from "../api";
import type { Settings } from "../types";
import { errorMessage } from "../types";
import { confirmDialog, el, toast } from "../ui";

function isLocalEndpoint(url: string): boolean {
  try {
    const host = new URL(url).hostname;
    return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]";
  } catch {
    return false;
  }
}

/// 設定画面 (FR-16, NFR-03 の非localhost警告)
export function settingsView(onSaved: () => void): HTMLElement {
  const endpointInput = el("input", { class: "input", type: "text" });
  const chatModelInput = el("input", { class: "input", type: "text", list: "model-list" });
  const embedModelInput = el("input", { class: "input", type: "text", list: "model-list" });
  const modelList = el("datalist", { id: "model-list" });
  const turnLimitInput = el("input", { class: "input input-narrow", type: "number", min: "2", max: "50" });
  const greetingToggle = el("input", { type: "checkbox" }); // FR-21
  const resultBox = el("div", { class: "test-result" });

  let current: Settings | null = null;
  void api.getSettings().then((s) => {
    current = s;
    endpointInput.value = s.endpoint;
    chatModelInput.value = s.chatModel;
    embedModelInput.value = s.embedModel;
    turnLimitInput.value = String(s.autoTurnLimit);
    greetingToggle.checked = s.greetingEnabled;
  });

  const testBtn = el("button", {
    class: "btn",
    text: "接続確認",
    onClick: async () => {
      resultBox.textContent = "確認中...";
      try {
        await save(false);
        const r = await api.testConnection();
        resultBox.replaceChildren();
        resultBox.append(
          el("p", {
            class: r.connected ? "ok" : "ng",
            text: r.connected ? "✓ 推論エンジンに接続できました" : `✗ ${r.message}`,
          }),
        );
        if (r.connected) {
          modelList.replaceChildren(...r.models.map((m) => el("option", { value: m })));
          resultBox.append(
            el("p", {
              class: r.chatModelFound ? "ok" : "ng",
              text: r.chatModelFound
                ? "✓ チャットモデルが見つかりました"
                : `✗ チャットモデルが見つかりません (導入済み: ${r.models.join(", ") || "なし"})`,
            }),
            el("p", {
              class: r.embedOk ? "ok" : "ng",
              text: r.embedOk
                ? "✓ 埋め込みモデルが使えます"
                : "✗ 埋め込みモデルが使えません (記憶の想起精度が下がります。例: ollama pull nomic-embed-text)",
            }),
          );
        }
      } catch (e) {
        resultBox.textContent = errorMessage(e);
      }
    },
  });

  const save = async (notify: boolean): Promise<void> => {
    if (!current) return;
    const endpoint = endpointInput.value.trim();
    // NFR-03: localhost 以外への送信は明示確認
    if (endpoint && !isLocalEndpoint(endpoint) && endpoint !== current.endpoint) {
      const ok = await confirmDialog(
        `接続先が localhost ではありません。会話や記憶の内容が「${endpoint}」に送信されます。よろしいですか?`,
        true,
      );
      if (!ok) {
        endpointInput.value = current.endpoint;
        return;
      }
    }
    const next: Settings = {
      ...current,
      endpoint,
      chatModel: chatModelInput.value.trim(),
      embedModel: embedModelInput.value.trim(),
      autoTurnLimit: Math.max(2, Math.min(50, Number(turnLimitInput.value) || 12)),
      greetingEnabled: greetingToggle.checked,
    };
    await api.updateSettings(next);
    current = next;
    if (notify) {
      toast("設定を保存しました");
      onSaved();
    }
  };

  return el("div", { class: "form" }, [
    el("h2", { text: "設定" }),
    el("label", { class: "field-label", text: "推論エンジンの接続先 (OpenAI互換API)" }),
    endpointInput,
    el("label", { class: "field-label", text: "チャットモデル" }),
    chatModelInput,
    el("label", { class: "field-label", text: "埋め込みモデル (記憶の想起に使用)" }),
    embedModelInput,
    modelList,
    el("label", { class: "field-label", text: "自律会話のターン数上限 (2〜50)" }),
    turnLimitInput,
    el("label", { class: "toggle-label" }, [greetingToggle, "チャットを開いたときにペルソナから話しかける"]),
    el("div", { class: "form-buttons" }, [
      el("button", { class: "btn btn-primary", text: "保存", onClick: () => void save(true) }),
      testBtn,
    ]),
    resultBox,
  ]);
}
