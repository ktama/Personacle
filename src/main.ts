import { open } from "@tauri-apps/plugin-dialog";
import { api, subscribeEvents } from "./api";
import type { Persona } from "./types";
import { errorMessage, isAppError } from "./types";
import { clear, confirmDialog, el, toast } from "./ui";
import { autonomousView, type AutonomousController } from "./views/autonomous";
import { chatView, type ChatController } from "./views/chat";
import { memoriesView } from "./views/memories";
import { personaFormView } from "./views/personaForm";
import { personalityView } from "./views/personality";
import { sessionsView } from "./views/sessions";
import { settingsView } from "./views/settings";

type View =
  | { kind: "onboarding" }
  | { kind: "create" }
  | { kind: "persona"; personaId: string; tab: PersonaTab }
  | { kind: "autonomous" }
  | { kind: "settings" };

type PersonaTab = "chat" | "memories" | "personality" | "sessions" | "edit";

const TAB_LABELS: Record<PersonaTab, string> = {
  chat: "チャット",
  memories: "記憶",
  personality: "人格",
  sessions: "履歴",
  edit: "編集",
};

class App {
  private root: HTMLElement;
  private personas: Persona[] = [];
  private view: View = { kind: "onboarding" };
  private chat: ChatController | null = null;
  private auto: AutonomousController | null = null;

  constructor(root: HTMLElement) {
    this.root = root;
  }

  async start(): Promise<void> {
    await subscribeEvents({
      onUtteranceStarted: (p) => {
        if (this.chat?.sessionId() === p.sessionId) this.chat.onUtteranceStarted(p.utteranceId, p.speakerName);
        if (this.auto?.sessionId() === p.sessionId) this.auto.onUtteranceStarted(p.utteranceId, p.speakerName);
      },
      onUtteranceDelta: (p) => {
        if (this.chat?.sessionId() === p.sessionId) this.chat.onUtteranceDelta(p.utteranceId, p.delta);
        if (this.auto?.sessionId() === p.sessionId) this.auto.onUtteranceDelta(p.utteranceId, p.delta);
      },
      onUtteranceCompleted: (p) => {
        if (this.chat?.sessionId() === p.sessionId) this.chat.onUtteranceCompleted(p.utteranceId, p.state);
      },
      onGenerationFailed: (p) => {
        if (this.chat?.sessionId() === p.sessionId) {
          this.chat.onGenerationFailed(p.message, p.kind);
        } else {
          toast(p.message, "error");
        }
      },
      onSessionStatusChanged: (p) => {
        this.auto?.onSessionStatusChanged(p.sessionId, p.status);
      },
      onPostprocessCompleted: (p) => {
        if (p.memoryCount > 0 || p.eventCount > 0) {
          toast(`会話から ${p.memoryCount} 件の記憶と ${p.eventCount} 件の変化が生まれました`);
        }
      },
    });
    await this.reload();
  }

  private async reload(selectId?: string): Promise<void> {
    try {
      this.personas = await api.listPersonas();
    } catch (e) {
      toast(errorMessage(e), "error");
      this.personas = [];
    }
    if (this.personas.length === 0) {
      this.view = { kind: "onboarding" }; // EC-01
    } else if (selectId) {
      this.view = { kind: "persona", personaId: selectId, tab: "chat" };
    } else if (this.view.kind === "onboarding" || this.view.kind === "create") {
      this.view = { kind: "persona", personaId: this.personas[0].id, tab: "chat" };
    }
    this.render();
  }

  private async navigate(view: View): Promise<void> {
    // チャットから離れるときはセッションを閉じる (後処理が走る)
    if (this.chat) {
      const leavingChat =
        view.kind !== "persona" ||
        this.view.kind !== "persona" ||
        view.personaId !== this.view.personaId ||
        view.tab !== "chat";
      if (leavingChat) {
        await this.chat.dispose();
        this.chat = null;
      }
    }
    this.view = view;
    this.render();
  }

  private render(): void {
    clear(this.root);
    this.root.append(this.sidebar(), this.mainArea());
  }

  private sidebar(): HTMLElement {
    const items: HTMLElement[] = [el("div", { class: "app-title", text: "Personacle" })];
    for (const p of this.personas) {
      const active = this.view.kind === "persona" && this.view.personaId === p.id;
      items.push(
        el("button", {
          class: `side-item ${active ? "active" : ""}`,
          text: p.name,
          onClick: () => void this.navigate({ kind: "persona", personaId: p.id, tab: "chat" }),
        }),
      );
    }
    items.push(
      el("button", {
        class: `side-item side-action ${this.view.kind === "create" ? "active" : ""}`,
        text: "+ 新しいペルソナ",
        onClick: () => void this.navigate({ kind: "create" }),
      }),
      el("button", {
        class: "side-item side-action",
        text: "ファイルから取り込む",
        onClick: () => void this.importPersona(),
      }),
    );
    if (this.personas.length >= 2) {
      items.push(
        el("button", {
          class: `side-item side-action ${this.view.kind === "autonomous" ? "active" : ""}`,
          text: "自律会話",
          onClick: () => void this.navigate({ kind: "autonomous" }),
        }),
      );
    }
    items.push(
      el("button", {
        class: `side-item side-action ${this.view.kind === "settings" ? "active" : ""}`,
        text: "設定",
        onClick: () => void this.navigate({ kind: "settings" }),
      }),
    );
    return el("nav", { class: "sidebar" }, items);
  }

