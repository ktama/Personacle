# plan.md — Personacle 実装計画

入力: docs/requirements.md v0.1 / docs/design.md v0.1 (全ADR承認済み)

## 環境 (S0: 確認済み 2026-07-07)

- Rust 1.96 / Node 22.16 / npm 11.4 — OK
- Ollama 稼働中 (127.0.0.1:11434, OpenAI互換API疎通OK)。モデル: gpt-oss:20b のみ
- 埋め込みモデル未導入 → S11 で nomic-embed-text を pull
- 注意: PowerShell の Invoke-RestMethod は localhost でプロキシを踏む。疎通確認は `curl.exe --noproxy "*"` を使う
- **バージョン管理なし**(git未初期化)。ユーザー指示がないため git init はせず、完了報告で提案する

## 実装内の自由裁量の決定(逸脱ではなく設計が委ねた範囲)

- フロントエンドはフレームワークなしの TypeScript + Vite(依存最小・ADR-01の軽量方針に沿う)
- DB接続は Mutex 保護の単一コネクション(シングルユーザーのため接続プール不要)
- 埋め込みベクトルは f32 リトルエンディアンの BLOB
- キャンセルは tokio CancellationToken、セッションIDごとに管理
- 推論APIは trait 化し、テストはモック実装で行う

## ステップ

- [ ] S1: プロジェクト骨格 (Tauri 2 + vanilla TS + Vite、アイコン生成、依存解決)
      検証: `cargo check` 成功 / `npm run build` 成功
- [ ] S2: DBレイヤ (schema v1, WAL, migrations, モデル, リポジトリ) — FR-17, ADR-07, 設計5章
      検証: `cargo test db` (往復・マイグレーション・削除整合性 EC-07)
- [ ] S3: InferenceClient (chat stream SSE / embeddings / models, エラー分類) — ADR-02, 設計6.3
      検証: `cargo test inference` (SSE解析・エラー分類の単体)
- [ ] S4: MemoryService (cosine, 新しさ減衰, スコアリング, 上位K, アーカイブ EC-06) — ADR-04
      検証: `cargo test memory`
- [ ] S5: PersonalityService (クランプ, イベント追記, 現在値) — ADR-05, FR-12/13
      検証: `cargo test personality` (上限クランプの境界値)
- [ ] S6: PromptBuilder (system組み立て, トークン予算での履歴切詰め) — 設計6.4
      検証: `cargo test prompt`
- [ ] S7: ConversationService (セッション, 排他 EC-08, キャンセル FR-07, 自律会話ターンループ FR-14) — 設計7章フロー1,2
      検証: `cargo test conversation` (モック推論)
- [ ] S8: BackgroundWorker (記憶抽出→埋め込み→人格評定→クランプ適用, 起動時リカバリ EC-03) — ADR-06, 設計7章フロー3,4
      検証: `cargo test worker` (JSON解析の頑健性含む)
- [ ] S9: Command Facade + イベント (検証: 空入力 EC-09, 上限 EC-05, 制御文字 EC-10, 同名警告 EC-04) — 設計6.1/6.2
      検証: `cargo test commands` + `cargo check`
- [ ] S10: フロントエンド (オンボーディング EC-01, ペルソナ管理, チャット, 自律会話, 記憶/人格ビューア, 履歴, 設定)
      検証: `npm run build` (tsc + vite) 成功
- [ ] S11: 実機統合 (nomic-embed-text pull, Ollama相手の ignored 統合テスト: 作成→対話→終了→後処理→記憶→想起)
      検証: `cargo test --test integration -- --ignored` 成功
- [ ] S12: README(導入手順 NFR-07) + 全テスト実行 + 報告
      検証: `cargo test` 全件成功 / `npm run build` 成功

## 受け入れ基準→検証の対応(主要)

| 基準 | 検証手段 |
| --- | --- |
| FR-01/03/04 CRUD | S2 リポジトリ単体 + S9 コマンド単体 + GUI手動(報告に明記) |
| FR-05 逐次表示 | S3 SSE解析単体 + S11 実機 + GUI手動 |
| FR-07 キャンセル保存 | S7 単体(キャンセル時 state=canceled で保存) |
| FR-08/09 記憶生成・想起 | S8 単体(モック) + S11 実機(再起動相当=新セッションで想起) |
| FR-11 編集後の再埋め込み | S8/S9 単体 |
| FR-12 変化量上限 | S5 境界値単体(±2/±5 クランプ) |
| FR-13 履歴 | S2/S5 単体(イベント追記) |
| FR-14/15 自律会話 | S7 単体(交互・上限・停止) + S8(参加者ごと後処理) |
| FR-16 接続確認 | S3 単体 + S11 実機 |
| FR-17 永続化 | S2 単体(別コネクション再読込) |
| NFR-05 リカバリ | S8 単体(active残留→再処理) |
| EC-04/05/09/10 | S9 検証単体 |
| NFR-01/07, FR-02等のGUI項目 | 実装後の手動確認項目として報告 |

## 進捗記録

- S1〜S9 完了 (2026-07-07): `cargo test` → **47 passed / 0 failed**
  - 内訳: db 6件, inference(SSE) 4件, memory 6件, personality 5件, prompt 4件, conversation 8件, worker 8件, commands 6件
  - 修正1回: async-trait とコールバック `&str` のライフタイム衝突 → `FnMut(String)` (所有権渡し) に変更してグリーン
- アイコン生成済み (`npx tauri icon`)。npm install 済み
- S10 完了 (2026-07-08): `npm run build` (tsc strict + vite) 成功。バンドル 26KB
  - 構成: main.ts(シェル/ルーティング/イベント配線) + views 7画面 + api/types/ui
  - 修正1回: 未使用変数 `generating` → 二重送信ガードとして使用
- README.md 作成 (NFR-07 導入手順)
- S11 完了 (2026-07-08): nomic-embed-text 導入。`cargo test --lib real_ollama -- --ignored` → **2 passed**
  - E2E: 好物を伝える→終了→記憶抽出(fact/8)→親密度20→21・印象更新→新セッションで「カレーライスがお好きだと覚えております」(FR-05/08/09/12 実機確認)
  - 自律会話: 4ターン交互、人格の描き分けあり、両者に記憶5件ずつ (FR-14/15 実機確認)
  - NFR-01 計測: 応答全体 3.6秒 (gpt-oss:20b, 本機)。基準10秒以内を満たす
  - 発見: 導入済みモデルに gemma4:latest もあり
- S12 完了 (2026-07-08): README 作成。最終検証:
  - `cargo test` → 47 passed / 0 failed / 2 ignored(実機テスト・個別実行で成功済み)
  - `npm run build` → 成功 (JS 26KB / CSS 8KB)
  - 修正: integration_test.rs の未使用 import 1件除去

## 未実装 (計画どおりのスコープ外)

- FR-18 エクスポート/インポート (Could) — 設計9章どおり「詳細は実装時」のまま未着手
- FR-19 3体以上の自律会話 (Could) — 拡張点(ターンループの発話者選択)のみ確保

## GUI手動確認項目 (npm run tauri dev で確認)

- FR-01/02 作成画面と一覧表示、FR-05 逐次表示の見た目、EC-01 オンボーディング表示
- FR-16 設定画面の接続確認、NFR-03 非localhost警告ダイアログ
- NFR-01 体感応答 (初回はモデルロードで遅い場合あり)

## 逸脱記録

(なし)

## 発見事項

- Ollama が AMD AI Bundle 由来のパスにあり、標準インストールと異なる。README の手順は標準 Ollama を前提に書き、既存導入でも動く旨を注記する
