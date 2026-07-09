import { api } from "../api";
import type { RelationshipGraph } from "../types";
import { errorMessage } from "../types";
import { el } from "../ui";
import { relationshipGraphSvg } from "./charts";

/// ペルソナ関係図ビュー (FR-30)。ノードを選ぶと詳細へ移動する。
export function graphView(onSelectPersona: (personaId: string) => void): HTMLElement {
  const body = el("div", { class: "graph-body" }, [el("p", { class: "empty-note", text: "読み込み中..." })]);

  const render = async () => {
    let graph: RelationshipGraph;
    try {
      graph = await api.getRelationshipGraph();
    } catch (e) {
      body.replaceChildren(el("p", { class: "empty-note", text: errorMessage(e) }));
      return;
    }
    if (graph.nodes.length <= 1) {
      body.replaceChildren(el("p", { class: "empty-note", text: "ペルソナがいません" }));
      return;
    }
    const svg = relationshipGraphSvg(graph, onSelectPersona);
    body.replaceChildren(svg as unknown as Node);
  };
  void render();

  return el("div", { class: "panel" }, [
    el("div", { class: "panel-header" }, [
      el("h2", { text: "関係図" }),
      el("span", { class: "muted", text: "線の太さは親密度。ペルソナをクリックで詳細へ" }),
    ]),
    body,
  ]);
}
