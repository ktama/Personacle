// 小さな DOM ヘルパー群 (フレームワーク非依存の方針: ADR-01 の軽量化)

type Attrs = Record<string, string | boolean | ((e: Event) => void)>;

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  children: (Node | string)[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (typeof v === "function") {
      node.addEventListener(k.replace(/^on/, "").toLowerCase(), v as EventListener);
    } else if (typeof v === "boolean") {
      if (v) node.setAttribute(k, "");
    } else if (k === "text") {
      node.textContent = v;
    } else {
      node.setAttribute(k, v);
    }
  }
  for (const c of children) {
    node.append(c);
  }
  return node;
}

export function clear(node: HTMLElement): void {
  node.replaceChildren();
}

export function formatDateTime(ms: number | null): string {
  if (ms == null) return "-";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/// WebView のネイティブダイアログに依存しない確認モーダル (FR-04 の削除確認に使用)
export function confirmDialog(message: string, danger = false): Promise<boolean> {
  return new Promise((resolve) => {
    const close = (result: boolean) => {
      overlay.remove();
      resolve(result);
    };
    const overlay = el("div", { class: "modal-overlay" }, [
      el("div", { class: "modal" }, [
        el("p", { class: "modal-message", text: message }),
        el("div", { class: "modal-buttons" }, [
          el("button", { class: "btn", onClick: () => close(false), text: "キャンセル" }),
          el("button", {
            class: danger ? "btn btn-danger" : "btn btn-primary",
            onClick: () => close(true),
            text: "OK",
          }),
        ]),
      ]),
    ]);
    document.body.append(overlay);
  });
}

let toastTimer: number | undefined;

export function toast(message: string, kind: "info" | "error" = "info"): void {
  let node = document.querySelector<HTMLDivElement>("#toast");
  if (!node) {
    node = el("div", { id: "toast" });
    document.body.append(node);
  }
  node.textContent = message;
  node.className = `toast-${kind} toast-show`;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    node!.classList.remove("toast-show");
  }, 4000);
}

export function errorBanner(message: string, onSettings?: () => void): HTMLElement {
  const children: (Node | string)[] = [el("span", { text: message })];
  if (onSettings) {
    children.push(el("button", { class: "btn btn-small", onClick: onSettings, text: "設定を開く" }));
  }
  return el("div", { class: "error-banner" }, children);
}
