import { api } from "../api";
import type { PersonaDetail, Series } from "../types";
import { TRAIT_LABELS, errorMessage } from "../types";
import { el } from "../ui";
import { chartLegend, lineChart } from "./charts";

/// 成長ダッシュボード (FR-29)。性格5軸と相手ごとの親密度の推移を折れ線で表示する。
export function dashboardView(personaId: string): HTMLElement {
  const body = el("div", { class: "dashboard" }, [el("p", { class: "empty-note", text: "読み込み中..." })]);

  void load(personaId, body);

  return el("div", { class: "panel" }, [
    el("div", { class: "panel-header" }, [el("h2", { text: "成長の推移" })]),
    body,
  ]);
}

async function load(personaId: string, body: HTMLElement): Promise<void> {
  let traitSeries: Series[];
  let detail: PersonaDetail;
  try {
    [traitSeries, detail] = await Promise.all([api.getTraitSeries(personaId), api.getPersona(personaId)]);
  } catch (e) {
    body.replaceChildren(el("p", { class: "empty-note", text: errorMessage(e) }));
    return;
  }

  const sections: HTMLElement[] = [];

  // 性格5軸
  const hasTraitData = traitSeries.some((s) => s.points.length > 1);
  sections.push(
    el("section", { class: "dashboard-section" }, [
      el("h3", { text: "性格傾向" }),
      hasTraitData
        ? el("div", {}, [lineChart(traitSeries) as unknown as Node, chartLegend(traitSeries, TRAIT_LABELS) as Node])
        : el("p", { class: "muted", text: "変化の履歴がまだありません。会話を重ねると推移が現れます" }),
    ]),
  );

  // 相手ごとの親密度
  const intimacySeries: Series[] = [];
  for (const rel of detail.relationships) {
    try {
      const s = await api.getIntimacySeries(personaId, rel.targetName);
      if (s.points.length > 0) intimacySeries.push(s);
    } catch {
      /* 個別の失敗は無視 */
    }
  }
  sections.push(
    el("section", { class: "dashboard-section" }, [
      el("h3", { text: "相手ごとの親密度" }),
      intimacySeries.length > 0
        ? el("div", {}, [lineChart(intimacySeries) as unknown as Node, chartLegend(intimacySeries)])
        : el("p", { class: "muted", text: "親密度の変化はまだありません" }),
    ]),
  );

  body.replaceChildren(...sections);
}
