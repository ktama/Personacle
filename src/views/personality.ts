import { api } from "../api";
import type { PersonaDetail, PersonalityEvent } from "../types";
import { TRAIT_LABELS, errorMessage } from "../types";
import { el, formatDateTime } from "../ui";

/// 人格ビューア (FR-13/25): 現在の性格傾向・ムード・関係性・変化履歴
export function personalityView(detail: PersonaDetail): HTMLElement {
  // ムード (FR-25): 減衰計算済みの現在値と直近の変動要因
  const moodSection = el("div", { class: "panel-section" }, [
    el("h3", { text: "今の気分" }),
    el("p", { class: "muted", text: "読み込み中..." }),
  ]);
  void api
    .getMood(detail.persona.id)
    .then((m) => {
      const children: (Node | string)[] = [el("h3", { text: "今の気分" })];
      const badge = el("span", { class: `mood-badge mood-${m.value > 0 ? "pos" : m.value < 0 ? "neg" : "neutral"}` }, [
        m.label || "平常",
      ]);
      children.push(el("div", { class: "mood-row" }, [badge, el("span", { class: "muted", text: `(${m.value > 0 ? "+" : ""}${m.value})` })]));
      if (m.recentEvent) {
        children.push(el("p", { class: "muted", text: `直近の変化: ${m.recentEvent.label || "-"} (${formatDateTime(m.recentEvent.createdAt)})` }));
      }
      moodSection.replaceChildren(...children);
    })
    .catch(() => moodSection.replaceChildren(el("h3", { text: "今の気分" }), el("p", { class: "muted", text: "-" })));

  const traitsSection = el("div", { class: "panel-section" }, [
    el("h3", { text: "性格傾向" }),
    ...detail.traits.map((t) =>
      barRow(TRAIT_LABELS[t.key] ?? t.key, t.value, 100),
    ),
  ]);

  const relSection = el("div", { class: "panel-section" }, [
    el("h3", { text: "関係性" }),
    ...(detail.relationships.length === 0
      ? [el("p", { class: "empty-note", text: "まだ誰とも関係が築かれていません" })]
      : detail.relationships.map((r) =>
          el("div", { class: "relationship-row" }, [
            el("div", { class: "rel-name", text: r.targetKind === "user" ? "ユーザー" : r.targetName }),
            barRow("親密度", r.intimacy, 100),
            ...(r.impressionText
              ? [el("div", { class: "rel-impression", text: `印象: ${r.impressionText}` })]
              : []),
          ]),
        )),
  ]);

  const historyList = el("div", { class: "history-list" }, [
    el("p", { class: "empty-note", text: "読み込み中..." }),
  ]);
  void api
    .getPersonalityHistory(detail.persona.id)
    .then((events) => {
      historyList.replaceChildren();
      if (events.length === 0) {
        historyList.append(el("p", { class: "empty-note", text: "まだ変化はありません" }));
        return;
      }
      for (const e of events.slice(0, 100)) {
        historyList.append(historyRow(e));
      }
    })
    .catch((e) => {
      historyList.replaceChildren(el("p", { class: "empty-note", text: errorMessage(e) }));
    });

  return el("div", { class: "panel" }, [
    el("h2", { text: "人格プロファイル" }),
    moodSection,
    traitsSection,
    relSection,
    el("div", { class: "panel-section" }, [el("h3", { text: "変化の履歴" }), historyList]),
  ]);
}

function barRow(label: string, value: number, max: number): HTMLElement {
  const pct = Math.max(0, Math.min(100, (value / max) * 100));
  const fill = el("div", { class: "bar-fill" });
  fill.style.width = `${pct}%`;
  return el("div", { class: "bar-row" }, [
    el("span", { class: "bar-label", text: label }),
    el("div", { class: "bar-track" }, [fill]),
    el("span", { class: "bar-value", text: String(value) }),
  ]);
}

function historyRow(e: PersonalityEvent): HTMLElement {
  const [type, target] = splitItem(e.item);
  const label =
    type === "trait"
      ? `${TRAIT_LABELS[target] ?? target}`
      : type === "intimacy"
        ? `${target}への親密度`
        : `${target}への印象`;
  return el("div", { class: "history-row" }, [
    el("span", { class: "muted", text: formatDateTime(e.createdAt) }),
    el("span", { text: label }),
    el("span", { class: "history-change", text: `${e.oldValue || "(なし)"} → ${e.newValue}` }),
  ]);
}

function splitItem(item: string): [string, string] {
  const idx = item.indexOf(":");
  return idx === -1 ? [item, ""] : [item.slice(0, idx), item.slice(idx + 1)];
}
