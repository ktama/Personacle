import { defineConfig } from "vite";

// Tauri 開発サーバー設定: ポート固定 (tauri.conf.json の devUrl と一致させる)
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
