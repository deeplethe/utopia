/* 个人信息页：头像（首字母占位）、显示名、邮箱、改密码。 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import {
  Button,
  Input,
  Loading,
} from "../ui";
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
      <div className="mb-1 text-fine font-medium text-ink-3">{label}</div>
      {node}
    </div>
  );

  return (
    <div className="max-w-lg p-8">
      <h1 className="u-title text-title mb-6">{S.account.profileTitle}</h1>

      <div className="glass rounded-xl p-6 mb-4">
        <div className="flex items-center gap-4 mb-6">
          <Avatar name={me.data.display_name} size={56} />
          <p className="text-small text-ink-3">{S.account.avatarHint}</p>
        </div>

        {field(
          S.account.displayName,
          <div className="flex gap-2">
            <Input className="flex-1"
              value={displayName}
              onChange={(e) => setName(e.target.value)}
            />
            <Button variant="primary" size="sm"
              disabled={!nameDirty || saveName.isPending}
              onClick={() => saveName.mutate(displayName.trim())}
            >
              {S.account.save}
            </Button>
          </div>,
        )}

        {field(
          S.account.email,
          <div className="text-body text-ink-2 px-1">{me.data.email}</div>,
        )}
      </div>

      <div className="glass rounded-xl p-6">
        <h2 className="text-body font-medium text-ink mb-4">
          {S.account.passwordTitle}
        </h2>
        {field(
          S.account.currentPassword,
          <Input className="w-full"
            type="password"
            autoComplete="current-password"
            value={curPw}
            onChange={(e) => setCurPw(e.target.value)}
          />,
        )}
        {field(
          S.account.newPassword,
          <Input className="w-full"
            type="password"
            autoComplete="new-password"
            value={newPw}
            onChange={(e) => setNewPw(e.target.value)}
          />,
        )}
        <div className="flex justify-end">
          <Button variant="primary" size="sm"
            disabled={!curPw || newPw.length < 8 || changePw.isPending}
            onClick={() => changePw.mutate()}
          >
            {S.account.changePassword}
          </Button>
        </div>
      </div>
    </div>
  );
}
