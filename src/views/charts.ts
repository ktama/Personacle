// 自作SVGチャート (ADR-17: 依存追加なし)。折れ線グラフと関係図の描画ヘルパー。
import type { RelationshipGraph, Series } from "../types";

const SVG_NS = "http://www.w3.org/2000/svg";

function svgEl<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number> = {},
): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) {
    node.setAttribute(k, String(v));
  }
  return node;
}

/// 表示幅に応じてデータ点を間引く (NFR-08: 大量点でも軽量に描く)
function downsample<T>(points: T[], maxPoints: number): T[] {
  if (points.length <= maxPoints) return points;
  const step = points.length / maxPoints;
  const out: T[] = [];
  for (let i = 0; i < maxPoints; i++) {
    out.push(points[Math.floor(i * step)]);
  }
  out.push(points[points.length - 1]);
  return out;
}

const LINE_COLORS = ["#4f8cff", "#ff7ac2", "#54c66a", "#f6a942", "#9b6cf0", "#38c0c0"];

/// 複数系列の折れ線グラフ (FR-29)。値域は 0〜100 固定 (性格軸・親密度)。
export function lineChart(seriesList: Series[], opts: { width?: number; height?: number } = {}): SVGSVGElement {
  const width = opts.width ?? 640;
  const height = opts.height ?? 260;
  const pad = { top: 16, right: 12, bottom: 24, left: 32 };
  const plotW = width - pad.left - pad.right;
  const plotH = height - pad.top - pad.bottom;
  const minY = 0;
  const maxY = 100;

  const svg = svgEl("svg", { viewBox: `0 0 ${width} ${height}`, class: "line-chart", role: "img" });
  svg.setAttribute("width", "100%");

  // 全系列の時間範囲
  const allT = seriesList.flatMap((s) => s.points.map((p) => p.t));
  const tMin = allT.length ? Math.min(...allT) : 0;
  const tMax = allT.length ? Math.max(...allT) : 1;
  const tSpan = tMax - tMin || 1;

  const xOf = (t: number) => pad.left + ((t - tMin) / tSpan) * plotW;
  const yOf = (v: number) => pad.top + (1 - (v - minY) / (maxY - minY)) * plotH;

  // Y 軸グリッド (0/50/100)
  for (const gv of [0, 50, 100]) {
    const y = yOf(gv);
    svg.append(svgEl("line", { x1: pad.left, y1: y, x2: width - pad.right, y2: y, class: "chart-grid" }));
    const label = svgEl("text", { x: 4, y: y + 4, class: "chart-axis" });
    label.textContent = String(gv);
    svg.append(label);
  }

  seriesList.forEach((s, idx) => {
    const pts = downsample(s.points, 200);
    if (pts.length === 0) return;
    const color = LINE_COLORS[idx % LINE_COLORS.length];
    if (pts.length === 1) {
      svg.append(svgEl("circle", { cx: xOf(pts[0].t), cy: yOf(pts[0].value), r: 3, fill: color }));
    } else {
      const d = pts.map((p, i) => `${i === 0 ? "M" : "L"}${xOf(p.t).toFixed(1)},${yOf(p.value).toFixed(1)}`).join(" ");
      const path = svgEl("path", { d, fill: "none", stroke: color, "stroke-width": 2 });
      svg.append(path);
    }
  });

  return svg;
}

/// 系列の凡例
export function chartLegend(seriesList: Series[], labels?: Record<string, string>): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "chart-legend";
  seriesList.forEach((s, idx) => {
    const item = document.createElement("span");
    item.className = "legend-item";
    const swatch = document.createElement("span");
    swatch.className = "legend-swatch";
    swatch.style.background = LINE_COLORS[idx % LINE_COLORS.length];
    const text = document.createElement("span");
    text.textContent = labels?.[s.key] ?? s.key;
    item.append(swatch, text);
    wrap.append(item);
  });
  return wrap;
}

/// ペルソナ関係図 (FR-30)。ノードを円環配置し、親密度で辺の太さ・濃さを変える。
export function relationshipGraphSvg(
  graph: RelationshipGraph,
  onSelect: (personaId: string) => void,
  size = 420,
): SVGSVGElement {
  const svg = svgEl("svg", { viewBox: `0 0 ${size} ${size}`, class: "relationship-graph", role: "img" });
  svg.setAttribute("width", "100%");
  const cx = size / 2;
  const cy = size / 2;
  const radius = size / 2 - 56;

  const pos = new Map<string, { x: number; y: number }>();
  const n = graph.nodes.length;
  graph.nodes.forEach((node, i) => {
    // ユーザーを中心、ペルソナを円環に置く
    if (node.kind === "user" && n > 1) {
      pos.set(node.id, { x: cx, y: cy });
    } else {
      const angle = (i / Math.max(n, 1)) * Math.PI * 2 - Math.PI / 2;
      pos.set(node.id, { x: cx + Math.cos(angle) * radius, y: cy + Math.sin(angle) * radius });
    }
  });

  // 辺
  for (const e of graph.edges) {
    const a = pos.get(e.from);
    const b = pos.get(e.to);
    if (!a || !b) continue;
    const intensity = Math.max(0, Math.min(100, e.intimacy)) / 100;
    const line = svgEl("line", {
      x1: a.x, y1: a.y, x2: b.x, y2: b.y,
      stroke: "#4f8cff",
      "stroke-opacity": (0.15 + intensity * 0.7).toFixed(2),
      "stroke-width": (1 + intensity * 5).toFixed(1),
    });
    svg.append(line);
  }

  // ノード
  for (const node of graph.nodes) {
    const p = pos.get(node.id);
    if (!p) continue;
    const g = svgEl("g", { class: "graph-node", tabindex: 0 });
    if (node.kind === "persona") {
      g.style.cursor = "pointer";
      g.addEventListener("click", () => onSelect(node.id));
      g.addEventListener("keydown", (ev) => {
        if ((ev as KeyboardEvent).key === "Enter") onSelect(node.id);
      });
    }
    const circle = svgEl("circle", {
      cx: p.x, cy: p.y, r: node.kind === "user" ? 22 : 26,
      fill: node.kind === "user" ? "#f6a942" : "#2b3350",
      stroke: "#4f8cff", "stroke-width": 2,
    });
    const label = svgEl("text", { x: p.x, y: p.y + 42, class: "graph-label", "text-anchor": "middle" });
    label.textContent = node.name;
    g.append(circle, label);
    svg.append(g);
  }
  return svg;
}
