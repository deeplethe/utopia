/* Chat：agentic 对话（检索/图谱工具 + remember 记忆）。
   会话持久化：左栏会话列表;上下文由服务端拼,前端只发 conversation_id + 新消息;
   行动轨迹(steps)与引用(sources)随消息落库,历史回放与实时流共用渲染。 */
import { memo, useEffect, useRef, useState, useSyncExternalStore } from "react";
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
  ChevronRight,
  Database,
  GitCompareArrows,
  History,
  Layers,
  MoreHorizontal,
  Search,
  Search as SearchIcon,
  Square,
  SquarePen,
  Waypoints,
  Wrench,
} from "lucide-react";
import {
  api,
  conversationsApi,
  reattachChat,
  streamChat,
  type ChatStep,
  type ConversationRow,
} from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useKb, useKbId } from "../kb";
import {
  Button,
  cn,
  ConvMark,
  DangerConfirm,
  IconButton,
  Input,
  RAIL_CLS,
  REVEAL,
  Row,
  ROW_HOVER,
  Textarea,
} from "../ui";
import { convMarks, useLive, useUnread } from "../unread";
import {
  answeredWithoutSources,
  citedSources,
  liveAnswer,
  type LiveHandle,
  type Turn,
} from "../liveAnswer";
import { NodCard } from "./PendingFacts";
import { NextStep, nextStep, useReadiness } from "./NextStep";

/* `Turn` 定义在 liveAnswer 里：进行中的那一次也是一串 Turn，
   而它必须活得比这个组件长（见那个文件顶上的说明） */

/** 同标签页记忆：上次会话（按库）与未发送草稿——切页回来还原，新标签页从头开始 */
const lastKey = (kbId: string) => `chat:last:${kbId}`;
const DRAFT_KEY = "chat:draft";

