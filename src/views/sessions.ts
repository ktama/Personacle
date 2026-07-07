import { api } from "../api";
import type { Session } from "../types";
import { errorMessage } from "../types";
import { clear, el, formatDateTime } from "../ui";
import { transcriptView } from "./chat";

const KIND_LABELS: Record<string, string> = {
  user_dialogue: "1対1",
  autonomous: "自律会話",
};

const STATUS_LABELS: Record<string, string> = {
  active: "進行中",
  ended: "整理中",
  processed: "完了",
};

/// 会話履歴の閲覧 (FR-06)
export function sessionsView(personaId: string): HTMLElement {
  const list = el("div", { class: "session-list" });
  const detail = el("div", { class: "session-detail" });

  const showSession = async (s: Session) => {
    clear(detail);
    detail.append(
      el("div", { class: "session-detail-header" }, [
        el("strong", { text: s.kind === "autonomous" ? `自律会話: ${s.participantNames.join(" × ")}` : s.participantNames[0] }),
        ...(s.theme ? [el("span", { class: "muted", text: `テーマ: ${s.theme}` })] : []),
        el("span", { class: "muted", text: formatDateTime(s.startedAt) }),
      ]),
    );
    try {
      const utterances = await api.getSessionUtterances(s.id);
      detail.append(transcriptView(utterances));
    } catch (e) {
      detail.append(el("p", { class: "empty-note", text: errorMessage(e) }));
    }
  };

  void api
    .listSessions(personaId)
    .then((sessions) => {
      if (sessions.length === 0) {
        list.append(el("p", { class: "empty-note", text: "まだ会話履歴はありません" }));
        return;
      }
      for (const s of sessions) {
        list.append(
          el("button", { class: "session-row", onClick: () => void showSession(s) }, [
            el("span", { class: `session-kind kind-${s.kind}`, text: KIND_LABELS[s.kind] ?? s.kind }),
            el("span", { text: formatDateTime(s.startedAt) }),
            el("span", { class: "muted", text: STATUS_LABELS[s.status] ?? s.status }),
          ]),
        );
      }
    })
    .catch((e) => {
      list.append(el("p", { class: "empty-note", text: errorMessage(e) }));
    });

  return el("div", { class: "panel" }, [
    el("h2", { text: "会話履歴" }),
    el("div", { class: "session-split" }, [list, detail]),
  ]);
}
