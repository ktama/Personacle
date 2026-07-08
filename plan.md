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

---

## FR-19: 3体以上の自律会話 (2026-07-08 着手)

ベースライン: `cargo test` 47 passed / `npm run build` 成功 (変更前に確認済み)

### 設計判断 (ADR-08 として design.md に追記)

- 発話順: **ラウンドロビン** (参加者の登録順で巡回)。受け入れ基準「全員が1回以上発話」を決定的に満たす
- 参加数上限: **6体** (コンテキスト予算と後処理コストの上限として)
- 後処理: 関係性評定は「自分以外の各参加者」ごとに実行。**性格軸デルタの適用はセッションあたり1回のみ**
  (相手ごとに適用すると FR-12 の1セッション変化量上限±2を実質超えてしまうため)

### ステップ (FR-19)

- [ ] F1: prompt.rs — build_system を複数相手対応 (PartnerInfo 配列)。既存テスト調整+複数相手テスト
      検証: `cargo test prompt`
- [ ] F2: conversation.rs — 参加数検証(2〜6)、generate_reply が相手一覧をセッションから導出、
      ターンループをラウンドロビン化。テスト: 3体巡回(FR-19受け入れ基準)、7体/1体の拒否
      検証: `cargo test conversation`
- [ ] F3: worker.rs — 相手ごとの関係性評定ループ + 性格デルタは1回のみ適用。テスト: 3体後処理
      検証: `cargo test worker`
- [ ] F4: フロント autonomous.ts — 2択セレクトをチェックボックス複数選択(2〜6)に変更
      検証: `npm run build`
- [ ] F5: design.md — ADR-08 追記、7章フロー2とトレーサビリティ表の FR-19 行を更新
      検証: 目視 (トレーサビリティ行の整合)
- [ ] F6: 全テスト + 完了検証
      検証: `cargo test` 全件 / `npm run build`

### 受け入れ基準→テスト対応 (FR-19)

| 基準 | テスト |
| --- | --- |
| 3体指定で開始→全員が1回以上発話 | conversation::three_personas_round_robin (モック) |
| (FR-12 整合) 多相手でも性格変化がセッション上限内 | worker::postprocess_three_participants |

### 進捗記録 (FR-19)

- F1+F2 完了: prompt.rs (PartnerInfo 複数相手)、conversation.rs (2〜6体検証・相手導出・ラウンドロビン)。
  同一クレートのため一括検証: `cargo test` → 51 passed (+4: multiple_partners_listed_fr19,
  three_personas_round_robin_fr19, autonomous_participant_count_validated_fr19, postprocess_three_participants)
- F3 完了: worker.rs 相手ごとの評定ループ + 性格デルタ1回適用 (同上のテストで検証)
- F4 完了: autonomous.ts チェックボックス複数選択 + styles.css。`npm run build` 成功
- F5 完了: design.md v1.1 (ADR-08 追加、フロー2一般化、FR-19 トレーサビリティ更新)
- F6 完了 (2026-07-08): `cargo test` **51 passed / 0 failed**、`npm run build` 成功。
  diff 通読: 変更8ファイルすべて F1〜F5 に対応、無関係な変更なし

### 逸脱記録 (FR-19)

- 性格軸デルタを相手ごとに適用すると FR-12 の上限を超えるため「セッションあたり1回のみ適用」とする。
  分類: 実装の自由範囲 (FR-12 の上限保証を優先する解釈)。ADR-08 に明記

---

## FR-18: ペルソナのエクスポート/インポート (2026-07-08 着手)

ベースライン: `cargo test` 51 passed / `npm run build` 成功 (FR-19 完了時点で確認済み)

### 実装内の判断 (設計トレーサビリティ表 FR-18 行が委ねた詳細)

- ファイル形式: JSON 1ファイル。`format: "personacle-persona"` + `formatVersion: 1` + `appSchemaVersion` を記録
- **埋め込みベクトルは書き出さない**。インポート時に NULL とし Reembed ジョブで再計算
  (移行先の埋め込みモデルが異なる可能性があり、含めても互換性がないため)
- ID はインポート時に全て新規発行し、旧ID→新IDのマップで参照(記憶の出所セッション、発話者)を張り替える
- 会話履歴を含めた場合、インポートされたセッションは status=processed・参加者処理済みとして取り込む
  (後処理の再実行で記憶が二重生成されるのを防ぐ)
- 履歴に含まれる「他ペルソナ」への参照は名前スナップショットで保持 (EC-07 の削除済み表示と同じ扱い)
- 同名ペルソナが既存なら EC-04 と同様に duplicate_name 警告 → force で取込
- ファイルダイアログは tauri-plugin-dialog (公式プラグイン) を追加して使う

### ステップ (FR-18)

- [ ] G1: export.rs 新設 — ExportFile 構造体 (serde)、build_export / import_value /
      export_to_file / import_from_file。テスト: 履歴あり/なしラウンドトリップ、同名警告→force、
      不正形式・未来バージョン拒否、Reembed ジョブ投入、ファイル経由ラウンドトリップ
      検証: `cargo test export`
- [ ] G2: コマンド追加 (export_persona / import_persona) + tauri-plugin-dialog の Rust/JS 依存・
      capability 追加 + lib.rs 配線
      検証: `cargo test` (全件) — コマンドは薄いラッパのため既存+G1 テストで担保
- [ ] G3: フロント — ペルソナ編集タブにエクスポート(履歴含む/含まないの選択付き)、
      サイドバーにインポート。duplicate_name は確認→force 再送
      検証: `npm run build`
