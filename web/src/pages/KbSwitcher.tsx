/* 顶栏的知识库切换器：与用户菜单、告警面板同一套弹层（shadcn Popover）。
   下面列全部的库，当前那个带勾。从前这三处各写一遍「胶囊原地长成面板」的
   过渡，面板第一行还要把胶囊本身再画一遍——统一之后那一行没有了，
   触发器就是触发器。

   面板分三段：查找、库、新建。库上了十几个之后，切换库这件事是**先打字再挑**，
   不是滚一列名字；新建钉在最下面而不是混在列里——它不是一个库，是这张面板
   的出口。 */
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ChevronDown, Layers, Plus } from "lucide-react";
import { api, type Kb } from "../api";
import { S } from "../i18n";
import { Button, Input, Row } from "../ui";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { KbMenuList } from "./KbMenuList";
import { fetchMe } from "../authSession";

export function KbSwitcher({
  kb,
  kbs,
  onChange,
}: {
  kb: Kb | null | undefined;
  kbs: Kb[];
  onChange: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  const navigate = useNavigate();
  const name = kb?.name ?? "…";
  const [q, setQ] = useState("");
  // 建库要系统管理员或工作区 Admin+（见 api/kbs.rs create）：没这个权限的人
  // 看不到入口，省得点进去吃一个 403
  const me = useQuery({ queryKey: ["me"], queryFn: fetchMe });
  const wsRole = useQuery({
    queryKey: ["workspaceRole", kb?.workspace_id],
    queryFn: () => api.workspaceRole(kb!.workspace_id),
    enabled: !!kb?.workspace_id,
  });

  // 关掉就把查找词丢掉：下次打开是从头挑，不是接着上次的筛选结果
  const dismiss = () => {
    setQ("");
    close();
  };
  const needle = q.trim().toLowerCase();
  const shown = needle
    ? kbs.filter((k) => k.name.toLowerCase().includes(needle))
    : kbs;

  return (
    /* 库切换器。**弹层是 shadcn 的 Popover**，与顶栏另外两个面板同一副：
       从前是「胶囊原地长成面板」的手写过渡，第一行要把胶囊本身再画一遍、
       触发器打开时还得隐身。现在触发器就是触发器，面板就是面板。 */
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        {/* 胶囊：无底无框，图标与导航标签同一档灰、同一个大小和线宽（15 / 1.8），
            名字与标签同一个字号和字重（正文、中等），颜色是正文色——它是当前库的
            名字，不是次要信息 */}
        <Button
          variant="ghost"
          size="sm"
          title={S.nav.kbLabel}
          /* 内距 12，与全站的行、以及顶栏字标的 pl-3 同一个数。
             从前是 8，图标贴着胶囊的左缘——胶囊一有底（悬停、打开）就看得出来 */
          className="h-8 max-w-64 border-0 px-3"
          icon={<Layers size={15} strokeWidth={1.8} className="text-ink-2" />}
        >
          <span className="truncate text-body font-medium text-ink">{name}</span>
          <ChevronDown size={12} className="shrink-0 text-ink-2" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-max min-w-64 max-w-80 overflow-hidden p-0"
      >
          {/* 查找：没有自己的框（bare）——它是面板的一段，不是面板里摆的一个控件。
              打开就聚焦，一开口就能打字；Esc 与外点由 Popover 统一管 */}
          <div className="border-b border-line px-4 py-3">
            <Input
              bare
              autoFocus
              className="w-full text-body"
              placeholder={S.nav.findKb}
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          </div>
          <div className="u-scroll max-h-80 overflow-y-auto">
            <KbMenuList
              kbs={shown}
              currentId={kb?.id}
              grouped={!needle}
              onPick={(id) => {
                dismiss();
                if (id !== kb?.id) onChange(id);
              }}
            />
            {shown.length === 0 && (
              <p className="px-4 py-3 text-body text-ink-2">{S.nav.noKbMatch}</p>
            )}
          </div>
          {/* 新建钉在最下面，不混在列里：它不是一个库。去的是「我的知识库」，
              带上 create——落地就是表单，不用到了那一页再找一次按钮 */}
          {(me.data?.is_admin ||
            wsRole.data?.role === "admin" ||
            wsRole.data?.role === "owner") && (
            <Row
              density="menu"
              className="gap-3 border-t border-line px-4 py-3 text-body"
              icon={<Plus size={14} />}
              onClick={() => {
                dismiss();
                navigate({ to: "/account/kbs", search: { create: true } });
              }}
            >
              {S.settings.kbs.newKb}
            </Row>
          )}
      </PopoverContent>
    </Popover>
  );
}
