/* 对话里的引用：正文角标、落款行、预览浮窗的接线。
   样子在 ui/citation.tsx，这里只管「哪个号是哪条来源」和「去原文是哪条路」。 */
import { createContext, useContext } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, BookOpen } from "lucide-react";
import type { Components } from "react-markdown";
import { api, type Source } from "../api";
import { citeHref } from "../citations";
import { S } from "../i18n";
import { useKbId } from "../kb";
import { originLabel } from "../origin";
import { CiteChip, CiteMark, CiteRow, PreviewCard } from "../ui/citation";

/** 这一轮回答的来源。
 *
 *  **走 context 而不是 prop**：`Segment` 按文本记忆化（一条长答案每来一个词元都要
 *  重渲染，收了尾的段落不该再解析一遍），而来源是一条条到的。把它当 prop 传下去，
 *  每来一条来源所有段落就全部失效；context 只让角标自己重画。 */
const SourcesCtx = createContext<Source[]>([]);
export const SourcesProvider = SourcesCtx.Provider;

/** 给 react-markdown 的 `components`。**必须是模块级常量**：每次渲染现造一个
 *  对象，react-markdown 认为配置变了，`Segment` 的记忆化就白做了 */
export const chatMarkdown: Components = {
  // `node` 是 react-markdown 递进来的 hast 节点，不是 DOM 属性——落到 <a> 上
  // React 会在控制台里抱怨一次
  a: ({ node: _node, href, children, ...rest }) => {
    /* remend 把流式中还没收尾的 `[2` 补成一个链接（`streamdown:incomplete-link`），
       于是一个正在打出来的引用会先闪成一个蓝色的「2」再变成角标。它本来就不是
       链接，按原样的文字画 */
    if (href?.startsWith("streamdown:")) return <>{children}</>;
    const ns = citeHref(href);
    if (!ns) {
      return (
        <a href={href} {...rest}>
          {children}
        </a>
      );
    }
    // `[1, 2]` 是两条来源：画成两个各自可点的角标，别让人点一个号跳出两段原文
    return (
      <>
        {ns.map((n) => (
          <InlineCite key={n} n={n} />
        ))}
      </>
    );
  },
};

function InlineCite({ n }: { n: number }) {
  const sources = useContext(SourcesCtx);
  const s = sources.find((x) => x.n === n);
  // 流式中来源比正文晚到：先画成静的，那条到了自己就变成可点的
  if (!s) return <CiteMark n={n} />;
  return <CiteChip n={n} preview={<SourcePreview s={s} />} />;
}

/** 落款：只列正文引到的那几条（见 citedSources），一个面板装一组 */
export function SourceList({ sources }: { sources: Source[] }) {
  return (
    <div className="mt-2 glass rounded-panel divide-y divide-line">
      {sources.map((s) => (
        <CiteRow key={s.n} preview={<SourcePreview s={s} />}>
          {s.kind === "charter" ? (
            <span className="flex items-center gap-2">
              <span className="u-num text-accent">[{s.n}]</span>
              <BookOpen size={11} className="shrink-0 text-ink-2" />
              <span className="truncate">{charterTitle(s)}</span>
            </span>
          ) : (
            <span className="block truncate">
              <span className="u-num text-accent">[{s.n}]</span> {s.filename} ·{" "}
              {s.excerpt.slice(0, 60)}…
            </span>
          )}
        </CiteRow>
      ))}
    </div>
  );
}

/** 引言节的 heading 就是文章名，避免 "X › X" */
function charterTitle(s: Source): string {
  return s.heading && s.heading !== s.filename
    ? `${s.filename} › ${s.heading}`
    : s.filename;
}

/** 预览：先给检索回来的那段摘录（160 字，立刻有东西看），
 *  整块原文随文档详情到了再换上——同一把 react-query 钥匙，
 *  所以点完「打开原文」那一页是现成的 */
function SourcePreview({ s }: { s: Source }) {
  const kbId = useKbId();
  const doc = useQuery({
    queryKey: ["docDetail", s.document_id],
    queryFn: () => api.documentDetail(s.document_id!),
    enabled: !!s.document_id,
  });
  const chunk = doc.data?.chunks.find((c) => c.id === s.chunk_id);

  if (s.kind === "charter") {
    return (
      <PreviewCard
        title={charterTitle(s)}
        body={s.excerpt}
        action={
          <Link
            to="/docs/$slug"
            params={{ slug: s.slug! }}
            hash={s.anchor || undefined}
            className="u-card-link flex items-center gap-1 text-fine text-ink-2"
          >
            {S.ask.openOriginal}
            <ArrowUpRight size={11} />
          </Link>
        }
      />
    );
  }

  return (
    <PreviewCard
      title={s.filename}
      meta={originLabel(chunk?.origin, chunk?.anchor) ?? undefined}
      body={chunk?.text ?? s.excerpt}
      action={
        <Link
          to="/kb/$kbId/doc/$docId"
          params={{ kbId, docId: s.document_id! }}
          search={{ chunk: s.chunk_id }}
          className="u-card-link flex items-center gap-1 text-fine text-ink-2"
        >
          {S.ask.openOriginal}
          <ArrowUpRight size={11} />
        </Link>
      }
    />
  );
}
