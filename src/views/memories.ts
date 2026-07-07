import { api } from "../api";
import type { Memory } from "../types";
import { MEMORY_KIND_LABELS, errorMessage } from "../types";
import { clear, confirmDialog, el, formatDateTime, toast } from "../ui";

/// 記憶ビューア (FR-10/11, EC-06 のアーカイブ表示)
export function memoriesView(personaId: string): HTMLElement {
  const list = el("div", { class: "memory-list" });
  const archivedToggle = el("input", { type: "checkbox" });
  const countLabel = el("span", { class: "muted", text: "" });

  const render = async () => {
    clear(list);
    let memories: Memory[];
    try {
      memories = await api.listMemories(personaId, archivedToggle.checked);
    } catch (e) {
      list.append(el("p", { class: "empty-note", text: errorMessage(e) }));
      return;
    }
    countLabel.textContent = `${memories.length} 件`;
    if (memories.length === 0) {
      list.append(el("p", { class: "empty-note", text: "まだ記憶はありません。会話を終えると記憶が生まれます" }));
      return;
    }
    for (const m of memories) {
      list.append(memoryRow(m, render));
    }
  };

  archivedToggle.addEventListener("change", () => void render());
  void render();

  return el("div", { class: "panel" }, [
    el("div", { class: "panel-header" }, [
      el("h2", { text: "記憶" }),
      countLabel,
      el("label", { class: "toggle-label" }, [archivedToggle, "アーカイブも表示"]),
    ]),
    list,
  ]);
}

function memoryRow(m: Memory, refresh: () => Promise<void>): HTMLElement {
  const content = el("div", { class: "memory-content", text: m.content });
  const meta = el("div", { class: "memory-meta" }, [
    el("span", { class: `memory-kind kind-${m.kind}`, text: MEMORY_KIND_LABELS[m.kind] ?? m.kind }),
    el("span", { text: `重要度 ${m.importance}` }),
    el("span", { text: formatDateTime(m.createdAt) }),
    ...(m.userEdited ? [el("span", { class: "muted", text: "編集済み" })] : []),
    ...(m.archived ? [el("span", { class: "muted", text: "アーカイブ" })] : []),
  ]);

  const editBtn = el("button", {
    class: "btn btn-small",
    text: "編集",
    onClick: () => {
      const textarea = el("textarea", { class: "input", rows: "2" });
      textarea.value = m.content;
      const saveBtn = el("button", {
        class: "btn btn-small btn-primary",
        text: "保存",
        onClick: async () => {
          try {
            await api.updateMemory(m.id, textarea.value); // FR-11
            toast("記憶を更新しました");
            await refresh();
          } catch (e) {
            toast(errorMessage(e), "error");
          }
        },
      });
      const cancelBtn = el("button", { class: "btn btn-small", text: "取消", onClick: () => void refresh() });
      content.replaceWith(el("div", {}, [textarea, el("div", { class: "row-buttons" }, [saveBtn, cancelBtn])]));
    },
  });

  const deleteBtn = el("button", {
    class: "btn btn-small btn-danger",
    text: "削除",
    onClick: async () => {
      const ok = await confirmDialog("この記憶を削除しますか? ペルソナはこの内容を思い出せなくなります", true);
      if (!ok) return;
      try {
        await api.deleteMemory(m.id); // FR-11
        toast("記憶を削除しました");
        await refresh();
      } catch (e) {
        toast(errorMessage(e), "error");
      }
    },
  });

  return el("div", { class: "memory-row" }, [content, meta, el("div", { class: "row-buttons" }, [editBtn, deleteBtn])]);
}
