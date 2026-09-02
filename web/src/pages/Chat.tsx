/* Chat：agentic 对话（检索/图谱工具 + remember 记忆）。
   会话持久化：左栏会话列表;上下文由服务端拼,前端只发 conversation_id + 新消息;
   行动轨迹(steps)与引用(sources)随消息落库,历史回放与实时流共用渲染。 */
import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import remend from "remend";
import {
  ArrowUp,
  BookOpen,
  Check,
  ChevronDown,
  Database,
  GitCompareArrows,
  History,
  Search as SearchIcon,
  Square,
  SquarePen,
  MoreHorizontal,
  Waypoints,
  Wrench,
} from "lucide-react";
import { ThinkingOrb, type OrbState } from "thinking-orbs";
import {
  conversationsApi,
  streamChat,
  type ChatStep,
  type ConversationRow,
  type Source,
} from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import { useKb, useKbId } from "../kb";
import { DangerConfirm, RAIL_CLS } from "../ui";

interface Turn {
  role: "user" | "assistant";
  content: string;
  sources?: Source[];
  steps?: ChatStep[];
  error?: string;
}

/** 同标签页记忆：上次会话（按库）与未发送草稿——切页回来还原，新标签页从头开始 */
const lastKey = (kbId: string) => `chat:last:${kbId}`;
const DRAFT_KEY = "chat:draft";

