/* 个人信息页：头像（首字母占位）、显示名、邮箱、改密码。 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import { Loading } from "../ui";
import { toast } from "../toast";
import { Avatar } from "./UserMenu";

export function Account() {
  const queryClient = useQueryClient();
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });

  const [name, setName] = useState<string | null>(null);
  const [curPw, setCurPw] = useState("");
  const [newPw, setNewPw] = useState("");

  const saveName = useMutation({
    mutationFn: (n: string) => api.updateMe(n),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["me"] });
      setName(null);
      toast.success(S.toast.saved);
    },
    onError: (e) => toast.error((e as Error).message),
  });

  const changePw = useMutation({
    mutationFn: () => api.changePassword(curPw, newPw),
    onSuccess: () => {
      setCurPw("");
      setNewPw("");
      toast.success(S.account.passwordChanged);
    },
    onError: (e) => toast.error((e as Error).message),
  });

  if (!me.data) return <Loading>{S.nav.loading}</Loading>;
  const displayName = name ?? me.data.display_name;
  const nameDirty = displayName.trim() !== me.data.display_name && displayName.trim();

  const field = (label: string, node: React.ReactNode) => (
    <div className="mb-4">
      <div className="mb-1 text-[11px] font-medium text-neutral-500">{label}</div>
      {node}
    </div>
  );

  return (
    <div className="max-w-lg p-8">
      <h1 className="u-title text-lg mb-6">{S.account.profileTitle}</h1>

      <div className="glass rounded-2xl p-5 mb-4">
        <div className="flex items-center gap-4 mb-5">
          <Avatar name={me.data.display_name} size={56} />
          <p className="text-xs text-neutral-500">{S.account.avatarHint}</p>
        </div>

        {field(
          S.account.displayName,
          <div className="flex gap-2">
            <input
              className="input-dark flex-1 px-3 py-2 text-sm"
              value={displayName}
              onChange={(e) => setName(e.target.value)}
            />
            <button
              className="u-btn u-btn-primary px-3.5 text-xs"
              disabled={!nameDirty || saveName.isPending}
              onClick={() => saveName.mutate(displayName.trim())}
            >
              {S.account.save}
            </button>
          </div>,
        )}

        {field(
          S.account.email,
          <div className="text-sm text-neutral-400 px-0.5">{me.data.email}</div>,
        )}
      </div>

      <div className="glass rounded-2xl p-5">
        <h2 className="text-[13px] font-medium text-neutral-200 mb-4">
          {S.account.passwordTitle}
        </h2>
        {field(
          S.account.currentPassword,
          <input
            type="password"
            autoComplete="current-password"
            className="input-dark w-full px-3 py-2 text-sm"
            value={curPw}
            onChange={(e) => setCurPw(e.target.value)}
          />,
        )}
        {field(
          S.account.newPassword,
          <input
            type="password"
            autoComplete="new-password"
            className="input-dark w-full px-3 py-2 text-sm"
            value={newPw}
            onChange={(e) => setNewPw(e.target.value)}
          />,
        )}
        <div className="flex justify-end">
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!curPw || newPw.length < 8 || changePw.isPending}
            onClick={() => changePw.mutate()}
          >
            {S.account.changePassword}
          </button>
        </div>
      </div>
    </div>
  );
}
