/* 来源栏（"来源即文件夹"）：Library 与 DocViewer 共用的左侧导航。 */
import type { SourceKind } from "../sourceKinds";
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
  Trash2,
  Upload,
  Users,
  Webhook,
  type LucideIcon,
} from "lucide-react";
import { api, type SourceView } from "../api";
import { S } from "../i18n";
import { IconButton, RAIL_CLS, RailItem } from "../ui";

/** 左栏选择：全部 / 手动上传 / 某个来源 id */
/** "deleted" = 墓碑视图（#268）：删了、还能恢复或清除的文档 */
export type LibrarySelection = "all" | "uploads" | "deleted" | string;

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

// 按 SourceKind 键全：加一种来源没配图标，tsc 就红
export const KIND_ICON: Record<SourceKind, LucideIcon> = {
  folder: FolderOpen,
  url: Globe,
  rss: Rss,
  api: Webhook,
  custom: Puzzle,
  github_issues: CircleDot,
  jira_issues: SquareKanban,
  s3: HardDrive,
  azure_blob: Cloud,
  gcs: Cloud,
  webdav: FolderOpen,
  notion: Notebook,
  memory: Brain,
  upload: Upload,
};

/** 有拉取/同步语义的来源类型（folder/api 无同步概念） */
export const SYNCING_KINDS = new Set([
  "url",
  "rss",
  "custom",
  "github_issues",
  "jira_issues",
  "s3",
  "azure_blob",
  "gcs",
  "webdav",
  "notion",
]);

export const SYNC_DOT: Record<SourceView["last_sync_status"], string> = {
  never: "bg-ink-3",
  queued: "bg-warn",
  running: "bg-warn animate-pulse",
  ok: "bg-ok",
  failed: "bg-danger",
};

export function sourceIcon(s: SourceView): LucideIcon {
  // 内置类型图标固定，只有 custom 尊重用户自选图标
  if (s.kind === "custom" && s.icon && SOURCE_ICONS[s.icon]) return SOURCE_ICONS[s.icon];
  return KIND_ICON[s.kind] || Globe;
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
  // 左栏只要两个数：整库多少篇、没有来源的多少篇。**各取一页零条**——
  // 统计随响应回来，不必把文档拉下来数
  const docs = useQuery({
    queryKey: ["docCount", kbId],
    queryFn: () => api.documents(kbId, { limit: 1, offset: 0 }),
  });
  const uploads = useQuery({
    queryKey: ["docCount", kbId, "uploads"],
    queryFn: () => api.documents(kbId, { source: "none", limit: 1, offset: 0 }),
  });
  const sources = useQuery({
    queryKey: ["sources", kbId],
    queryFn: () => api.sources(kbId),
  });

  const sourceList = sources.data?.sources ?? [];
  const uploadsCount = uploads.data?.total ?? 0;

  return (
    <aside className={`${RAIL_CLS} flex flex-col`}>
      {/* 全部文档置顶为一级入口；SOURCES 小节（含 +）居其下 */}
      <div className="px-2 pt-3">
        <RailItem
          active={active === "all"}
          onClick={() => onSelect("all")}
          icon={<LibraryIcon size={14} />}
          count={docs.data?.total ?? 0}
        >
          {S.library.allDocs}
        </RailItem>
      </div>
      {/* 加号紧跟在 SOURCES 后面，是这个小节的动作，不是右边栏位里的一个数。
          按钮贴着图标（4px 内距、不要那 1px 透明边框），标题行不因它长高 */}
      <div className="flex items-center gap-1 px-4 pt-3 pb-2">
        <span className="text-fine font-medium uppercase tracking-[0.08em] text-ink-3">
          {S.library.sources}
        </span>
        {onAdd && (
          <IconButton
            size="sm"
            label={S.library.addSource}
            className="h-auto w-auto border-0 p-1"
            onClick={onAdd}
          >
            <Plus size={14} />
          </IconButton>
        )}
      </div>
      <div className="u-scroll flex-1 overflow-y-auto px-2 pb-3 space-y-1">
        {/* Uploads：常驻默认来源（上传的默认去处，不可删除） */}
        <RailItem
          active={active === "uploads"}
          onClick={() => onSelect("uploads")}
          icon={<Upload size={14} />}
          count={uploadsCount}
        >
          {S.library.uploads}
        </RailItem>
        {sourceList.map((s) => {
          const Icon = sourceIcon(s);
          return (
            <RailItem
              key={s.id}
              active={active === s.id}
              onClick={() => onSelect(s.id)}
              icon={<Icon size={14} />}
              count={s.doc_count}
              dot={
                SYNCING_KINDS.has(s.kind) || s.kind === "api"
                  ? SYNC_DOT[s.last_sync_status]
                  : undefined
              }
            >
              {s.name}
            </RailItem>
          );
        })}
      </div>
      {/* 已删除（#268）：墓碑在这里等着被恢复或清除。一个都没有就不占一行 */}
      {((docs.data?.deleted ?? 0) > 0 || active === "deleted") && (
        <div className="px-2 pb-3">
          <RailItem
            active={active === "deleted"}
            onClick={() => onSelect("deleted")}
            icon={<Trash2 size={14} />}
            count={docs.data?.deleted ?? 0}
          >
            {S.library.deleted}
          </RailItem>
        </div>
      )}
    </aside>
  );
}
