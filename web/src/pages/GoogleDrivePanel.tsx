/* Google Drive 来源的连接与选文件夹（0074），住在来源设置弹窗里。
 *
 * 连接走弹窗而不是整页跳转：设置弹窗里的东西（改了一半的名字、日程）不会丢。
 * 回调页不带脚本，不 postMessage 回来——这边在弹窗开着的时候轮询来源列表，
 * 看见 `connected_at` 变了就关掉它。人自己关了弹窗，轮询也就停了。
 *
 * 浏览器从头到尾拿不到 Google 的令牌：文件夹列表是服务器以连接者的身份去取的。
 * 读的是连接者自己的 Drive，所以浏览与选文件夹只给他本人；库里别的编辑者只看得见选了什么。 */
import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, Folder, X } from "lucide-react";
import { api, type SourceView } from "../api";
import { fetchMe } from "../authSession";
import { S } from "../i18n";
import { toast } from "../toast";
import { Button, Checkbox, Chip, DangerConfirm, IconButton, LinkButton } from "../ui";

type Entry = { id: string; name: string };

/** 服务器给的两个固定入口按 id 换成界面语言；其余是 Drive 里的真名 */
function label(e: Entry): string {
  if (e.id === "root") return S.library.googleDrive.myDrive;
  if (e.id === "shared:with-me") return S.library.googleDrive.sharedWithMe;
  return e.name;
}

