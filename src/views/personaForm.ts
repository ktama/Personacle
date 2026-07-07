import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { TRAIT_LABELS, errorMessage, isAppError } from "../types";
import type { PersonaDetail, PersonaInput, TraitValue } from "../types";
import { confirmDialog, el, toast } from "../ui";

export interface PersonaFormOptions {
  existing?: PersonaDetail;
  onSaved: (personaId: string) => void;
  onDeleted?: () => void;
}

/// ペルソナの作成・編集フォーム (FR-01 / FR-03 / FR-04)
export function personaFormView(opts: PersonaFormOptions): HTMLElement {
  const p = opts.existing?.persona;
  const nameInput = el("input", { class: "input", type: "text", placeholder: "例: アリス" });
  const descInput = el("textarea", { class: "input", rows: "3", placeholder: "例: 明るく好奇心旺盛。新しいものが好き" });
  const styleInput = el("textarea", { class: "input", rows: "2", placeholder: "例: です・ます調。語尾に「〜ですね」が多い" });
  const valuesInput = el("textarea", { class: "input", rows: "2", placeholder: "例: 正直さを大切にする" });
  const introInput = el("textarea", { class: "input", rows: "2", placeholder: "例: アリスです。よろしくお願いします" });
  if (p) {
    nameInput.value = p.name;
    descInput.value = p.description;
    styleInput.value = p.speechStyle;
    valuesInput.value = p.valuesText;
    introInput.value = p.selfIntro;
  }

  // 性格スライダー (作成時のみ。編集では成長値を壊さないため表示しない: FR-03)
  const sliders: Record<string, HTMLInputElement> = {};
  const sliderRows: HTMLElement[] = [];
  if (!opts.existing) {
    for (const [key, label] of Object.entries(TRAIT_LABELS)) {
      const slider = el("input", { class: "slider", type: "range", min: "0", max: "100" });
      slider.value = "50";
      const valueLabel = el("span", { class: "slider-value", text: "50" });
      slider.addEventListener("input", () => (valueLabel.textContent = slider.value));
      sliders[key] = slider;
      sliderRows.push(el("div", { class: "slider-row" }, [el("label", { text: label }), slider, valueLabel]));
    }
  }

  const suggestBtn = el("button", {
    class: "btn btn-small",
    text: "性格をAIに提案させる",
    onClick: async () => {
      const desc = descInput.value.trim();
      if (!desc) {
        toast("先に性格の説明を書いてください", "error");
        return;
      }
      suggestBtn.setAttribute("disabled", "");
      suggestBtn.textContent = "評定中...";
      try {
        const traits = await api.suggestTraits(desc);
        for (const t of traits) {
          if (sliders[t.key]) {
            sliders[t.key].value = String(t.value);
            sliders[t.key].dispatchEvent(new Event("input"));
          }
        }
        toast("性格の初期値を反映しました");
      } catch (e) {
        toast(errorMessage(e), "error");
      } finally {
        suggestBtn.removeAttribute("disabled");
        suggestBtn.textContent = "性格をAIに提案させる";
      }
    },
  });

  const save = async (force: boolean) => {
    const traits: TraitValue[] = Object.entries(sliders).map(([key, s]) => ({
      key,
      value: Number(s.value),
    }));
    const input: PersonaInput = {
      name: nameInput.value,
      description: descInput.value,
      speechStyle: styleInput.value,
      valuesText: valuesInput.value,
      selfIntro: introInput.value,
      traits,
      force,
    };
    try {
      if (opts.existing) {
        await api.updatePersona(opts.existing.persona.id, input);
        toast("保存しました");
        opts.onSaved(opts.existing.persona.id);
      } else {
        const created = await api.createPersona(input);
        toast(`「${created.name}」を作成しました`);
        opts.onSaved(created.id);
      }
    } catch (e) {
      // EC-04: 同名警告 → 確認のうえ force 再送
      if (isAppError(e) && e.kind === "duplicate_name") {
        const ok = await confirmDialog(`${e.message}。別人として作成しますか?`);
        if (ok) await save(true);
        return;
      }
      toast(errorMessage(e), "error");
    }
  };

  const buttons: HTMLElement[] = [
    el("button", { class: "btn btn-primary", text: opts.existing ? "保存" : "作成", onClick: () => void save(false) }),
  ];
  if (opts.existing && opts.onDeleted) {
    buttons.push(
      el("button", {
        class: "btn btn-danger",
        text: "このペルソナを削除",
        onClick: async () => {
          // FR-04: 復元できない旨を明記した確認
          const ok = await confirmDialog(
            `「${opts.existing!.persona.name}」を削除します。人格・記憶・会話履歴は復元できません。よろしいですか?`,
            true,
          );
          if (!ok) return;
          try {
            await api.deletePersona(opts.existing!.persona.id);
            toast("削除しました");
            opts.onDeleted!();
          } catch (e) {
            toast(errorMessage(e), "error");
          }
        },
      }),
    );
  }

  // FR-18: エクスポート (編集画面のみ)
  const exportSection: HTMLElement[] = [];
  if (opts.existing) {
    const historyCheck = el("input", { type: "checkbox" });
    const exportBtn = el("button", {
      class: "btn",
      text: "ファイルに書き出す",
      onClick: async () => {
        const persona = opts.existing!.persona;
        const path = await saveFileDialog({
          defaultPath: `${persona.name}.personacle.json`,
          filters: [{ name: "Personacle ペルソナ", extensions: ["json"] }],
        });
        if (!path) return;
        try {
          const summary = await api.exportPersona(persona.id, historyCheck.checked, path);
          toast(
            `エクスポートしました (記憶 ${summary.memoryCount} 件` +
              (historyCheck.checked ? `、会話 ${summary.sessionCount} 件)` : ")"),
          );
        } catch (e) {
          toast(errorMessage(e), "error");
        }
      },
    });
    exportSection.push(
      el("h3", { class: "form-subhead", text: "エクスポート" }),
      el("p", { class: "muted", text: "初期設定・人格・記憶をファイルに書き出し、別のPCの Personacle に取り込めます" }),
      el("div", { class: "auto-row" }, [
        el("label", { class: "toggle-label" }, [historyCheck, "会話履歴も含める"]),
        exportBtn,
      ]),
    );
  }

  return el("div", { class: "form" }, [
    el("h2", { text: opts.existing ? "ペルソナの編集" : "新しいペルソナ" }),
    el("label", { class: "field-label", text: "名前 (必須)" }),
    nameInput,
    el("label", { class: "field-label", text: "性格" }),
    descInput,
    el("label", { class: "field-label", text: "口調" }),
    styleInput,
    el("label", { class: "field-label", text: "価値観" }),
    valuesInput,
    el("label", { class: "field-label", text: "自己紹介" }),
    introInput,
    ...(sliderRows.length > 0
      ? [el("label", { class: "field-label", text: "性格の初期値" }), suggestBtn, ...sliderRows]
      : []),
    el("div", { class: "form-buttons" }, buttons),
    ...exportSection,
  ]);
}
