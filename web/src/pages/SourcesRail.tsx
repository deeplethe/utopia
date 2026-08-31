/* 来源栏（"来源即文件夹"）：Library 与 DocViewer 共用的左侧导航。 */
import { useQuery } from "@tanstack/react-query";
import {
  Archive,
  BookOpen,
  Brain,
  Briefcase,
  CircleDot,
  Building2,
  Cloud,
  Database,
  FileText,
  FlaskConical,
  FolderOpen,
  Globe,
  HardDrive,
  Inbox,
  Library as LibraryIcon,
  Newspaper,
  Notebook,
  Plus,
  Puzzle,
  Rocket,
  Rss,
  Server,
  SquareKanban,
  Upload,
  Users,
  Webhook,
  type LucideIcon,
} from "lucide-react";
import { api, type SourceView } from "../api";
import { S } from "../i18n";
import { RAIL_CLS } from "../ui";

/** 左栏选择：全部 / 手动上传 / 某个来源 id */
export type LibrarySelection = "all" | "uploads" | string;

/** 可选的来源图标（lucide 图标名 → 组件）。 */
export const SOURCE_ICONS: Record<string, LucideIcon> = {
  "folder-open": FolderOpen,
  globe: Globe,
  rss: Rss,
  webhook: Webhook,
  "book-open": BookOpen,
  newspaper: Newspaper,
  "file-text": FileText,
  database: Database,
  cloud: Cloud,
  "hard-drive": HardDrive,
  inbox: Inbox,
  archive: Archive,
  briefcase: Briefcase,
  "building-2": Building2,
  "flask-conical": FlaskConical,
  notebook: Notebook,
  rocket: Rocket,
  server: Server,
  users: Users,
};

export const KIND_ICON = {
  folder: FolderOpen,
  url: Globe,
  rss: Rss,
  api: Webhook,
  custom: Puzzle,
  github_issues: CircleDot,
  jira_issues: SquareKanban,
  memory: Brain,
  upload: Upload,
} as const;

/** 有拉取/同步语义的来源类型（folder/api 无同步概念） */
export const SYNCING_KINDS = new Set([
  "url",
  "rss",
  "custom",
  "github_issues",
  "jira_issues",
]);

export const SYNC_DOT: Record<SourceView["last_sync_status"], string> = {
  never: "bg-neutral-600",
  queued: "bg-[var(--u-warn)]",
  running: "bg-[var(--u-warn)] animate-pulse",
  ok: "bg-[var(--u-ok)]",
  failed: "bg-[var(--u-danger)]",
};

export function sourceIcon(s: SourceView): LucideIcon {
  // 内置类型图标固定，只有 custom 尊重用户自选图标
  if (s.kind === "custom" && s.icon && SOURCE_ICONS[s.icon]) return SOURCE_ICONS[s.icon];
  return KIND_ICON[s.kind] || Globe;
}

function RailItem({
  active,
  onClick,
  icon,
  label,
  count,
  dot,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  count: number;
  dot?: string;
}) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-[13px] transition-colors ${
        active ? "u-nav-active" : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200"
      }`}
    >
      <span className="shrink-0 text-neutral-500">{icon}</span>
      <span className="truncate">{label}</span>
      {dot && <span className={`h-1.5 w-1.5 rounded-full shrink-0 ${dot}`} />}
      <span className="ml-auto shrink-0 u-num text-[10.5px] text-neutral-600">{count}</span>
    </button>
  );
}

export function SourcesRail({
  kbId,
  active,
  onSelect,
  onAdd,
}: {
  kbId: string;
  active: LibrarySelection | null;
  onSelect: (sel: LibrarySelection) => void;
  /** 缺省时隐藏 "+"（如文档查看页） */
  onAdd?: () => void;
}) {
  const docs = useQuery({
    queryKey: ["documents", kbId],
    queryFn: () => api.documents(kbId),
  });
  const sources = useQuery({
    queryKey: ["sources", kbId],
    queryFn: () => api.sources(kbId),
  });

  const allDocs = docs.data ?? [];
  const sourceList = sources.data?.sources ?? [];
  const uploadsCount = allDocs.filter((d) => !d.source_id).length;

  return (
    <aside className={`${RAIL_CLS} flex flex-col`}>
      {/* 全部文档置顶为一级入口；SOURCES 小节（含 +）居其下 */}
      <div className="px-2 pt-3">
        <RailItem
          active={active === "all"}
          onClick={() => onSelect("all")}
          icon={<LibraryIcon size={14} />}
          label={S.library.allDocs}
          count={allDocs.length}
        />
      </div>
      <div className="flex items-center justify-between px-4 pt-3 pb-1.5">
        <span className="text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-500">
          {S.library.sources}
        </span>
        {onAdd && (
          <button
            onClick={onAdd}
            title={S.library.addSource}
            className="text-neutral-500 hover:text-neutral-200"
          >
            <Plus size={14} />
          </button>
        )}
      </div>
      <div className="u-scroll flex-1 overflow-y-auto px-2 pb-3 space-y-0.5">
        {/* Uploads：常驻默认来源（上传的默认去处，不可删除） */}
        <RailItem
          active={active === "uploads"}
          onClick={() => onSelect("uploads")}
          icon={<Upload size={14} />}
          label={S.library.uploads}
          count={uploadsCount}
        />
        {sourceList.map((s) => {
          const Icon = sourceIcon(s);
          return (
            <RailItem
              key={s.id}
              active={active === s.id}
              onClick={() => onSelect(s.id)}
              icon={<Icon size={14} />}
              label={s.name}
              count={s.doc_count}
              dot={
                SYNCING_KINDS.has(s.kind) || s.kind === "api"
                  ? SYNC_DOT[s.last_sync_status]
                  : undefined
              }
            />
          );
        })}
      </div>
    </aside>
  );
}