export function GoogleDrivePanel({ kbId, source }: { kbId: string; source: SourceView }) {
  const G = S.library.googleDrive;
  const queryClient = useQueryClient();
  const cfg = source.config ?? {};
  const connectedAt = cfg.connected_at ?? null;
  const me = useQuery({ queryKey: ["me"], queryFn: fetchMe });
  const mine = Boolean(connectedAt) && cfg.connected_by === me.data?.id;
  const [selected, setSelected] = useState<Entry[]>(cfg.folders ?? []);
  // 浏览路径：空 = 第一层（我的云端硬盘、与我共享、共享云端硬盘）
  const [path, setPath] = useState<Entry[]>([]);
  const [waiting, setWaiting] = useState(false);
  // 断开会丢掉连接者的令牌与文件夹：先问一句，说清断开的是谁的 Drive
  const [confirmingDisconnect, setConfirmingDisconnect] = useState(false);
  const popup = useRef<Window | null>(null);
  const startedFrom = useRef<string | null>(null);

  const status = useQuery({
    queryKey: ["google-drive-status"],
    queryFn: api.googleDriveStatus,
  });
  const parent = path.at(-1)?.id;
  const folders = useQuery({
    queryKey: ["google-drive-folders", source.id, connectedAt, parent ?? ""],
    queryFn: () => api.googleDriveFolders(kbId, source.id, parent),
    enabled: mine,
    retry: false,
  });

  // 弹窗开着时轮询：连上了（connected_at 变了）就关弹窗；人关了弹窗就停
  useEffect(() => {
    if (!waiting) return;
    const timer = window.setInterval(() => {
      if (popup.current?.closed) {
        setWaiting(false);
        return;
      }
      queryClient.invalidateQueries({ queryKey: ["sources", kbId] });
    }, 1500);
    return () => window.clearInterval(timer);
  }, [waiting, kbId, queryClient]);
  useEffect(() => {
    if (waiting && connectedAt && connectedAt !== startedFrom.current) {
      popup.current?.close();
      setWaiting(false);
      setPath([]);
    }
  }, [waiting, connectedAt]);

  const connect = useMutation({
    mutationFn: async () => {
      // 先同步地开一个空弹窗，再填地址：等请求回来再 window.open 会被拦
      const w = window.open("", "utopia-google-drive", "width=520,height=680");
      try {
        const { url } = await api.googleDriveConnect(kbId, source.id);
        if (w) w.location.href = url;
        else window.location.href = url;
        popup.current = w;
        startedFrom.current = connectedAt;
        setWaiting(true);
      } catch (e) {
        w?.close();
        throw e;
      }
    },
    onError: (e) => toast.error((e as Error).message),
  });

  const disconnect = useMutation({
    mutationFn: () => api.googleDriveDisconnect(kbId, source.id),
    onSuccess: () => {
      setConfirmingDisconnect(false);
      toast.success(G.disconnected);
      queryClient.invalidateQueries({ queryKey: ["sources", kbId] });
    },
    onError: (e) => toast.error((e as Error).message),
  });

  const save = useMutation({
    mutationFn: () => api.googleDriveSelect(kbId, source.id, selected),
    onSuccess: () => {
      toast.success(G.saved);
      queryClient.invalidateQueries({ queryKey: ["sources", kbId] });
    },
    onError: (e) => toast.error((e as Error).message),
  });

  const isSelected = (id: string) => selected.some((f) => f.id === id);
  const toggle = (e: Entry) =>
    setSelected((cur) =>
      cur.some((f) => f.id === e.id)
        ? cur.filter((f) => f.id !== e.id)
        : [...cur, { id: e.id, name: label(e) }],
    );
  const unchanged =
    JSON.stringify(selected.map((f) => f.id)) ===
    JSON.stringify((cfg.folders ?? []).map((f) => f.id));

  if (status.data && !status.data.enabled) {
    return <p className="mb-3 text-fine leading-relaxed text-ink-2">{G.notConfigured}</p>;
  }

  return (
    <div className="mb-3">
      <div className="mb-1 text-fine font-medium text-ink-2">{G.account}</div>
      <div className="mb-3 flex items-center justify-between gap-3">
        <span className="text-small text-ink">
          {waiting
            ? G.waiting
            : connectedAt
              ? cfg.account_email
                ? G.connectedAs(cfg.account_email)
                : G.connectedNoEmail
              : G.notConnected}
        </span>
        <div className="flex shrink-0 gap-2">
          {connectedAt && (
            <Button
              variant="secondary"
              size="sm"
              disabled={disconnect.isPending}
              onClick={() => setConfirmingDisconnect(true)}
            >
              {G.disconnect}
            </Button>
          )}
          {/* 别人连着的只能先断开再连：重连会换掉他的 Drive，服务器也会拒绝 */}
          {(!connectedAt || mine) && (
            <Button
              variant={connectedAt ? "secondary" : "primary"}
              size="sm"
              disabled={connect.isPending || waiting}
              onClick={() => connect.mutate()}
            >
              {connectedAt ? G.reconnect : G.connect}
            </Button>
          )}
        </div>
      </div>

      {confirmingDisconnect && (
        <DangerConfirm
          title={G.disconnect}
          hint={G.disconnectConfirm(
            cfg.account_email ?? G.unnamedAccount,
            mine,
            cfg.connected_by_name ?? G.someoneElse,
          )}
          confirmLabel={G.disconnect}
          cancelLabel={G.cancel}
          busy={disconnect.isPending}
          onConfirm={() => disconnect.mutate()}
          onCancel={() => setConfirmingDisconnect(false)}
        />
      )}

      {connectedAt && !mine && (
        <>
          <div className="mb-1 text-fine font-medium text-ink-2">{G.folders}</div>
          <p className="mb-2 text-fine leading-relaxed text-ink-2">{G.onlyConnector}</p>
          {(cfg.folders ?? []).length === 0 ? (
            <p className="text-fine text-ink-2">{G.noneSelected}</p>
          ) : (
            <div className="flex flex-wrap gap-1">
              {(cfg.folders ?? []).map((f) => (
                <Chip key={f.id}>{f.name}</Chip>
              ))}
            </div>
          )}
        </>
      )}

      {mine && (
        <>
          <div className="mb-1 text-fine font-medium text-ink-2">{G.folders}</div>
          <p className="mb-2 text-fine leading-relaxed text-ink-2">{G.foldersHint}</p>

          {/* 面包屑：点哪一级回哪一级 */}
          <div className="mb-1 flex flex-wrap items-center gap-1 text-fine text-ink-2">
            <LinkButton onClick={() => setPath([])}>{G.top}</LinkButton>
            {path.map((p, i) => (
              <span key={p.id} className="flex items-center gap-1">
                <ChevronRight size={12} />
                <LinkButton onClick={() => setPath(path.slice(0, i + 1))}>
                  {label(p)}
                </LinkButton>
              </span>
            ))}
          </div>
          <div className="mb-3 max-h-56 overflow-y-auto rounded-cell border border-line">
            {folders.isPending ? (
              <div className="p-3 text-fine text-ink-2">…</div>
            ) : folders.isError ? (
              <div className="p-3 text-fine text-danger">{(folders.error as Error).message}</div>
            ) : folders.data.folders.length === 0 ? (
              <div className="p-3 text-fine text-ink-2">{G.empty}</div>
            ) : (
              folders.data.folders.map((f) => (
                <div
                  key={f.id}
                  className="flex items-center justify-between gap-2 border-b border-line px-2 py-1 last:border-b-0"
                >
                  {/* 「与我共享」只是一个列表，不是一个能同步的文件夹 */}
                  {f.id === "shared:with-me" ? (
                    <span className="flex items-center gap-2 text-small">
                      <Folder size={13} className="text-ink-2" />
                      {label(f)}
                    </span>
                  ) : (
                    <Checkbox
                      checked={isSelected(f.id)}
                      onChange={() => toggle(f)}
                      label={label(f)}
                    />
                  )}
                  <IconButton
                    variant="ghost"
                    label={G.open}
                    onClick={() => setPath([...path, f])}
                  >
                    <ChevronRight size={14} />
                  </IconButton>
                </div>
              ))
            )}
          </div>

          {selected.length === 0 ? (
            <p className="mb-2 text-fine text-ink-2">{G.noneSelected}</p>
          ) : (
            <div className="mb-2 flex flex-wrap gap-1">
              {/* 点一下就从选中里拿掉 */}
              {selected.map((f) => (
                <Chip key={f.id} title={G.remove} onClick={() => toggle(f)}>
                  {f.name}
                  <X size={12} />
                </Chip>
              ))}
            </div>
          )}
          <Button
            variant="primary"
            size="sm"
            disabled={unchanged || save.isPending}
            onClick={() => save.mutate()}
          >
            {G.saveFolders}
          </Button>
        </>
      )}
    </div>
  );
}
