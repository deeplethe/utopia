/* 账户层"我的知识库"：可访问的库 + 我的角色 + 加入信息 + 概览统计，**以及建库**。
   从前建库的入口在 Administration 里，而那一节只有系统管理员看得见——可建库要的
   是工作区 Admin+，于是一个工作区管理员在界面上根本没有建库的门。两处列的又是
   同一份数据（都读 `my-kbs`），所以并成这一页。 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { Lock, Plus, Search } from "lucide-react";
import { api, type MyKb } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { kbGroups } from "./kbGroups";
import {
  Button,
  Checkbox,
  Chip,
  Dialog,
  Field,
  Input,
  Loading,
  MultiSearchSelect,
  PageHeader,
  TBody,
  THead,
  Table,
  Td,
  Th,
  Tr,
  localDate,
} from "../ui";
import { fetchMe } from "../authSession";

function joinInfo(row: MyKb): string {
  if (row.my_role === "owner") return S.account.deploymentAdmin;
  if (row.joined_at) {
    return row.added_by_name
      ? S.account.addedBy(row.added_by_name, localDate(row.joined_at))
      : S.account.joinedOn(localDate(row.joined_at));
  }
  return S.account.openToEveryone;
}

export function MyKbs() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { workspace, setKb } = useKb();
  // 库切换器最后一行带 ?create=true 过来：落地就开表单
  const { create: deepLink } = useSearch({ from: "/account/account/kbs" });
  const [creating, setCreating] = useState(!!deepLink);
  const [filter, setFilter] = useState("");

  const mine = useQuery({
    queryKey: ["myKbs", workspace?.id],
    queryFn: () => api.myKbs(workspace!.id),
    enabled: !!workspace,
  });
  const me = useQuery({ queryKey: ["me"], queryFn: fetchMe });
  /* 能不能建库：系统管理员，或者在这个工作区里是 Admin+（api/kbs.rs 的 create
     就是这么判的）。**editor 不能**——它管的是库里的内容，不是有几个库 */
  const wsRole = useQuery({
    queryKey: ["workspaceRole", workspace?.id],
    queryFn: () => api.workspaceRole(workspace!.id),
    enabled: !!workspace,
  });
  const canCreate =
    !!me.data?.is_admin ||
    wsRole.data?.role === "admin" ||
    wsRole.data?.role === "owner";

  if (!workspace || mine.isPending) return <Loading>{S.nav.loading}</Loading>;

  const q = filter.trim().toLowerCase();
  const rows = (mine.data?.kbs ?? []).filter(
    (r) =>
      !q ||
      r.kb.name.toLowerCase().includes(q) ||
      (r.kb.description ?? "").toLowerCase().includes(q),
  );
  // 不筛选时按 `owner/` 归组：组名占一行，下面是短名（#1249）；筛选时按全名平铺
  const rowById = new Map(rows.map((r) => [r.kb.id, r]));
  const lines: ({ heading: string } | { row: (typeof rows)[number]; label: string })[] = q
    ? rows.map((row) => ({ row, label: row.kb.name }))
    : kbGroups(rows.map((r) => r.kb)).flatMap((entry) =>
        entry.kind === "kb"
          ? [{ row: rowById.get(entry.kb.id)!, label: entry.kb.name }]
          : [
              { heading: entry.name },
              ...entry.items.map((item) => ({ row: rowById.get(item.kb.id)!, label: item.label })),
            ],
      );
  const openKb = (id: string) => {
    setKb(id);
    navigate({ to: "/kb/$kbId/graph", params: { kbId: id } });
  };

  return (
    <div className="px-8 py-6">
      {/* 与管理页同一副身材：内容居中限宽，行长不随窗口拉长 */}
      <div className="mx-auto w-full max-w-4xl">
        <PageHeader
          title={S.account.kbsTitle}
          actions={
            canCreate && (
              <Button variant="primary" size="sm" onClick={() => setCreating(true)}>
                <Plus size={12} />
                {S.settings.kbs.newKb}
              </Button>
            )
          }
        />

        {/* 筛这份名单的东西在面板外面（DESIGN.md 6） */}
        <div className="mb-4">
          <Input
            icon={<Search size={13} />}
            /* 筛选条上的搜索框全站一个宽度：w-64（成员、规则、数据口径都是它） */
            className="w-64"
            placeholder={S.account.kbsFilter}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>

        {/* **一张表，不是一列卡片。**这一页是一份记录清单——每个库的字段都一样，
            人来这儿是横着比：谁的文档多、我在哪个库是 admin、哪个是受限的。
            从前把这些堆成「名字 + 三枚胶囊」再加一行点号串起来的小字，同一个事实
            在不同行落在不同的横向位置，比不了；而且全站别的清单（成员、令牌、
            文库、规则、本体）早就是 Table / Th / Td，只有这一页是手写的。
            身份那一列**是一个词，不是一枚填色胶囊**：它在自己那一列里，
            列头已经说了这是什么，不需要再染一次色。 */}
        <div className="glass rounded-panel overflow-hidden">
          <Table>
            <THead>
              <Tr>
                <Th>{S.account.kbNameLabel}</Th>
                <Th>{S.account.kbRoleLabel}</Th>
                <Th className="text-right">{S.account.kbDocsLabel}</Th>
                <Th className="text-right">{S.account.kbMembersLabel}</Th>
                <Th>{S.account.kbAccessLabel}</Th>
                <Th />
              </Tr>
            </THead>
            <TBody>
              {lines.map((line) => {
                if ("heading" in line)
                  return (
                    <Tr key={`group-${line.heading}`}>
                      <th
                        scope="rowgroup"
                        colSpan={6}
                        className="px-3 pt-4 pb-2 text-left align-top text-small font-medium text-ink-2"
                      >
                        {line.heading}
                      </th>
                    </Tr>
                  );
                const { row, label } = line;
                const canManage =
                  row.my_role === "admin" || row.my_role === "owner";
                return (
                  <Tr key={row.kb.id}>
                    <Td>
                      <div className="flex items-center gap-2">
                        <span className="truncate text-body text-ink" title={row.kb.name}>
                          {label}
                        </span>
                        {/* 这两个是**库自己的性质**，不是一列数据：缺省库只有一个，
                            受限的也少，单开一列会空掉大半，所以贴在名字旁边 */}
                        {row.kb.is_default && (
                          <Chip tone="neutral">{S.settings.kbs.defaultChip}</Chip>
                        )}
                        {row.kb.visibility === "restricted" && (
                          <span
                            className="flex shrink-0 items-center gap-1 text-fine text-ink-2"
                            title={S.account.kbRestricted}
                          >
                            <Lock size={10} />
                            {S.account.kbRestricted}
                          </span>
                        )}
                      </div>
                    </Td>
                    <Td className="text-ink-2">
                      {row.my_role
                        ? (S.account.roleNames[row.my_role] ?? row.my_role)
                        : "—"}
                    </Td>
                    <Td className="u-num text-right text-ink-2">{row.doc_count}</Td>
                    <Td className="u-num text-right text-ink-2">
                      {row.member_count}
                    </Td>
                    <Td className="text-small text-ink-2">{joinInfo(row)}</Td>
                    {/* 设置在前、打开在后：不是每一行都有设置（要 admin），把总是在的
                        那个放右端，一列按钮的右缘才不会一行一个样 */}
                    <Td className="text-right">
                      <div className="flex items-center justify-end gap-2">
                        {canManage && (
                          <Button variant="secondary" size="sm"
                            onClick={() => {
                              setKb(row.kb.id);
                              navigate({
                                to: "/kb/$kbId/settings",
                                params: { kbId: row.kb.id },
                              });
                            }}
                          >
                            {S.account.kbSettingsBtn}
                          </Button>
                        )}
                        <Button variant="secondary" size="sm"
                          onClick={() => openKb(row.kb.id)}
                        >
                          {S.account.openKb}
                        </Button>
                      </div>
                    </Td>
                  </Tr>
                );
              })}
            </TBody>
          </Table>
          {rows.length === 0 && (
            <p className="px-4 py-6 text-body text-ink-2">{S.ui.noMatches}</p>
          )}
        </div>

        {creating && (
          <NewKbModal
            workspaceId={workspace.id}
            onDone={(id) => {
              setCreating(false);
              queryClient.invalidateQueries({ queryKey: ["myKbs", workspace.id] });
              queryClient.invalidateQueries({ queryKey: ["kbs", workspace.id] });
              // 建完直达库设置：下一步几乎总是邀人/配置
              if (id) {
                setKb(id);
                navigate({ to: "/kb/$kbId/settings", params: { kbId: id } });
              }
            }}
          />
        )}
      </div>
    </div>
  );
}