export function Chat() {
  const kbId = useKbId();
  const { kb, kbs, setKb } = useKb();
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  /* **只拦模型这一档。** 空的知识库照样能聊——挂载的数据库查得了，记忆也读得到；
     没有对话模型才是真的一句都问不出来，而问候语从前照常显示，用户敲完第一句
     才撞墙（#313） */
  const readiness = useReadiness(kbId);
  const modelStep = readiness.data?.has_chat_model
    ? null
    : nextStep(readiness.data, {
        kbId,
        isAdmin: !!me.data?.is_admin,
        canUpload: kb?.my_role !== "viewer",
      });
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
  // 已经结束的那些轮次，从库里读来。**进行中的那一次不在这里**——见下
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState(() => sessionStorage.getItem(DRAFT_KEY) ?? "");
  /* **按 URL 认领，不按 state。** 这个文件开头就写着「URL 是当前会话的唯一
     事实来源」，而这里一度用了 `activeId`——它是 state，切走再回来时更新得
     比第一次渲染晚，于是那一帧认不出自己，屏幕空着。用地址栏里的那个 id
     就没有时序可言。新会话还没拿到 id 时两者都是空，也对得上 */
  const currentId = routeConvId ?? activeId;
  // 进行中的那些回答都活在组件之外（见 liveAnswer.ts），这里只认领「正在看的
  // 这一场」。别场的任何变更都不动这一场的快照引用——「一个正在别处生成的
  // 回答不该改变这里的任何东西」如今在渲染层也字面成立：React 靠引用相等
  // 跳过重渲染，别场逐字增长不再打扰当前会话
  const liveHere = useSyncExternalStore(
    liveAnswer.subscribe,
    () => liveAnswer.entry(kb?.id ?? null, currentId),
  );
  /* **是「这一场」在流，不是「有一场」在流。**
     写成全局的话，另一场在生成时这一场的输入框也会变成停止按钮、发不出消息，
     而且最后一轮会被当成还在流——引用于是被藏起来（那条判据见 TurnView）。
     一个正在别处生成的回答不该改变这里的任何东西 */
  const streaming = liveHere?.streaming ?? false;
  const shown = liveHere ? liveHere.turns : turns;
  const [scopeOpen, setScopeOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<ConversationRow | null>(null);
  // 会话搜索。**搜标题也搜正文**——人记得住的往往是问过的那句话
  const [convSearch, setConvSearch] = useState("");
  // 三点菜单展开的是哪一条。同时只开一个
  // 「最近」这一组收起来没有。默认展开：左栏本来就是为了看见这些会话
  const [recentOpen, setRecentOpen] = useState(true);
  /* 左栏每一条会话的记号（正在写 / 写完了还没看）。两张表都活在组件外面——
     "写完了"常常发生在这个组件已经卸载的时候（见 unread.ts 开头） */
  const liveIds = useLive();
  const unreadIds = useUnread();
  const scopeRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  /* 打开哪一场就等于看过哪一场：记号消掉，并且告诉 unread「此刻看的是它」——
     正看着的那一场写完时不该变成未读。传 null（新对话、删掉了当前这场）
     同样有意义：那之后写完的任何一场都算未读 */
  useEffect(() => {
    convMarks.view(activeId);
  }, [activeId]);

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
  }, [shown]);

  // 切库回到新会话（首次拿到 kb 不算切换——直刷 /chat/$id 时不能把 URL 冲掉）
  const prevKbRef = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevKbRef.current;
    prevKbRef.current = kb?.id ?? null;
    if (prev && kb && prev !== kb.id) {
      // **不 abort**：换库不该杀掉另一个库里正在写的回答，它落到那边的会话里
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

  /** 接回一个正在生成的回答。没有在跑的话服务端回 `idle`，什么都不发生。 */
  const attachIfRunning = (id: string, history: Turn[]) => {
    let abort = () => {};
    let handle: LiveHandle | null = null;
    const stop = reattachChat(kb!.id, id, {
      onConversation: () => {},
      /* **快照到了才建这一轮。** 先摆一个空位再等回答的话，没有在跑的会话
         上会闪一下空的助手气泡——而那是绝大多数情况。
         快照是覆盖：它是那个回答此刻的全貌，不是增量 */
      onSnapshot: (s) => {
        handle = liveAnswer.begin(
          kb!.id,
          id,
          [
            ...history,
            {
              role: "assistant",
              content: s.content,
              steps: s.steps.length ? s.steps : undefined,
              sources: s.sources.length ? s.sources : undefined,
            },
          ],
          abort,
        );
      },
      onSources: (sources) => handle?.patchLast((t) => ({ ...t, sources })),
      onStep: (step) =>
        handle?.patchLast((t) => ({ ...t, steps: [...(t.steps ?? []), step] })),
      onDelta: (text) => handle?.patchLast((t) => ({ ...t, content: t.content + text })),
      onDone: () => {
        handle?.finish();
        invalidateList();
      },
      onError: (message) => {
        handle?.patchLast((t) => ({ ...t, error: message }));
        handle?.finish();
      },
      onIdle: () => {},
    });
    abort = stop;
  };

  const loadConversation = async (id: string) => {
    // 回到正在写的那一场：直接认领，别去库里读——库里要等它写完才有那一行
    if (liveAnswer.entry(kb!.id, id)) {
      activeIdRef.current = id;
      setActiveId(id);
      return;
    }
    activeIdRef.current = id;
    setActiveId(id);
    try {
      const { messages } = await conversationsApi.detail(kb!.id, id);
      sessionStorage.setItem(lastKey(kb!.id), id);
      const history: Turn[] = messages.map((m) => ({
        role: m.role,
        content: m.content,
        steps: m.steps.length ? m.steps : undefined,
        sources: m.sources.length ? m.sources : undefined,
      }));
      setTurns(history);
      /* **刷新之后接回去。** 上面那个 store 只活在这一个页面里；刷新、
         新标签页、换台机器都拿不到它，而服务端那边生成还在跑。问一句
         「这个会话有没有在跑的」——没有是最常见的答案，代价是一次会
         立刻回 `idle` 的请求。
         最后一条是用户说的话时才问：那正好是「问了但还没答上」的形状 */
      if (history[history.length - 1]?.role === "user") {
        attachIfRunning(id, history);
      }
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
    // 同样不 abort：开一场新的不等于放弃上一场
    if (kb) sessionStorage.removeItem(lastKey(kb.id));
    activeIdRef.current = null;
    setActiveId(null);
    setTurns([]);
    navigate({ to: "/kb/$kbId/chat", params: { kbId } });
    inputRef.current?.focus();
  };

  const removeConversation = async (id: string) => {
    await conversationsApi.remove(kb!.id, id);
    // 记号跟着会话走，否则这个 id 会一直留在浏览器的那张表里
    convMarks.forget(id);
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

    /* **结果留在 store 里，不交回组件状态。**
       交回去要经过一个 `setTurns`，而流结束时这个组件可能早就卸载了——
       那一下是空操作，内容就此消失（切回来一片空白，问题气泡都没有）。
       留在 store 里，谁挂载谁认领。这一场从开场起就有名有姓：先建条目、
       后开流，回调顺着句柄只写自己这一场 */
    const handle = liveAnswer.begin(
      kb.id,
      activeId,
      // 从屏上正在显示的那些轮续接，而不是组件 state——流结束后内容只落在
      // store 里，state 还是上次装 conversation 时的库内历史，用它会让
      // 上一条回答从画面里消失
      [...(liveHere?.turns ?? turns), { role: "user", content: q }, { role: "assistant", content: "" }],
      () => {},
    );
    const abort = streamChat(
      kb.id,
      { conversation_id: activeId ?? undefined, message: q },
      {
        onConversation: (id) => {
          handle.identify(id);
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
        onSources: (sources) => handle.patchLast((t) => ({ ...t, sources })),
        onStep: (step) =>
          handle.patchLast((t) => ({ ...t, steps: [...(t.steps ?? []), step] })),
        onDelta: (text) =>
          handle.patchLast((t) => ({ ...t, content: t.content + text })),
        onDone: () => {
          handle.finish();
          invalidateList();
        },
        onError: (message) => {
          handle.patchLast((t) => ({ ...t, error: message }));
          handle.finish();
        },
      },
    );
    // streamChat 的 abort 要等它返回才有；真 abort 到手前，句柄上先占着空操作
    handle.setAbort(abort);
  };

  /* Composer 卡：新对话首屏居中出场，进入对话后停靠底部（同一块 JSX 两处复用） */
  const composerCard = (
    <div className="u-composer px-4 pt-3 pb-2">
      <Textarea
        bare
        ref={inputRef}
        rows={1}
        className="w-full resize-none text-body leading-relaxed max-h-48"
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
        <div className="flex items-center gap-3 min-w-0">
          {/* 作用域 chip：提问点位可见"在问哪个库"，切库沿用现有语义（开新会话） */}
          <div ref={scopeRef} className="relative shrink-0">
            {/* 图标的左缘要落在上面占位符的起点上（与顶栏切换器同一个图标——
                两处都是「在哪个库」）。**做法是整颗按钮左挂一档，不是把内距抹掉**：
                抹掉内距，图标就贴死在按钮的边上，指针一停，底色紧紧箍着图标，
                左边没有一点余地。挂出去则内距照留——图标落在同一个位置，
                而那块底色在它四周是匀的。挂多少就给多少内距（都是 8）。 */}
            <Button
              variant="ghost"
              size="sm"
              className="max-w-52 border-0 px-2 -ml-2"
              title={S.ask.scopeLabel}
              onClick={() => setScopeOpen((v) => !v)}
            >
              <Layers size={12} className="shrink-0 text-ink-2" />
              <span className="truncate">{kb?.name ?? "…"}</span>
              <ChevronDown
                size={11}
                className={cn("u-turn shrink-0 text-ink-2", scopeOpen && "rotate-180")}
              />
            </Button>
            {scopeOpen && (
              <div className="u-menu-glass u-pop-up absolute bottom-full mb-2 left-0 z-50 w-56 rounded-overlay u-lift-strong overflow-hidden">
                <div className="border-b border-line px-4 py-3 text-body font-medium text-ink">
                  {S.ask.scopeLabel}
                </div>
                <div className="u-scroll max-h-60 overflow-y-auto">
                  {kbs.map((k) => (
                    <Row
                      key={k.id}
                      density="menu"
                      active={k.id === kb?.id}
                      /* 对勾走 `trailing` 槽。**塞进 children 的 svg 会掉到第二行**：
                         Row 把 children 整个包进一个 `flex-1 truncate` 的 span，
                         那个 span 不是 flex 容器，preflight 又把 svg 设成块级，
                         于是勾自己占一行、整行跟着变高（顶栏的库切换器早就这么写的） */
                      trailing={
                        k.id === kb?.id ? <Check size={12} className="text-ink-2" /> : undefined
                      }
                      onClick={() => {
                        setScopeOpen(false);
                        if (k.id !== kb?.id) setKb(k.id);
                      }}
                    >
                      <span className="truncate">{k.name}</span>
                    </Row>
                  ))}
                </div>
              </div>
            )}
          </div>
          <span className="text-fine text-ink-2 truncate">{S.ask.composerHint}</span>
        </div>
        {streaming ? (
          <IconButton
            onClick={() => {
              // **只停正在看的这一场**——切页面、换会话、换库都不打断（liveAnswer.ts），
              // 别场照常写它们自己的条目
              if (kb) liveAnswer.stop(kb.id, currentId);
            }}
            label={S.ask.stop}
            variant="secondary"
            className="shrink-0"
          >
            <Square size={11} fill="currentColor" />
          </IconButton>
        ) : (
          <IconButton
            variant={input.trim() ? "primary" : "secondary"}
            className="shrink-0"
            label={S.ask.send}
            disabled={!input.trim()}
            onClick={send}
          >
            <ArrowUp size={15} strokeWidth={2.4} />
          </IconButton>
        )}
      </div>
    </div>
  );

  return (
    <div className="h-full flex">
      {/* 会话栏 */}
      <aside className={`${RAIL_CLS} flex flex-col`}>
        {/* 搜索在最上面，与图谱页的搜索框、本体页的过滤框同一副身材、同一个
            角落（左上各 12px、中号带放大镜）：换标签页时框留在原地。
            **标题重是常态**（同一个问题问两次就重了），而正文里那句话才是人
            记得住的——所以服务端两处都搜 */}
        <div className="px-2 pt-3 pb-1">
          <Input
            icon={<Search size={12} />}
            placeholder={S.ask.searchConversations}
            value={convSearch}
            onChange={(e) => setConvSearch(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setConvSearch("")}
          />
        </div>
        {/* 左栏的节奏（DESIGN 规矩 2 的左栏基线）：**盒 8 / 图标 20 / 文字 42**。
            栏的横内距 8，行的横内距 12，图标 14，图标到字 8——所以这一栏里
            每一行的字都从 42 起，搜索框的占位字也是（它的图标槽正好 left-3
            + pl-[34px]）。不带图标的行**留着那一格**（Row 自动补），字才落在
            同一条竖线上；`flush` 是给整组都没有图标的栏用的，不是给这一栏用的 */}
        <div className="px-2 pb-1">
          <Row density="nav" icon={<SquarePen size={14} />} onClick={newChat}>
            {S.ask.newChat}
          </Row>
        </div>
        {/* 「最近」是这一组的名字，不是一条会话：同一副行的身材、同一档字色
            （字色只有两档，见 styles.css），右端的三角说明这一组收得起来
            （朝右=收着，朝下=开着） */}
        <div className="px-2">
          <Row
            density="nav"
            aria-expanded={recentOpen}
            onClick={() => setRecentOpen((v) => !v)}
            trailing={
              <ChevronRight
                size={12}
                className={cn("u-turn", recentOpen && "rotate-90")}
              />
            }
          >
            {S.ask.recent}
          </Row>
        </div>
        {recentOpen && (
        <div className="u-rail-list u-scroll flex-1 overflow-y-auto px-2 pb-3">
          {(convs.data?.conversations ?? []).map((c: ConversationRow) => (
            <div
              key={c.id}
              className="group relative"
            >
              {/* 单行标题；删除键悬停浮现（弹确认，不直接删） */}
              {renamingId === c.id ? (
                /* 就地编辑：Enter 保存、Esc 取消。改一个名字不值得弹对话框 */
                <Input size="sm" className="w-full"
                  autoFocus
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
                <Row
                  density="nav"
                  icon={
                    <ConvMark
                      state={
                        liveIds.has(c.id)
                          ? "live"
                          : unreadIds.has(c.id)
                            ? "unread"
                            : "rest"
                      }
                    />
                  }
                  active={c.id === activeId}
                  className="pr-8"
                  onClick={() => openConversation(c.id)}
                >
                  <span
                    className={`block truncate pr-6 text-body ${
                      c.id === activeId ? "text-ink" : "text-ink-2"
                    }`}
                  >
                    {c.title || S.ask.untitled}
                  </span>
                </Row>
              )}
              {/* 三点菜单：**一个入口装下所有动作**（从前右边直接是删除，
                  而删除是这里最不该一步到位的那个）。走 shadcn 的
                  DropdownMenu：弹层样式与全站统一，触发器由 Radix 自动带上
                  `aria-haspopup`——按钮的"按下去沉 1px"那条特意排除了菜单
                  触发器，手搓的入口没有这个标记，所以从前一点就跳。 */}
              {renamingId !== c.id && (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <IconButton
                      size="sm"
                      label={S.ask.moreActions}
                      className={cn(REVEAL, "absolute right-1 top-1/2 -translate-y-1/2")}
                      onClick={(e) => e.stopPropagation()}
                    >
                      <MoreHorizontal size={14} />
                    </IconButton>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="w-40">
                    <DropdownMenuItem
                      onSelect={() => {
                        setRenameDraft(c.title || "");
                        setRenamingId(c.id);
                      }}
                    >
                      {S.ask.rename}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      onSelect={() => navigator.clipboard?.writeText(c.title || "")}
                    >
                      {S.ask.copyTitle}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      variant="destructive"
                      onSelect={() => setPendingDelete(c)}
                    >
                      {S.ask.deleteConversation}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              )}
            </div>
          ))}
          {/* 文字从 20 起：栏的 px-2（8）加行自己的 px-3（12），与「最近」和
              上面每条会话的标题同一条线。写成 px-2 就落在 16，差那 4px 一眼看得出 */}
          {convs.data?.conversations.length === 0 && (
            /* 34 = 行内距 12 + 图标 14 + 间距 8：这句话与上面每一条会话的
               标题同一条竖线，而不是自己另起一列 */
            <p className="py-2 pl-[34px] pr-3 text-small text-ink-2">
              {S.ask.noConversations}
            </p>
          )}
        </div>
        )}
      </aside>

      {/* 对话区：新对话首屏 = 问候 + 居中 composer（ChatGPT/Claude 惯例）；
          有消息后 composer 停靠底部 */}
      <div className="flex-1 min-w-0 flex flex-col">
        {shown.length === 0 ? (
          /* 锚定上三分之一而非垂直居中：居中在高窗口下会显得下坠。
             22vh + 顶部 chrome(~100px) ≈ 问候落在 37% 高度、composer 中心 ~49% */
          <div className="flex-1 px-4 pt-[22vh]">
            <div className="w-full max-w-3xl mx-auto">
              <h1
                className="text-center text-[26px] text-ink mb-8"
                style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.03em" }}
              >
                {S.ask.greeting}
              </h1>
              {modelStep && (
                <div className="mb-6 grid place-items-center">
                  <NextStep {...modelStep} />
                </div>
              )}
              {composerCard}
            </div>
          </div>
        ) : (
          <>
            <div className="flex-1 overflow-y-auto u-scroll u-chat-fade px-4 pt-6 pb-12">
              <div className="max-w-3xl mx-auto space-y-4">
                {shown.map((t, i) => (
                  <TurnView key={i} turn={t} live={streaming && i === shown.length - 1} />
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
  if (kind === "entity" || kind === "neighbors" || kind === "path")
    return <Waypoints size={11} />;
  if (kind === "facts" || kind === "timeline") return <History size={11} />;
  // facts 读世界轴、changes 读认知轴，两个图谱工具给不同的图标——
  // 用户看步骤条时该看得出问的是哪根轴
  if (kind === "changes") return <GitCompareArrows size={11} />;
  if (kind === "query") return <Database size={11} />;
  return <Wrench size={11} />;
}

/** 工具步骤 → 球体状态：思考球讲当前动作的语言 */
/** 一段 markdown。**按文本记忆化**：一次生成里每来一个词元，整条消息都要重渲染，
 *  而 react-markdown 每次都把那一段从头解析一遍——答案越长每个词元越贵，读起来
 *  就是越写越顿。收了尾的段落文本不再变，`memo` 让它们一次也不重解析；
 *  还在长的那一段照旧，它本来就得重解析。 */
const Segment = memo(function Segment({ text }: { text: string }) {
  return (
    <div className="u-chat-prose">
      <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeHighlight]}>
        {text}
      </Markdown>
    </div>
  );
});

/** 思考指示：**正在做的那句话自己在发光**。
 *
 * 这里换过两轮。最早是 `thinking-orbs` 的球体，跟这套界面不是一路——chrome 是
 * 零色偏的中性灰，花样留给画布，而那颗球自带一套发光点阵，`theme` 还写死成暗色。
 * 换成房里的转圈之后问题只剩一半：在做什么本来就由那行字说了
 * （「检索文档 · 3 篇」），旁边再转一个零件，等于同一件事说两遍，而且转圈说的是
 * 「有个东西在忙」，跟屏幕上这句话没关系。
 *
 * 现在没有零件：亮带横扫过那行字。它既是「还在动」，也是「动的是这句话」。
 * 还没有步骤可说的时候就扫「Thinking…」——那也是一句话，不是一个图标。 */
function Thinking({ step }: { step?: ChatStep }) {
  return (
    <span className="u-thinking text-small truncate">
      {step ? `${step.label} · ${step.detail}` : S.ask.thinking}
    </span>
  );
}

function TurnView({ turn, live }: { turn: Turn; live?: boolean }) {
  const kbId = useKbId();
  if (turn.role === "user") {
    return (
      <div className="flex justify-end">
        {/* **对话按 title 那一档读**（16/24，不是正文的 14/22）。这一页是这个
            产品里唯一一块拿来"读"的长文本，其余都是表格、清单、面板里的字段，
            14px 在那些地方是对的，成段读就偏小了。仍在五档之内，不新增字号 */}
        <div className="u-bubble-user max-w-[85%] rounded-panel px-4 py-2 text-title whitespace-pre-wrap text-ink">
          {turn.content}
        </div>
      </div>
    );
  }

  const thinking = live && !turn.content && !turn.error;
  const cited = citedSources(turn);
  const lastStep = turn.steps?.[turn.steps.length - 1];

  return (
    <div className="max-w-[95%]">
      {/* agent 回复无气泡：正文直接落在画布上（用户消息保留气泡以区分角色） */}
      <div className="py-1 text-title text-ink leading-relaxed">
        {/* **轨迹按发生的顺序穿在正文里。**
            模型是边说边查的：说一句、调一次、再说一句。把调用整块提到最前面，
            读起来就成了「先查七次再一口气说完」——那不是它做的事，而且相邻两次
            调用之间那句「我先看看这一个」失去了它解释的对象。
            同一轮里的多次调用共享一个位置，于是自然并成一组——一组就是一轮 */}
        {segments(turn).map((seg, i) =>
          seg.kind === "steps" ? (
            <div
              key={i}
              className="my-3 space-y-1 border-l border-line-strong pl-3"
            >
              {seg.steps.map((s, j) => (
                <div key={j}>
                  <div className="flex items-center gap-2 text-small">
                    <span className="text-ink-2">{stepIcon(s.kind)}</span>
                    <span className="text-ink-2 truncate">{s.label}</span>
                    <span className="text-ink-2 shrink-0">· {s.detail}</span>
                  </div>
                  {/* remember 那一步后面跟着确认卡（0015）：这句话抽出的事实先等人点头。
                      抽取是异步的，卡片在任务完成时才长出来；回放时按同一个 chunk 重画 */}
                  {s.chunk_id && <NodCard kbId={kbId} chunkId={s.chunk_id} />}
                </div>
              ))}
            </div>
          ) : (
            /* react-markdown 承载渲染（皮肤全归 u-chat-prose 设计系统），
               流式中经 remend 修补未闭合语法（粗体/围栏/链接），
               rehype-highlight 做代码高亮——成熟件组装，观感自持。
               **只有还在长的那一段需要 remend**：先前的段落已经收尾了 */
            <Segment
              key={i}
              text={live && seg.last ? remend(seg.text) : seg.text}
            />
          ),
        )}
        {thinking && <Thinking step={lastStep} />}
        {turn.error && <div className="text-danger">{turn.error}</div>}
      </div>
      {/* **引用等答案说完再出。**
          `sources` 是随检索一次次增量发来的，跟着渲染的话，一份还在生长的清单
          就挂在一段还没写完的话下面，一边长一边把正文往上推。它是答案的落款，
          不是过程的一部分——过程已经由上面的轨迹交代了 */}
      {/* 一个面板装多行（DESIGN.md 6）：引用是同构的一组，悬停归行。
          只列正文引到的那几条（见 citedSources）：检索到的不等于用到的 */}
      {!live && cited.length > 0 && (
        <div className="mt-2 glass rounded-panel divide-y divide-line">
          {cited.map((s) =>
            s.kind === "charter" ? (
              /* 手册引用：视觉上与数据引用隔离（BookOpen），跳排版好的 /docs 小节 */
              <Link
                key={s.n}
                to="/docs/$slug"
                params={{ slug: s.slug! }}
                hash={s.anchor || undefined}
                title={s.excerpt}
                className={`u-card-link flex items-center gap-2 text-small text-ink-2 px-3 py-2 ${ROW_HOVER}`}
              >
                <span className="u-num text-accent">[{s.n}]</span>
                <BookOpen size={11} className="shrink-0 text-ink-2" />
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
                className={`u-card-link block text-small text-ink-2 px-3 py-2 ${ROW_HOVER}`}
              >
                <span className="u-num text-accent">[{s.n}]</span> {s.filename} ·{" "}
                {s.excerpt.slice(0, 60)}…
              </Link>
            ),
          )}
        </div>
      )}
      {/* 没有引用时，引用那一格换成一句「未引用任何来源」（#547）：
          缺席没人读得出来，得写出来。判据见 answeredWithoutSources */}
      {answeredWithoutSources(turn, !!live) && (
        <div className="mt-2 text-small text-ink-2">{S.ask.noSources}</div>
      )}
    </div>
  );
}
