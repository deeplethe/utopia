import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";

/* lucide 已移除品牌图标，GitHub mark 内联（官方 mark 路径，fill=currentColor） */
function GithubMark({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  );
}
import { api, ApiError } from "../api";
import { S } from "../i18n";
import { Wordmark } from "../ui";
import { LoginScene } from "./LoginScene";
import { usePageTitle } from "../useTitle";

export function Login() {
  usePageTitle(S.app.name, S.login.signIn);
  const navigate = useNavigate();
  const [mode, setMode] = useState<"login" | "register">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [leaving, setLeaving] = useState(false);

  const mutation = useMutation({
    mutationFn: async () => {
      if (mode === "login") return api.login(email, password);
      return api.register(email, password, displayName);
    },
    onSuccess: () => {
      // 谢幕：卡片上浮淡出、巨构放大穿越，再进入图谱首页
      setLeaving(true);
      window.setTimeout(() => navigate({ to: "/graph" }), 650);
    },
  });

  const error =
    mutation.error instanceof ApiError
      ? mutation.error.message
      : mutation.error
        ? S.login.networkError
        : null;

  const input = "input-dark w-full px-3 py-2 text-sm";

  return (
    <div className="min-h-screen flex items-center justify-center px-4">
      {/* 巨构变换背景：星球 → 环形都市 → 城市平原 → 波动巨碑 */}
      <LoginScene leaving={leaving} />
      <div className={`relative z-10 w-full max-w-sm ${leaving ? "u-depart" : ""}`}>
        <div className="mb-8 text-center u-rise">
          <h1 className="text-5xl font-normal">
            <Wordmark />
          </h1>
          <p className="mt-2 text-sm text-neutral-400">{S.app.tagline}</p>
        </div>

        <div className="u-card-opaque rounded-2xl p-6 u-rise" style={{ animationDelay: "90ms" }}>
          <div className="flex gap-1 mb-5 bg-white/5 rounded-lg p-1">
            {(["login", "register"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`flex-1 rounded-md py-1.5 text-sm font-medium transition-colors ${
                  mode === m
                    ? "bg-white/10 text-neutral-100"
                    : "text-neutral-500 hover:text-neutral-300"
                }`}
              >
                {m === "login" ? S.login.signIn : S.login.signUp}
              </button>
            ))}
          </div>

          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              mutation.mutate();
            }}
          >
            {mode === "register" && (
              <input
                className={input}
                placeholder={S.login.displayName}
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                required
              />
            )}
            <input
              type="email"
              className={input}
              placeholder={S.login.email}
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              required
            />
            <input
              type="password"
              className={input}
              placeholder={S.login.password}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              minLength={8}
            />
            {error && <p className="text-sm text-rose-400">{error}</p>}
            <button
              type="submit"
              disabled={mutation.isPending || leaving}
              className="btn-primary w-full py-2 text-sm"
            >
              {mutation.isPending || leaving
                ? S.login.submitting
                : mode === "login"
                  ? S.login.signIn
                  : S.login.createAccount}
            </button>
          </form>
        </div>

        {/* 页脚：惯用同意句式内嵌条款/隐私链接 + GitHub 入口 */}
        <div className="mt-6 text-center u-rise" style={{ animationDelay: "180ms" }}>
          <p className="u-balance text-[11px] leading-relaxed text-neutral-600">
            {S.login.agreePrefix}
            <Link
              to="/terms"
              className="whitespace-nowrap text-neutral-500 underline decoration-white/20 underline-offset-2 hover:text-neutral-300 transition-colors"
            >
              {S.legal.termsTitle}
            </Link>
            {S.login.agreeAnd}
            <Link
              to="/privacy"
              className="whitespace-nowrap text-neutral-500 underline decoration-white/20 underline-offset-2 hover:text-neutral-300 transition-colors"
            >
              {S.legal.privacyTitle}
            </Link>
            {S.login.agreeSuffix}
          </p>
          <a
            href={S.login.githubUrl}
            target="_blank"
            rel="noreferrer"
            title="GitHub"
            className="mt-3 inline-flex text-neutral-600 hover:text-neutral-300 transition-colors"
          >
            <GithubMark size={16} />
          </a>
        </div>
      </div>
    </div>
  );
}