/** 新建知识库弹窗：缺省 restricted，不污染全员切换器。
    **建库不是系统管理员专属**——工作区 Admin 起就能建（见 api/kbs.rs 的
    create），所以它住在人人都会来的这一页，不在 Administration 里 */
function NewKbModal({
  workspaceId,
  onDone,
}: {
  workspaceId: string;
  onDone: (id?: string) => void;
}) {
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [restricted, setRestricted] = useState(true);
  // 默认一个包都不勾（#580）。从前预勾 schema.org，理由是"要一个已经能认出人、
  // 组织、产品的起点"；量过之后本体从文档里长出来的是库自己的十几个类，而 915 个
  // 类的包让每块抽取提示词从 2k 涨到 18k tokens。要包的人在这里勾
  const [packs, setPacks] = useState<string[]>([]);

  const available = useQuery({
    queryKey: ["ontologyPacks"],
    queryFn: api.ontologyPacks,
  });

  // 勾选顺序即安装顺序：第一个包的类会认领同名的种子类，
  // 后面的撞名才查得到对齐表。所以取消再勾会排到末尾——这是对的
  const toggle = (id: string) =>
    setPacks((prev) =>
      prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id],
    );

  const create = useMutation({
    mutationFn: () =>
      api.createKb(workspaceId, {
        name: name.trim(),
        description: desc.trim() || null,
        visibility: restricted ? "restricted" : "open",
        ontology_packs: packs,
      }),
    onSuccess: (kb) => onDone(kb.id),
  });

  return (
    <Dialog
      open
      onOpenChange={(o) => !o && onDone()}
      title={S.settings.kbs.newKb}
      closeLabel={S.ui.close}
      width="sm"
      footer={
        <>
          <Button variant="secondary" size="sm" onClick={() => onDone()}>
            {S.library.cancel}
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={!name.trim() || create.isPending}
            onClick={() => create.mutate()}
          >
            {S.settings.kbs.create}
          </Button>
        </>
      }
    >
      <div>
        <Input className="w-full mb-2"
          autoFocus
          placeholder={S.settings.kbs.name}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <Input className="w-full mb-3"
          placeholder={S.settings.kbs.description}
          value={desc}
          onChange={(e) => setDesc(e.target.value)}
        />
        <Checkbox
          className="mb-4"
          checked={restricted}
          onChange={(v) => setRestricted(v)}
          label={S.settings.kbs.visRestricted}
        />

        {/* 包是可选的，而且**多半只装一个**：五张卡片铺开占了这张弹窗一多半，
            换成搜着选——说明与规模挪进下拉里的次要文案，选中的堆在框上面 */}
        <Field label={S.settings.kbs.packsLabel} hint={S.settings.kbs.packsHint}>
          <MultiSearchSelect
            className="w-full"
            values={packs}
            options={(available.data?.packs ?? []).map((p) => ({
              value: p.id,
              label: p.name,
              hint: `${p.summary} · ${S.settings.kbs.packsCount(p.classes, p.properties)}`,
            }))}
            placeholder={S.settings.kbs.packsPick}
            emptyHint={S.settings.kbs.packsNone}
            onToggle={toggle}
          />
        </Field>
        {create.isError && (
          <p className="text-small text-danger mb-2">
            {(create.error as Error).message}
          </p>
        )}
      </div>
    </Dialog>
  );
}