  private mainArea(): HTMLElement {
    const main = el("main", { class: "main" });
    switch (this.view.kind) {
      case "onboarding":
        main.append(this.onboarding());
        break;
      case "create":
        main.append(
          personaFormView({
            onSaved: (id) => void this.reload(id),
          }),
        );
        break;
      case "settings":
        main.append(settingsView(() => void 0));
        break;
      case "autonomous":
        this.auto = autonomousView(this.personas);
        main.append(this.auto.root);
        break;
      case "persona":
        main.append(this.personaArea(this.view.personaId, this.view.tab));
        break;
    }
    return main;
  }

  private personaArea(personaId: string, tab: PersonaTab): HTMLElement {
    const persona = this.personas.find((p) => p.id === personaId);
    if (!persona) return el("p", { class: "empty-note", text: "ペルソナが見つかりません" });

    const tabs = el(
      "div",
      { class: "tabs" },
      (Object.keys(TAB_LABELS) as PersonaTab[]).map((t) =>
        el("button", {
          class: `tab ${t === tab ? "active" : ""}`,
          text: TAB_LABELS[t],
          onClick: () => void this.navigate({ kind: "persona", personaId, tab: t }),
        }),
      ),
    );

    const body = el("div", { class: "tab-body" });
    switch (tab) {
      case "chat": {
        this.chat = chatView({
          persona,
          openSettings: () => void this.navigate({ kind: "settings" }),
        });
        body.append(this.chat.root);
        break;
      }
      case "memories":
        body.append(memoriesView(personaId));
        break;
      case "personality":
        body.append(el("p", { class: "empty-note", text: "読み込み中..." }));
        void api
          .getPersona(personaId)
          .then((detail) => body.replaceChildren(personalityView(detail)))
          .catch((e) => body.replaceChildren(el("p", { class: "empty-note", text: errorMessage(e) })));
        break;
      case "sessions":
        body.append(sessionsView(personaId));
        break;
      case "edit":
        body.append(el("p", { class: "empty-note", text: "読み込み中..." }));
        void api
          .getPersona(personaId)
          .then((detail) =>
            body.replaceChildren(
              personaFormView({
                existing: detail,
                onSaved: () => void this.reload(personaId),
                onDeleted: () => void this.reload(),
              }),
            ),
          )
          .catch((e) => body.replaceChildren(el("p", { class: "empty-note", text: errorMessage(e) })));
        break;
    }
    return el("div", { class: "persona-area" }, [tabs, body]);
  }

  /// エクスポートファイルからのペルソナ取込 (FR-18)
  private async importPersona(): Promise<void> {
    const path = await open({
      multiple: false,
      filters: [{ name: "Personacle ペルソナ", extensions: ["json"] }],
    });
    if (typeof path !== "string") return;
    const tryImport = async (force: boolean): Promise<void> => {
      try {
        const p = await api.importPersona(path, force);
        toast(`「${p.name}」を取り込みました。記憶の索引を再構築しています`);
        await this.reload(p.id);
      } catch (e) {
        // 同名は確認のうえ別個体として取込 (EC-04 相当)
        if (isAppError(e) && e.kind === "duplicate_name") {
          const ok = await confirmDialog(`${e.message}。別のペルソナとして取り込みますか?`);
          if (ok) await tryImport(true);
          return;
        }
        toast(errorMessage(e), "error");
      }
    };
    await tryImport(false);
  }

  /// 初回起動時の案内 (EC-01)
  private onboarding(): HTMLElement {
    return el("div", { class: "onboarding" }, [
      el("h1", { text: "Personacle へようこそ" }),
      el("p", {
        text: "Personacle は、あなたのPCの中で動く「記憶と人格を持つAIキャラクター」の育成アプリです。会話の内容は記憶として積み重なり、性格や関係性が少しずつ変わっていきます。データはすべてこのPCの中に保存されます。",
      }),
      el("ol", { class: "onboarding-steps" }, [
        el("li", { text: "推論エンジン (Ollama など) を起動し、「設定」で接続を確認する" }),
        el("li", { text: "最初のペルソナを作成する" }),
        el("li", { text: "会話して、記憶と人格が育つのを見守る" }),
      ]),
      el("div", { class: "form-buttons" }, [
        el("button", {
          class: "btn",
          text: "設定を開く",
          onClick: () => void this.navigate({ kind: "settings" }),
        }),
        el("button", {
          class: "btn btn-primary",
          text: "最初のペルソナを作る",
          onClick: () => void this.navigate({ kind: "create" }),
        }),
      ]),
    ]);
  }
}

const rootEl = document.querySelector<HTMLDivElement>("#app");
if (rootEl) {
  rootEl.className = "app-shell";
  void new App(rootEl).start();
}
