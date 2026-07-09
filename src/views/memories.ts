import { api } from "../api";
import type { Memory } from "../types";
import { MEMORY_KIND_LABELS, errorMessage } from "../types";
import { clear, confirmDialog, el, formatDateTime, toast } from "../ui";

const KIND_ORDER = ["fact", "event", "promise", "impression"];

/// 記憶ビューア (FR-10/11/28, EC-06 のアーカイブ表示)。検索・種別絞り込みつき。
export function memoriesView(personaId: string): HTMLElement {
  const list = el("div", { class: "memory-list" });
  const archivedToggle = el("input", { type: "checkbox" });
  const countLabel = el("span", { class: "muted", text: "" });
  const searchInput = el("input", { class: "input", type: "search", placeholder: "記憶を検索..." });
  // 種別フィルタ (FR-28)
  const kindBoxes = KIND_ORDER.map((k) => ({ kind: k, box: el("input", { type: "checkbox" }) }));

  const selectedKinds = () => kindBoxes.filter((k) => k.box.checked).map((k) => k.kind);

  const render = async () => {
    clear(list);
    let memories: Memory[];
    try {
      const q = searchInput.value.trim();
      const kinds = selectedKinds();
      // 検索語・種別絞り込みがあれば search、なければ従来の一覧 (FR-28)
      memories =
        q || kinds.length > 0
          ? await api.searchMemories(personaId, q, kinds, archivedToggle.checked)
          : await api.listMemories(personaId, archivedToggle.checked);
    } catch (e) {
      list.append(el("p", { class: "empty-note", text: errorMessage(e) }));
      return;
    }
    countLabel.textContent = `${memories.length} 件`;
    if (memories.length === 0) {
      const msg = searchInput.value.trim() || selectedKinds().length > 0
        ? "条件に合う記憶はありません"
        : "まだ記憶はありません。会話を終えると記憶が生まれます";
      list.append(el("p", { class: "empty-note", text: msg }));
      return;
    }
    for (const m of memories) {
      list.append(memoryRow(m, render));
    }
  };

  let debounce: number | undefined;
  const scheduleRender = () => {
    window.clearTimeout(debounce);
    debounce = window.setTimeout(() => void render(), 200);
  };
  searchInput.addEventListener("input", scheduleRender);
  kindBoxes.forEach((k) => k.box.addEventListener("change", () => void render()));
  archivedToggle.addEventListener("change", () => void render());
  void render();

  const kindFilters = el(
    "div",
    { class: "kind-filters" },
    kindBoxes.map((k) => el("label", { class: "toggle-label" }, [k.box, MEMORY_KIND_LABELS[k.kind] ?? k.kind])),
  );

  return el("div", { class: "panel" }, [
    el("div", { class: "panel-header" }, [
      el("h2", { text: "記憶" }),
      countLabel,
      el("label", { class: "toggle-label" }, [archivedToggle, "アーカイブも表示"]),
    ]),
    el("div", { class: "memory-search" }, [searchInput, kindFilters]),
    list,
  ]);
}

function memoryRow(m: Memory, refresh: () => Promise<void>): HTMLElement {
  const content = el("div", { class: "memory-content", text: m.content });
  // 統合記憶は source_session_id を持たない (FR-23): 由来を開けるボタンを出す
  const isConsolidated = m.sourceSessionId === null && !m.userEdited;
  const meta = el("div", { class: "memory-meta" }, [
    el("span", { class: `memory-kind kind-${m.kind}`, text: MEMORY_KIND_LABELS[m.kind] ?? m.kind }),
    el("span", { text: `重要度 ${m.importance}` }),
    el("span", { text: formatDateTime(m.createdAt) }),
    ...(m.userEdited ? [el("span", { class: "muted", text: "編集済み" })] : []),
    ...(m.archived ? [el("span", { class: "muted", text: "アーカイブ" })] : []),
    ...(isConsolidated ? [el("span", { class: "muted", text: "統合" })] : []),
  ]);

  // 統合記憶の由来一覧 (FR-23)
  const sourcesBox = el("div", { class: "memory-sources" });
  const sourcesBtn = isConsolidated
    ? el("button", {
        class: "btn btn-small",
        text: "由来を見る",
        onClick: async () => {
          if (sourcesBox.childElementCount > 0) {
            clear(sourcesBox);
            return;
          }
          try {
            const sources = await api.getMemorySources(m.id);
            if (sources.length === 0) {
              sourcesBox.append(el("p", { class: "muted", text: "由来の記憶はありません" }));
            } else {
              sourcesBox.append(
                el("div", { class: "muted", text: "統合元:" }),
                ...sources.map((s) =>
                  el("div", { class: "source-item" }, [`・${s.content}（${formatDateTime(s.createdAt)}）`]),
                ),
              );
            }
          } catch (e) {
            toast(errorMessage(e), "error");
          }
        },
      })
    : null;

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
      // EC-16: 統合記憶の削除時は、元記憶を想起対象へ戻すか確認する
      let restore = false;
      if (isConsolidated) {
        restore = await confirmDialog(
          "この統合記憶を削除します。統合元の記憶を想起の対象に戻しますか? (OK=戻す / キャンセル=戻さず削除のみ中止)",
        );
      }
      const ok = await confirmDialog("この記憶を削除しますか? ペルソナはこの内容を思い出せなくなります", true);
      if (!ok) return;
      try {
        await api.deleteMemory(m.id, restore); // FR-11 / EC-16
        toast(restore ? "統合記憶を削除し、元の記憶を戻しました" : "記憶を削除しました");
        await refresh();
      } catch (e) {
        toast(errorMessage(e), "error");
      }
    },
  });

  const buttons = [editBtn, ...(sourcesBtn ? [sourcesBtn] : []), deleteBtn];
  return el("div", { class: "memory-row" }, [content, meta, el("div", { class: "row-buttons" }, buttons), sourcesBox]);
}
