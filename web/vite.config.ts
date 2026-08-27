import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    // 多会话并行开发：PORT 由启动器分配（5173 被占时换口），未设时保持默认
    port: process.env.PORT ? Number(process.env.PORT) : 5173,
    proxy: {
      // 后端端口可用 UTOPIA_DEV_API 覆盖（默认 18080，与 .env 的 UTOPIA_BIND_ADDR 一致）
      "/api": process.env.UTOPIA_DEV_API ?? "http://127.0.0.1:18080",
    },
  },
});
