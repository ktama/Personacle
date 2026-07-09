import { api } from "../api";
import type { Diary } from "../types";
import { errorMessage } from "../types";
import { el } from "../ui";

/// 日記ビュー (FR-27)。日付の新しい順に表示する。
export function diaryView(personaId: string): HTMLElement {
  const list = el("div", { class: "diary-list" });

  const render = async () => {
    let diaries: Diary[];
    try {
      diaries = await api.listDiaries(personaId);
    } catch (e) {
      list.replaceChildren(el("p", { class: "empty-note", text: errorMessage(e) }));
      return;
    }
    if (diaries.length === 0) {
      list.replaceChildren(
        el("p", { class: "empty-note", text: "まだ日記はありません。会話をした日の分が、会話の整理後に生まれます" }),
      );
      return;
    }
    list.replaceChildren(
      ...diaries.map((d) =>
        el("div", { class: "diary-entry" }, [
          el("div", { class: "diary-date", text: d.date }),
          el("div", { class: "diary-body", text: d.content }),
        ]),
      ),
    );
  };
  void render();

  return el("div", { class: "panel" }, [
    el("div", { class: "panel-header" }, [el("h2", { text: "日記" })]),
    list,
  ]);
}