- [ ] G4: docs/design.md — ADR-09 (エクスポート形式と埋め込み除外) 追記、FR-18 トレーサビリティ更新、
      6.1 コマンド表に2行追加。README に共有機能の記載
      検証: 目視
- [ ] G5: 全テスト + diff 通読
      検証: `cargo test` 全件 / `npm run build`

### 受け入れ基準→テスト対応 (FR-18)

| 基準 | テスト |
| --- | --- |
| エクスポート→別環境でインポートで初期設定・人格・記憶が再現 | export::roundtrip_via_file (別DBの ctx へ取込) |
| 会話履歴を含むかは選択可 | export::roundtrip_without_history / roundtrip_with_history |

### 進捗記録 (FR-18)

- G1 完了: export.rs (ExportFile/build_export/import_value/ファイルIO)。`cargo test export` → 5 passed
  (履歴あり/なしラウンドトリップ、同名→force、不正形式・未来バージョン・壊れたファイル拒否、ファイル経由)
- G2 完了: export_persona/import_persona コマンド、tauri-plugin-dialog (Rust/JS/capability)、lib.rs 配線。
  `cargo test` → 56 passed
- G3 完了: 編集タブにエクスポート(履歴含む選択付き)、サイドバーに「ファイルから取り込む」。
  `npm run build` 成功。修正1回: dialog の save import がフォーム内ローカル関数 save と衝突 → saveFileDialog に改名
- G4 完了: design.md v1.2 (ADR-09、6.1 コマンド表、FR-18 トレーサビリティ)、README 機能追記+lint修正
- G5 完了 (2026-07-08): `cargo test` **56 passed / 0 failed**、`npm run build` 成功。
  diff 通読: 変更15ファイル+新規1、すべて G1〜G4 に対応

### 逸脱記録 (FR-18)

(なし。ADR-09 の判断は「実装内の判断」節のとおり設計が委ねた詳細の範囲)

---

## D-1: 推奨モデルの確定 (2026-07-08 着手)

- 候補: gpt-oss:20b (20B, 13GB, 推論型) / gemma4:latest (8B, Q4_K_M, 9.6GB, 131kコンテキスト)
- 計測ハーネス: src-tauri/src/poc_test.rs (poc_model_bench, #[ignore], PERSONACLE_POC_MODEL で切替)
- 計測項目: ロード時間 / 初トークン・全体時間の中央値3回 (NFR-01) / 口調サンプル /
  「知らない」正直さ5回 (FR-09簡易版) / 記憶抽出JSON成功率5回 (R-3)

### 計測結果 (D-1)

**gemma4:latest (8B, Q4_K_M)** — 2026-07-08 本機実測:

- ロード+初回応答 4.1秒
- 初トークン中央値 **2.6秒** / 全体中央値 **3.1秒** (NFR-01 基準 10秒/60秒を大幅クリア)
- 口調維持: 「〜なのです」「〜ですのよ」を完全に維持
- 知らないことへの正直さ: **5/5**
- 記憶抽出JSON: **成功 5/5**、平均2.6件/回

**gpt-oss:20b (20B, 13GB)** — 2026-07-08 本機実測:

- ロード+初回応答 16.7秒 (gemma4の4倍)
- 初トークン中央値 2.7秒 / 全体中央値 3.4秒 (速度はほぼ同等。ただし本機はAMD GPUあり)
- 口調維持: 概ね維持するが不自然な混成が散見 (「くださいなのです」「伺いますですのよ」)
- 知らないことへの正直さ: ハーネス判定は3/5だが、**出力の目視では5/5全て正直**
  (「知り得ません」「覚えておりません」がマーカー語に一致しなかった判定漏れ。ハーネスの限界として記録)
- 記憶抽出JSON: 成功 5/5、平均4.2件/回 (抽出は gpt-oss の方が網羅的)

### 判定 (D-1)

**推奨モデル: gemma4:latest** に決定。根拠:

- 日本語の自然さ・口調維持で明確に優位 (gpt-oss は文法的に不自然な語尾混成あり)
- ロードが4倍速く、ファイルサイズ 9.6GB vs 13GB でメモリ要件に余裕
- 正直さ・JSON抽出は両者とも実用水準。生成速度は同等
- gpt-oss:20b はメモリに余裕がある環境の代替として README に記載

留意: 本機は AMD GPU 環境。要件9-3 (GPUなし16GB での NFR-01 実測) は未検証のまま残る

### 発見事項 (D-1 計測中)

- **thinking対応モデル(gemma4等)では max_tokens が思考トークンに食われ、本文が空になることがある**。
  PoC初回実行で 256 トークン指定時に 3回中2回が空応答。ハーネスは 1024 に引き上げて解消。
  製品側の conversation.rs は max_tokens=512 のため同じ問題が起きうる → 対処要検討
  (max_tokens引き上げ、またはOllamaのthinking無効化オプション)。今回のD-1スコープ外として報告のみ

## GUI手動確認項目 (npm run tauri dev で確認)

- FR-01/02 作成画面と一覧表示、FR-05 逐次表示の見た目、EC-01 オンボーディング表示
- FR-16 設定画面の接続確認、NFR-03 非localhost警告ダイアログ
- NFR-01 体感応答 (初回はモデルロードで遅い場合あり)

## 逸脱記録

(なし)

## 発見事項

- Ollama が AMD AI Bundle 由来のパスにあり、標準インストールと異なる。README の手順は標準 Ollama を前提に書き、既存導入でも動く旨を注記する
