/* 内置文档：随应用打包（私有部署离线可用），版本与部署一致。
   公开路由——不依赖会话，登录前也可读（开发者接接口时未必有账号）。 */
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { Command, Search } from "lucide-react";
import { api } from "../api";
import { S } from "../i18n";
import {
  Button,
  cn,
  GithubMark,
  Input,
  Row,
  rowClass,
  SectionMark,
} from "../ui";
import { UserMenu } from "./UserMenu";
import { usePageTitle } from "../useTitle";
import ingestMd from "../docs/ingest.md?raw";
import mcpMd from "../docs/mcp.md?raw";

/** 文档清单：slug → 标题 + 内容（构建期打进 bundle） */
const DOCS: { slug: string; title: string; body: string }[] = [
  { slug: "ingest", title: "Ingest interfaces", body: ingestMd },
  { slug: "mcp", title: "Agents over MCP", body: mcpMd },
];

/** 全文检索（客户端，文档已在 bundle 里）：命中行 → 文档 + 以命中词为中心的摘录 */
function searchDocs(q: string): { slug: string; title: string; snippet: string }[] {
  const needle = q.trim().toLowerCase();
  if (needle.length < 2) return [];
  const hits: { slug: string; title: string; snippet: string }[] = [];
  for (const d of DOCS) {
    for (const line of d.body.split("\n")) {
      const cleaned = line.replace(/[#`*|>-]/g, " ").replace(/\s+/g, " ").trim();
      const idx = cleaned.toLowerCase().indexOf(needle);
      if (idx < 0) continue;
      // 摘录窗口以命中词为中心，避免命中被 90 字截断截丢
      const start = Math.max(0, idx - 30);
      const snippet = (start > 0 ? "…" : "") + cleaned.slice(start, start + 100);
      hits.push({ slug: d.slug, title: d.title, snippet });
      if (hits.length >= 8) return hits;
    }
  }
  return hits;
}

/** 摘录内高亮命中词（不区分大小写，全部命中都标） */
function Highlighted({ text, q }: { text: string; q: string }) {
  const needle = q.trim();
  if (!needle) return <>{text}</>;
  const parts = text.split(new RegExp(`(${needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "ig"));
  return (
    <>
      {parts.map((p, i) =>
        p.toLowerCase() === needle.toLowerCase() ? (
          /* 命中词用警示琥珀：结果列表里一眼定位到匹配处 */
          <mark
            key={i}
            className="u-mark"
          >
            {p}
          </mark>
        ) : (
          <span key={i}>{p}</span>
        ),
      )}
    </>
  );
}

/** 标题 → 锚点 id（TOC 与正文用同一函数保证对得上） */
function slugify(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9一-龥]+/g, "-")
    .replace(/(^-+|-+$)/g, "");
}

/** React children → 纯文本（h2/h3 里可能嵌 code/strong） */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function textOf(children: any): string {
  if (typeof children === "string") return children;
  if (Array.isArray(children)) return children.map(textOf).join("");
  if (children && typeof children === "object" && "props" in children)
    return textOf(children.props.children);
  return "";
}

/** 从 markdown 源提取 h2/h3 目录 */
function tocOf(body: string): { id: string; text: string; level: 2 | 3 }[] {
  const out: { id: string; text: string; level: 2 | 3 }[] = [];
  for (const line of body.split("\n")) {
    const m = /^(#{2,3})\s+(.+)$/.exec(line);
    if (!m) continue;
    const text = m[2].replace(/[`*]/g, "").trim();
    out.push({ id: slugify(text), text, level: m[1].length as 2 | 3 });
  }
  return out;
}

const IS_MAC = navigator.platform.toUpperCase().includes("MAC");

export function DocsPage() {
  const { slug } = useParams({ from: "/docs/$slug" });
  const doc = DOCS.find((d) => d.slug === slug) ?? DOCS[0];
  const navigate = useNavigate();
  const [q, setQ] = useState("");
  const searchRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const results = searchDocs(q);
  const toc = tocOf(doc.body);
  const mainRef = useRef<HTMLElement>(null);
  const [activeHeading, setActiveHeading] = useState<string | null>(null);
  // 标题：`Utopia | {文章名}`——Charter 是门脸字标，标题直接给文章
  usePageTitle(S.app.name, doc.title);
  // 公开页也感知登录态：已登录给用户菜单，未登录给 Sign in
  const me = useQuery({ queryKey: ["me"], queryFn: api.me, retry: false });
  const health = useQuery({ queryKey: ["health"], queryFn: api.health, staleTime: Infinity });

  // 滚动跟随：视口上沿之上最近的标题为当前小节；
  // 滚到底强制激活最后一节（短末节永远越不过判定线）
  const onScrollSpy = () => {
    const main = mainRef.current;
    if (!main) return;
    const top = main.getBoundingClientRect().top;
    let current: string | null = null;
    for (const h of toc) {
      const el = document.getElementById(h.id);
      if (el && el.getBoundingClientRect().top - top <= 96) current = h.id;
    }
    if (main.scrollTop + main.clientHeight >= main.scrollHeight - 8 && toc.length)
      current = toc[toc.length - 1].id;
    setActiveHeading(current);
  };

  useEffect(() => {
    if (!q) return;
    const onDown = (e: MouseEvent) => {
      if (searchRef.current && !searchRef.current.contains(e.target as Node)) setQ("");
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setQ("");
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [q]);

  // 键盘呼起：⌘K / Ctrl+K 任何时候生效；`/` 仅在焦点不在输入框时（别劫持打字）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const typing =
        el instanceof HTMLInputElement ||
        el instanceof HTMLTextAreaElement ||
        (el instanceof HTMLElement && el.isContentEditable);
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      } else if (e.key === "/" && !typing) {
        e.preventDefault();
        inputRef.current?.focus();
      } else if (e.key === "Escape" && el === inputRef.current) {
        inputRef.current?.blur();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="h-screen flex flex-col overflow-hidden">
      <header className="glass-strong relative z-40 border-x-0 border-t-0 h-14 shrink-0 flex items-center px-6">
        <SectionMark text={S.docs.brand} title={S.docs.backTitle} />
        {/* 居中搜索：检索打包在本地的全部文档；宽度对齐正文栏（max-w-3xl 去掉 px-8） */}
        <div
          ref={searchRef}
          className="absolute left-1/2 -translate-x-1/2 w-[min(44rem,55vw)]"
        >
          <div className="relative">
            <Input
              size="sm"
              className="w-full"
              icon={<Search size={13} />}
              ref={inputRef}
              placeholder={S.docs.searchPlaceholder}
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
            {/* 快捷键提示：输入中不占视线。⌘ 用 lucide 图标；
                Ctrl 无图形符号（⌘ 是 Mac 专属键符），Windows 规范写法就是文字 */}
            {!q && (
              <kbd className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 flex items-center gap-1 rounded-lg border border-line bg-surface px-2 py-1 font-sans text-fine text-ink-3">
                {IS_MAC ? <Command size={9} /> : <span>Ctrl</span>}
                <span>K</span>
              </kbd>
            )}
          </div>
          {q.trim().length >= 2 && (
            <div className="u-menu-glass u-pop-in u-pop-in-tl absolute inset-x-0 top-full mt-2 rounded-xl shadow-2xl overflow-hidden">
              {results.length === 0 ? (
                <p className="px-4 py-3 text-small text-ink-3">{S.docs.noResults}</p>
              ) : (
                results.map((r, i) => (
                  <Row
                    key={i}
                    density="menu"
                    className="border-b border-line px-4 py-3 last:border-0"
                    onClick={() => {
                      setQ("");
                      navigate({ to: "/docs/$slug", params: { slug: r.slug } });
                    }}
                  >
                    <div className="text-fine text-ink-3">{r.title}</div>
                    <div className="text-body text-ink truncate">
                      <Highlighted text={r.snippet} q={q} />
                    </div>
                  </Row>
                ))
              )}
            </div>
          )}
        </div>

        {/* 右侧：显式返回（与字标双路回城）+ GitHub·版本胶囊 + 登录态 */}
        <div className="ml-auto flex items-center gap-2">
          <Link
            to="/"
            className="u-navlink"
          >
            {S.account.backToApp}
          </Link>
          <a
            href={S.login.githubUrl}
            target="_blank"
            rel="noreferrer"
            title="GitHub"
            className="u-pill"
          >
            <GithubMark size={13} />
            {health.data && <span className="u-num text-fine">v{health.data.version}</span>}
          </a>
          {me.data ? (
            <div className="ml-1">
              <UserMenu user={me.data} />
            </div>
          ) : me.isError ? (
            <Link
              to="/login"
              className="u-btn u-btn-ghost px-3 py-2 text-small"
            >
              {S.login.signIn}
            </Link>
          ) : null}
        </div>
      </header>

      {/* 整页滚动（滚动条贴窗口最右），侧栏 sticky 钉住——正规文档站布局 */}
      <main
        ref={mainRef}
        onScroll={onScrollSpy}
        className="flex-1 min-h-0 overflow-y-auto u-scroll"
      >
        <div className="flex items-start">
          <aside className="w-64 shrink-0 sticky top-0 h-[calc(100vh-3.5rem)] glass-strong border-y-0 border-l-0 p-3 space-y-1">
            {DOCS.map((d) => (
              <Link
                key={d.slug}
                to="/docs/$slug"
                params={{ slug: d.slug }}
                className={rowClass(d.slug === doc.slug, "nav")}
              >
                {d.title}
              </Link>
            ))}
          </aside>

          {/* 上下都留足空白：标题不顶着头，末段内容能滚到屏幕中部——人读屏幕中间。
              排版交给官方 @tailwindcss/typography（prose，16px 基准），
              自定义只剩：标题锚点 id、外链新开、表格横向滚动容器 */}
          {/* prose-neutral：默认 gray 阶带蓝相（oklch 258°），违反 chrome 零色偏 */}
          <article className="prose prose-neutral prose-invert prose-headings:scroll-mt-6 prose-code:before:content-none prose-code:after:content-none flex-1 min-w-0 max-w-3xl mx-auto px-8 pt-16 pb-[40vh]">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={{
                h2: ({ children, ...p }) => (
                  <h2 id={slugify(textOf(children))} {...p}>
                    {children}
                  </h2>
                ),
                h3: ({ children, ...p }) => (
                  <h3 id={slugify(textOf(children))} {...p}>
                    {children}
                  </h3>
                ),
                a: (p) => <a target="_blank" rel="noreferrer" {...p} />,
                // 不加 u-scroll：overscroll-contain 会把竖向滚轮困在块内，页面滚不动
                table: (p) => (
                  <div className="overflow-x-auto">
                    <table {...p} />
                  </div>
                ),
              }}
            >
              {doc.body}
            </ReactMarkdown>
          </article>

          {/* 右侧目录：sticky 钉住，与正文标题同一水平线起头 */}
          {toc.length > 0 && (
            <aside className="hidden lg:block w-64 shrink-0 sticky top-0 pt-16 pr-8">
              <nav className="space-y-1">
                {toc.map((h) => (
                  <Button variant="ghost" size="sm"
                    key={h.id}
                    onClick={() =>
                      document.getElementById(h.id)?.scrollIntoView({ behavior: "smooth" })
                    }
                    className={cn(
                      "h-auto w-full justify-start whitespace-normal py-2 text-left leading-snug",
                      h.level === 3 && "pl-6",
                      activeHeading === h.id ? "text-ink" : "text-ink-3",
                    )}
                  >
                    {h.text}
                  </Button>
                ))}
              </nav>
            </aside>
          )}
        </div>
      </main>
    </div>
  );
}
