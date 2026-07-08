# ライセンスと配布 (D-4 決定記録)

| 項目 | 内容 |
| --- | --- |
| 決定日 | 2026-07-08 |
| 決定者 | 発注者 |
| 対応する未解決事項 | 要件書 9-5 / 設計書 D-4 |

## 決定

- **Personacle 本体のライセンス: MIT** (LICENSE ファイル参照)
- **配布形態: GitHub Releases** で NSIS/MSI インストーラを配布 (winget / Microsoft Store は将来再検討)

## 依存ライセンス調査の結果 (2026-07-08 実施)

調査方法: `cargo metadata` による全依存クレートのライセンス集計 (503件) と、
配布物に含まれる npm ランタイム依存 (2件) の確認。一覧は THIRD_PARTY_LICENSES.md。

- **Rust クレート**: 大半が MIT / Apache-2.0 のデュアルライセンス。その他も Zlib / ISC / BSD /
  Unicode-3.0 / CC0 / Unlicense 等の許容的ライセンスのみ
- **MPL-2.0 が5件** (cssparser, cssparser-macros, dtoa-short, selectors, option-ext):
  ファイル単位の弱いコピーレフト。**改変せず利用する限りソース開示義務はない**。ライセンス表記のみ必要
- **フロントエンド**: @tauri-apps/api, @tauri-apps/plugin-dialog とも MIT OR Apache-2.0。
  ビルドツール (vite, TypeScript, tauri-cli) は配布物に含まれないため再配布義務なし
- **SQLite** (rusqlite bundled): パブリックドメイン
- **WebView2 Runtime**: Microsoft の再配布可能コンポーネント (Windows 11 は同梱済み)
- **同梱しないもの**: Ollama・言語モデル (gpt-oss=Apache-2.0, Gemma=独自規約) はユーザーが
  自ら導入する構成 (要件 non-goals) のため、本アプリの配布に義務は発生しない

**結論: MIT での公開・バイナリ配布に法的ブロッカーなし。義務はサードパーティ表記の同梱のみ。**

## リリース時のチェックリスト

1. `npm run tauri build` で NSIS/MSI を生成 (インストーラに LICENSE が表示される設定済み)
2. サードパーティライセンスの**本文**を生成して同梱する:

   ```sh
   cargo install cargo-about
   cd src-tauri
   cargo about init          # 初回のみ (about.toml が生成される)
   cargo about generate about.hbs > ../THIRD_PARTY_LICENSES.html
   ```

   (THIRD_PARTY_LICENSES.md は一覧のみ。配布時は本文付きの生成物を推奨)
3. GitHub Releases にインストーラと THIRD_PARTY_LICENSES を添付
4. コード署名は未実施のため、初回起動時に SmartScreen 警告が出る旨を Release ノートに記載する
   (署名証明書の導入はダウンロード数が伸びてから再検討)