export function Chat() {
  const kbId = useKbId();
  const { kb, kbs, setKb } = useKb();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  // 会话即路由：/chat/$conversationId，URL 是当前会话的唯一事实来源（刷新/回退天然可用）
  const { conversationId: routeConvId } = useParams({ strict: false }) as {
    conversationId?: string;
  };
  const [activeId, setActiveId] = useState<string | null>(null);
  // 路由同步 effect 的判据：state 的提交时序晚于 navigate 触发的重渲染，
  // 用 ref 同步写入才能让"流式新建后仅换 URL"的守卫可靠命中
  const activeIdRef = useRef<string | null>(null);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState(() => sessionStorage.getItem(DRAFT_KEY) ?? "");
  const [streaming, setStreaming] = useState(false);
  const [scopeOpen, setScopeOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<ConversationRow | null>(null);
  // 会话搜索。**搜标题也搜正文**——人记得住的往往是问过的那句话
  const [convSearch, setConvSearch] = useState("");
  // 三点菜单展开的是哪一条。同时只开一个
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const abortRef = useRef<(() => void) | null>(null);
  const scopeRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // 作用域弹层：点外面 / Esc 关闭（与 ui/Dropdown 同惯例）
  useEffect(() => {
    if (!scopeOpen) return;
    const onDoc = (e: MouseEvent) => {
      if (!scopeRef.current?.contains(e.target as Node)) setScopeOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setScopeOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [scopeOpen]);

  const convs = useQuery({
    queryKey: ["conversations", kb?.id, convSearch],
    queryFn: () => conversationsApi.list(kb!.id, convSearch),
    enabled: !!kb,
    placeholderData: (prev) => prev,
  });
  // 改标题：**就地编辑**，不弹对话框——改一个名字不值得打断整页
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const rename = useMutation({
    mutationFn: (v: { id: string; title: string }) =>
      conversationsApi.rename(kb!.id, v.id, v.title),
    onSuccess: () => {
      setRenamingId(null);
      queryClient.invalidateQueries({ queryKey: ["conversations", kb?.id] });
    },
    onError: (e: Error) => toast.error(e.message),
  });

  // 直落底部（instant）：平滑滚动在流式追加下会一路慢爬
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "instant" });
  }, [turns]);

  // 切库回到新会话（首次拿到 kb 不算切换——直刷 /chat/$id 时不能把 URL 冲掉）
  const prevKbRef = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevKbRef.current;
    prevKbRef.current = kb?.id ?? null;
    if (prev && kb && prev !== kb.id) {
      if (streaming) abortRef.current?.();
      setStreaming(false);
      activeIdRef.current = null;
      setActiveId(null);
      setTurns([]);
      navigate({ to: "/kb/$kbId/chat", params: { kbId }, replace: true });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kb?.id]);

  // 路由 → 会话装载；裸 /chat 还原本库上次会话（切页回来仍在原对话）
  useEffect(() => {
    if (!kb) return;
    if (!routeConvId) {
      const last = sessionStorage.getItem(lastKey(kb.id));
      if (last) {
        navigate({
          to: "/kb/$kbId/chat/$conversationId",
          params: { kbId, conversationId: last },
          replace: true,
        });
      }
      return;
    }
    if (routeConvId === activeIdRef.current) return; // 流式新建会话后仅 URL 同步，勿重载
    loadConversation(routeConvId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kb?.id, routeConvId]);

  // 还原的草稿撑开输入框（高度平时由 onChange 维护）
  useEffect(() => {
    const el = inputRef.current;
    if (el && el.value) {
      el.style.height = "auto";
      el.style.height = `${Math.min(el.scrollHeight, 192)}px`;
    }
  }, []);

  const invalidateList = () =>
    queryClient.invalidateQueries({ queryKey: ["conversations", kb?.id] });

  /** 列表点击只改 URL，装载由路由同步 effect 负责 */
  const openConversation = (id: string) =>
    navigate({
      to: "/kb/$kbId/chat/$conversationId",
      params: { kbId, conversationId: id },
    });

  const loadConversation = async (id: string) => {
    if (streaming) abortRef.current?.();
    setStreaming(false);
    activeIdRef.current = id;
    setActiveId(id);
    try {
      const { messages } = await conversationsApi.detail(kb!.id, id);
      sessionStorage.setItem(lastKey(kb!.id), id);
      setTurns(
        messages.map((m) => ({
          role: m.role,
          content: m.content,
          steps: m.steps.length ? m.steps : undefined,
          sources: m.sources.length ? m.sources : undefined,
        })),
      );
    } catch {
      // 失效链接（会话已删 / 属于别的库）：安静回到新对话
      sessionStorage.removeItem(lastKey(kb!.id));
      activeIdRef.current = null;
      setActiveId(null);
      setTurns([]);
      navigate({ to: "/kb/$kbId/chat", params: { kbId }, replace: true });
    }
  };

  const newChat = () => {
    if (streaming) abortRef.current?.();
    setStreaming(false);
    if (kb) sessionStorage.removeItem(lastKey(kb.id));
    activeIdRef.current = null;
    setActiveId(null);
    setTurns([]);
    navigate({ to: "/kb/$kbId/chat", params: { kbId } });
    inputRef.current?.focus();
  };

  const removeConversation = async (id: string) => {
    await conversationsApi.remove(kb!.id, id);
    if (sessionStorage.getItem(lastKey(kb!.id)) === id) {
      sessionStorage.removeItem(lastKey(kb!.id));
    }
    invalidateList();
    if (id === activeId) newChat();
  };

  const send = () => {
    const q = input.trim();
    if (!q || streaming || !kb) return;
    setInput("");
    sessionStorage.removeItem(DRAFT_KEY);
    if (inputRef.current) inputRef.current.style.height = "auto";
    setTurns((prev) => [...prev, { role: "user", content: q }, { role: "assistant", content: "" }]);
    setStreaming(true);

    abortRef.current = streamChat(
      kb.id,
      { conversation_id: activeId ?? undefined, message: q },
      {
        onConversation: (id) => {
          // 先同步写 ref 再换 URL：路由同步 effect 因 id 相等而跳过重载，不打断流
          activeIdRef.current = id;
          setActiveId(id);
          sessionStorage.setItem(lastKey(kb.id), id);
          navigate({
            to: "/kb/$kbId/chat/$conversationId",
            params: { kbId, conversationId: id },
            replace: true,
          });
          invalidateList();
        },
        onSources: (sources) =>
          setTurns((prev) => {
            const next = [...prev];
            next[next.length - 1] = { ...next[next.length - 1], sources };
            return next;
          }),
        onStep: (step) =>
          setTurns((prev) => {
            const next = [...prev];
            const last = next[next.length - 1];
            next[next.length - 1] = { ...last, steps: [...(last.steps ?? []), step] };
            return next;
          }),
        onDelta: (text) =>
          setTurns((prev) => {
            const next = [...prev];
            const last = next[next.length - 1];
            next[next.length - 1] = { ...last, content: last.content + text };
            return next;
          }),
        onDone: () => {
          setStreaming(false);
          invalidateList();
        },
        onError: (message) => {
          setStreaming(false);
          setTurns((prev) => {
            const next = [...prev];
            next[next.length - 1] = { ...next[next.length - 1], error: message };
            return next;
          });
        },
      },
    );
  };

  /* Composer 卡：新对话首屏居中出场，进入对话后停靠底部（同一块 JSX 两处复用） */
  const composerCard = (
    <div className="rounded-2xl border border-white/[0.12] bg-white/[0.04] backdrop-blur-md focus-within:border-white/30 transition-colors px-4 pt-3 pb-2">
      <textarea
        ref={inputRef}
        rows={1}
        className="w-full bg-transparent outline-none text-sm resize-none leading-relaxed max-h-48 u-scroll placeholder:text-neutral-600"
        placeholder={S.ask.placeholder}
        value={input}
        onChange={(e) => {
          setInput(e.target.value);
          sessionStorage.setItem(DRAFT_KEY, e.target.value);
          const el = e.currentTarget;
          el.style.height = "auto";
          el.style.height = `${Math.min(el.scrollHeight, 192)}px`;
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div className="flex items-center justify-between gap-3 pt-1">
        <div className="flex items-center gap-2.5 min-w-0">
          {/* 作用域 chip：提问点位可见"在问哪个库"，切库沿用现有语义（开新会话） */}
          <div ref={scopeRef} className="relative shrink-0">
            <button
              onClick={() => setScopeOpen((v) => !v)}
              title={S.ask.scopeLabel}
              className="flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs text-neutral-400 hover:text-neutral-200 hover:bg-white/[0.07] transition-colors max-w-52"
            >
              <Database size={12} className="shrink-0 text-neutral-500" />
              <span className="truncate">{kb?.name ?? "…"}</span>
              <ChevronDown
                size={11}
                className={`shrink-0 text-neutral-600 transition-transform ${
                  scopeOpen ? "rotate-180" : ""
                }`}
              />
            </button>
            {scopeOpen && (
              <div className="u-pop u-pop-up absolute bottom-full mb-1.5 left-0 z-50 w-56 rounded-lg shadow-xl overflow-hidden">
                <div className="px-2.5 pt-2 pb-1 text-[9.5px] font-medium uppercase tracking-[0.1em] text-neutral-600 border-b border-white/5">
                  {S.ask.scopeLabel}
                </div>
                <div className="u-scroll max-h-60 overflow-y-auto">
                  {kbs.map((k) => (
                    <button
                      key={k.id}
                      onClick={() => {
                        setScopeOpen(false);
                        if (k.id !== kb?.id) setKb(k.id);
                      }}
                      className={`w-full flex items-center gap-2 text-left px-2.5 py-1.5 text-xs ${
                        k.id === kb?.id
                          ? "bg-white/[0.12] text-white"
                          : "text-neutral-300 hover:bg-white/[0.06] hover:text-white"
                      }`}
                    >
                      <span className="flex-1 min-w-0 truncate">{k.name}</span>
                      {k.id === kb?.id && <Check size={12} className="shrink-0 text-neutral-400" />}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
          <span className="text-[11px] text-neutral-600 truncate">{S.ask.composerHint}</span>
        </div>
        {streaming ? (
          <button
            onClick={() => {
              abortRef.current?.();
              setStreaming(false);
            }}
            title={S.ask.stop}
            className="h-8 w-8 shrink-0 rounded-lg grid place-items-center bg-white/[0.08] text-neutral-200 hover:bg-white/[0.14] transition-colors"
          >
            <Square size={11} fill="currentColor" />
          </button>
        ) : (
          <button
            onClick={send}
            disabled={!input.trim()}
            title={S.ask.send}
            className={`h-8 w-8 shrink-0 rounded-lg grid place-items-center transition-colors ${
              input.trim()
                ? "bg-white text-black hover:bg-neutral-200"
                : "bg-white/[0.07] text-neutral-600"
            }`}
          >
            <ArrowUp size={15} strokeWidth={2.4} />
          </button>
        )}
      </div>
    </div>
  );

  return (
    <div className="h-full flex">
      {/* 会话栏 */}
      <aside className={`${RAIL_CLS} flex flex-col`}>
        <div className="px-2 pt-3 pb-1">
          {/* 与会话行同一套样式：左栏是一列同质的行，新对话只是第一行 */}
          <button
            onClick={newChat}
            className="w-full flex items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px] text-neutral-300 hover:bg-white/[0.05] hover:text-white transition-colors"
          >
            <SquarePen size={14} className="shrink-0 text-neutral-500" />
            {S.ask.newChat}
          </button>
        </div>
        {/* 搜索。**标题重是常态**（同一个问题问两次就重了），而正文里那句话
            才是人记得住的——所以服务端两处都搜 */}
        <div className="px-2 pb-2">
          <input
            className="input-dark w-full px-2.5 py-1.5 text-[12.5px]"
            placeholder={S.ask.searchConversations}
            value={convSearch}
            onChange={(e) => setConvSearch(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setConvSearch("")}
          />
        </div>
        <div className="u-scroll flex-1 overflow-y-auto px-2 pb-3 space-y-0.5">
          {(convs.data?.conversations ?? []).map((c: ConversationRow) => (
            <div
              key={c.id}
              className={`group relative rounded-lg transition-colors ${
                c.id === activeId ? "u-nav-active" : "hover:bg-white/[0.05]"
              }`}
            >
              {/* 单行标题；删除键悬停浮现（弹确认，不直接删） */}
              {renamingId === c.id ? (
                /* 就地编辑：Enter 保存、Esc 取消。改一个名字不值得弹对话框 */
                <input
                  autoFocus
                  className="input-dark w-full px-2 py-1.5 text-[13px]"
                  value={renameDraft}
                  onChange={(e) => setRenameDraft(e.target.value)}
                  onBlur={() => setRenamingId(null)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && renameDraft.trim())
                      rename.mutate({ id: c.id, title: renameDraft });
                    if (e.key === "Escape") setRenamingId(null);
                  }}
                />
              ) : (
                <button
                  onClick={() => openConversation(c.id)}
                  className="w-full text-left px-2.5 py-2"
                >
                  <span
                    className={`block truncate pr-5 text-[13px] ${
                      c.id === activeId ? "text-white" : "text-neutral-300"
                    }`}
                  >
                    {c.title || S.ask.untitled}
                  </span>
                </button>
              )}
              {/* 三点菜单：**一个入口装下所有动作**。从前右边直接是删除，
                  而删除是这里最不该一步到位的那个 */}
              {renamingId !== c.id && (
                <button
                  onClick={() => setMenuFor(menuFor === c.id ? null : c.id)}
                  title={S.ask.moreActions}
                  className="absolute right-2 top-1/2 -translate-y-1/2 hidden group-hover:block text-neutral-600 hover:text-neutral-200"
                >
                  <MoreHorizontal size={14} />
                </button>
              )}
              {menuFor === c.id && (
                <>
                  {/* 点别处就关。铺满全屏而不是监听 document：不必在卸载时
                      记得摘监听器 */}
                  <div
                    className="fixed inset-0 z-10"
                    onClick={() => setMenuFor(null)}
                  />
                  <div className="glass-strong absolute right-2 top-8 z-20 w-32 rounded-lg py-1 shadow-xl">
                    <button
                      className="w-full px-3 py-1.5 text-left text-xs text-neutral-300 hover:bg-white/5"
                      onClick={() => {
                        setRenameDraft(c.title || "");
                        setRenamingId(c.id);
                        setMenuFor(null);
                      }}
                    >
                      {S.ask.rename}
                    </button>
                    <button
                      className="w-full px-3 py-1.5 text-left text-xs text-neutral-300 hover:bg-white/5"
                      onClick={() => {
                        navigator.clipboard?.writeText(c.title || "");
                        setMenuFor(null);
                      }}
                    >
                      {S.ask.copyTitle}
                    </button>
                    <button
                      className="w-full px-3 py-1.5 text-left text-xs text-[var(--u-danger)] hover:bg-white/5"
                      onClick={() => {
                        setPendingDelete(c);
                        setMenuFor(null);
                      }}
                    >
                      {S.ask.deleteConversation}
                    </button>
                  </div>
                </>
              )}
            </div>
          ))}
          {convs.data?.conversations.length === 0 && (
            <p className="px-2.5 py-2 text-xs text-neutral-600">{S.ask.noConversations}</p>
          )}
        </div>
      </aside>

      {/* 对话区：新对话首屏 = 问候 + 居中 composer（ChatGPT/Claude 惯例）；
          有消息后 composer 停靠底部 */}
      <div className="flex-1 min-w-0 flex flex-col">
        {turns.length === 0 ? (
          /* 锚定上三分之一而非垂直居中：居中在高窗口下会显得下坠。
             22vh + 顶部 chrome(~100px) ≈ 问候落在 37% 高度、composer 中心 ~49% */
          <div className="flex-1 px-4 pt-[22vh]">
            <div className="w-full max-w-3xl mx-auto">
              <h1
                className="text-center text-[26px] text-neutral-100 mb-9"
                style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.03em" }}
              >
                {S.ask.greeting}
              </h1>
              {composerCard}
            </div>
          </div>
        ) : (
          <>
            <div className="flex-1 overflow-y-auto u-scroll px-4 py-6">
              <div className="max-w-3xl mx-auto space-y-4">
                {turns.map((t, i) => (
                  <TurnView key={i} turn={t} live={streaming && i === turns.length - 1} />
                ))}
                <div ref={bottomRef} />
              </div>
            </div>
            <div className="px-4 pb-4 pt-2">
              <div className="max-w-3xl mx-auto">{composerCard}</div>
            </div>
          </>
        )}
      </div>

      {pendingDelete && (
        <DangerConfirm
          title={S.ask.deleteTitle}
          hint={S.ask.deleteHint(pendingDelete.title || S.ask.untitled)}
          confirmLabel={S.ask.deleteBtn}
          cancelLabel={S.ask.cancel}
          onConfirm={() => {
            removeConversation(pendingDelete.id);
            setPendingDelete(null);
          }}
          onCancel={() => setPendingDelete(null)}
        />
      )}
    </div>
  );
}

/** 一段正文，或一组同时发生的调用。 */
type Segment =
  | { kind: "text"; text: string; last: boolean }
  | { kind: "steps"; steps: ChatStep[] };

/** 把一轮回复拆成按发生顺序排列的段。
 *
 *  切分点是 `step.at`——那一步发生时正文已经有多长。**这条迁移之前落库的
 *  消息没有 `at`**，那时的顺序信息是真的没有存下来，编不出来也不该编：
 *  它们退回旧样子，整段轨迹在最前面。 */
function segments(turn: Turn): Segment[] {
  const steps = turn.steps ?? [];
  const text = turn.content ?? "";
  if (steps.length === 0) {
    return text ? [{ kind: "text", text, last: true }] : [];
  }
  if (steps.some((s) => s.at === undefined)) {
    return [
      { kind: "steps", steps },
      ...(text ? [{ kind: "text" as const, text, last: true }] : []),
    ];
  }
  const out: Segment[] = [];
  let cursor = 0;
  for (let i = 0; i < steps.length; ) {
    const at = steps[i].at!;
    // 同一位置的连成一组：一轮里的多次调用之间没有正文，它们本来就是一次扇出
    let j = i;
    while (j < steps.length && steps[j].at === at) j++;
    const before = text.slice(cursor, at);
    if (before) out.push({ kind: "text", text: before, last: false });
    out.push({ kind: "steps", steps: steps.slice(i, j) });
    cursor = at;
    i = j;
  }
  const tail = text.slice(cursor);
  if (tail) out.push({ kind: "text", text: tail, last: true });
  return out;
}

function stepIcon(kind: ChatStep["kind"]) {
  if (kind === "search") return <SearchIcon size={11} />;
  if (kind === "docs") return <BookOpen size={11} />;
  if (kind === "entity") return <Waypoints size={11} />;
  if (kind === "facts") return <History size={11} />;
  // facts 读世界轴、changes 读认知轴，两个图谱工具给不同的图标——
  // 用户看步骤条时该看得出问的是哪根轴
  if (kind === "changes") return <GitCompareArrows size={11} />;
  if (kind === "query") return <Database size={11} />;
  return <Wrench size={11} />;
}

/** 工具步骤 → 球体状态：思考球讲当前动作的语言 */
function orbState(kind?: ChatStep["kind"]): OrbState {
  if (kind === "search" || kind === "docs") return "searching";
  if (kind === "entity") return "connecting";
  if (kind === "facts" || kind === "changes") return "solving";
  if (kind === "query" || kind === "tool") return "working";
  return "listening"; // 尚无步骤：刚接到消息
}

/** 思考指示：thinking-orbs 球体 + 当前动作（应用是深色定妆，theme 钉死 dark）。 */
function Thinking({ step }: { step?: ChatStep }) {
  return (
    <span className="inline-flex items-center gap-2.5 text-neutral-500">
      <ThinkingOrb state={orbState(step?.kind)} size={20} theme="dark" />
      {step && (
        <span className="text-xs truncate">
          {step.label} · {step.detail}
        </span>
      )}
    </span>
  );
}

function TurnView({ turn, live }: { turn: Turn; live?: boolean }) {
  const kbId = useKbId();
  if (turn.role === "user") {
    return (
      <div className="flex justify-end">
        <div className="u-bubble-user max-w-[85%] rounded-2xl rounded-tr-sm px-4 py-2 text-sm whitespace-pre-wrap text-neutral-100">
          {turn.content}
        </div>
      </div>
    );
  }

  const thinking = live && !turn.content && !turn.error;
  const lastStep = turn.steps?.[turn.steps.length - 1];

  return (
    <div className="max-w-[95%]">
      {/* agent 回复无气泡：正文直接落在画布上（用户消息保留气泡以区分角色） */}
      <div className="py-1 text-sm text-neutral-200 leading-relaxed">
        {/* **轨迹按发生的顺序穿在正文里。**
            模型是边说边查的：说一句、调一次、再说一句。把调用整块提到最前面，
            读起来就成了「先查七次再一口气说完」——那不是它做的事，而且相邻两次
            调用之间那句「我先看看这一个」失去了它解释的对象。
            同一轮里的多次调用共享一个位置，于是自然并成一组——一组就是一轮 */}
        {segments(turn).map((seg, i) =>
          seg.kind === "steps" ? (
            <div
              key={i}
              className="my-2.5 space-y-1 border-l border-white/15 pl-2.5"
            >
              {seg.steps.map((s, j) => (
                <div key={j} className="flex items-center gap-1.5 text-xs">
                  <span className="text-neutral-600">{stepIcon(s.kind)}</span>
                  <span className="text-neutral-400 truncate">{s.label}</span>
                  <span className="text-neutral-600 shrink-0">· {s.detail}</span>
                </div>
              ))}
            </div>
          ) : (
            /* react-markdown 承载渲染（皮肤全归 u-chat-prose 设计系统），
               流式中经 remend 修补未闭合语法（粗体/围栏/链接），
               rehype-highlight 做代码高亮——成熟件组装，观感自持。
               **只有还在长的那一段需要 remend**：先前的段落已经收尾了 */
            <div key={i} className="u-chat-prose">
              <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeHighlight]}>
                {live && seg.last ? remend(seg.text) : seg.text}
              </Markdown>
            </div>
          ),
        )}
        {thinking && <Thinking step={lastStep} />}
        {turn.error && <div className="text-rose-400">{turn.error}</div>}
      </div>
      {/* **引用等答案说完再出。**
          `sources` 是随检索一次次增量发来的，跟着渲染的话，一份还在生长的清单
          就挂在一段还没写完的话下面，一边长一边把正文往上推。它是答案的落款，
          不是过程的一部分——过程已经由上面的轨迹交代了 */}
      {!live && turn.sources && turn.sources.length > 0 && (
        <div className="mt-2 space-y-1">
          {turn.sources.map((s) =>
            s.kind === "charter" ? (
              /* 手册引用：视觉上与数据引用隔离（BookOpen），跳排版好的 /docs 小节 */
              <Link
                key={s.n}
                to="/docs/$slug"
                params={{ slug: s.slug! }}
                hash={s.anchor || undefined}
                title={s.excerpt}
                className="flex items-center gap-1.5 text-xs text-neutral-500 glass rounded-lg px-3 py-1.5 glass-hover hover:text-neutral-300"
              >
                <span className="u-num text-[var(--u-accent)]">[{s.n}]</span>
                <BookOpen size={11} className="shrink-0 text-neutral-600" />
                <span className="truncate">
                  {/* 引言节 heading 即文章名，避免 "X › X" */}
                  {s.heading && s.heading !== s.filename
                    ? `${s.filename} › ${s.heading}`
                    : s.filename}
                </span>
              </Link>
            ) : (
              <Link
                key={s.n}
                to="/kb/$kbId/doc/$docId"
                params={{ kbId, docId: s.document_id! }}
                search={{ chunk: s.chunk_id }}
                title={s.excerpt}
                className="block text-xs text-neutral-500 glass rounded-lg px-3 py-1.5 glass-hover hover:text-neutral-300"
              >
                <span className="u-num text-[var(--u-accent)]">[{s.n}]</span> {s.filename} ·{" "}
                {s.excerpt.slice(0, 60)}…
              </Link>
            ),
          )}
        </div>
      )}
    </div>
  );
}
