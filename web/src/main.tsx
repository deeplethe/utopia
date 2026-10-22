import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { router } from "./router";
import { ToastHost } from "./toast";
import { DEFAULT_STALE_MS, applyQueryDefaults } from "./queryDefaults";
import { initTheme } from "./theme";
import { ensureDevSession } from "./devAuth";
import "./styles.css";

// 主题先落到 <html> 再渲染：画布第一次读令牌时它得已经是对的
initTheme();

// staleTime 按键分档，表在 queryDefaults.ts；这里只给全局默认（#518）
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
      staleTime: DEFAULT_STALE_MS,
    },
  },
});
applyQueryDefaults(queryClient);

// 开发期先确保会话（见 devAuth.ts），再渲染：Shell 第一次问 `me` 时 cookie 已在，
// 于是一开就落在图谱首页，看不到登录页。生产构建里 ensureDevSession 是空操作。
async function bootstrap() {
  await ensureDevSession();
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <ToastHost />
      </QueryClientProvider>
    </StrictMode>,
  );
}

void bootstrap();
