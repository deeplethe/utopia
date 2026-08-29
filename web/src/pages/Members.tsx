import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import { Dropdown, Pager, pageSlice, SearchSelect } from "../ui";

const ROLES = ["owner", "admin", "editor", "viewer"] as const;
const ROLE_OPTIONS = ROLES.map((r) => ({ value: r, label: S.members.roles[r] }));
const MEMBER_PAGE = 10;

export function Members({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const [addUserId, setAddUserId] = useState("");
  const [addRole, setAddRole] = useState("viewer");
  const [memberPage, setMemberPage] = useState(0);
  const [filter, setFilter] = useState("");
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  const [error, setError] = useState<string | null>(null);

  const members = useQuery({
    queryKey: ["members", workspaceId],
    queryFn: () => api.members(workspaceId),
  });
  const orgUsers = useQuery({ queryKey: ["orgUsers"], queryFn: api.orgUsers });

  const refresh = () => {
    setError(null);
    queryClient.invalidateQueries({ queryKey: ["members", workspaceId] });
  };
  const onError = (e: unknown) => setError((e as Error).message);

  const setRole = useMutation({
    mutationFn: ({ userId, role }: { userId: string; role: string }) =>
      api.setMemberRole(workspaceId, userId, role),
    onSuccess: refresh,
    onError,
  });
  const remove = useMutation({
    mutationFn: (userId: string) => api.removeMember(workspaceId, userId),
    onSuccess: refresh,
    onError,
  });

  const memberIds = new Set(members.data?.map((m) => m.user_id));
  const addable = orgUsers.data?.filter((u) => !memberIds.has(u.id)) ?? [];
  const q = filter.trim().toLowerCase();
  const memberList = (members.data ?? []).filter(
    (m) =>
      !q || m.display_name.toLowerCase().includes(q) || m.email.toLowerCase().includes(q),
  );
  const { rows: pagedMembers, safe: safeMemberPage } = pageSlice(memberList, memberPage, MEMBER_PAGE);

  return (
    <div className="glass rounded-xl p-5">
      <div className="flex items-center gap-3 mb-3">
        <h3 className="text-sm font-bold text-neutral-200">{S.members.title}</h3>
        <input
          className="input-dark ml-auto w-56 px-2.5 py-1 text-xs"
          placeholder={S.settings.searchUsers}
          value={filter}
          onChange={(e) => {
            setFilter(e.target.value);
            setMemberPage(0);
          }}
        />
      </div>

      {error && <p className="mb-3 text-sm text-rose-400">{error}</p>}

      <table className="w-full text-sm">
        <tbody>
          {pagedMembers.map((m) => (
            <tr key={m.user_id} className="border-b border-white/5">
              <td className="py-2 pr-3">
                <div className="text-neutral-200">
                  {m.display_name}
                  {m.is_admin && (
                    <span className="ml-1.5 rounded bg-[rgba(74,163,255,0.12)] px-1.5 py-0.5 text-[10px] text-[var(--u-accent)]">
                      {S.members.systemAdmin}
                    </span>
                  )}
                </div>
                <div className="text-xs text-neutral-500">{m.email}</div>
              </td>
              <td className="py-2 pr-3 text-right">
                <Dropdown
                  size="sm"
                  className="w-24 ml-auto"
                  value={m.role}
                  onChange={(role) => setRole.mutate({ userId: m.user_id, role })}
                  options={ROLE_OPTIONS}
                />
              </td>
              <td className="py-2 text-right">
                <button
                  onClick={() => remove.mutate(m.user_id)}
                  className="text-xs text-neutral-500 hover:text-rose-400"
                >
                  {S.members.remove}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="mb-4">
        <Pager
          total={memberList.length}
          pageSize={MEMBER_PAGE}
          page={safeMemberPage}
          onPage={setMemberPage}
        />
      </div>

      {/* picker 常驻。理由同 KbSettings 里那段：控件消失读作"坏了"，
          而不是"没人可加"；空列表 SearchSelect 自己会说 */}
      <div className="flex gap-2 items-center">
          <SearchSelect
            className="flex-1"
            value={addUserId}
            onChange={setAddUserId}
            placeholder={S.members.pickUser}
            options={addable.map((u) => ({
              value: u.id,
              label: u.display_name,
              hint: u.email,
            }))}
          />
          <Dropdown
            className="w-28"
            value={addRole}
            onChange={setAddRole}
            options={ROLE_OPTIONS}
          />
          <button
            onClick={() => addUserId && setRole.mutate({ userId: addUserId, role: addRole })}
            disabled={!addUserId}
            className="u-btn u-btn-primary px-3 py-1.5 text-sm"
          >
            {S.members.add}
          </button>
      </div>

      {me.data?.is_admin && <CreateUser onCreated={refresh} />}
    </div>
  );
}

/** 管理员代开账号（注册关闭后的唯一入口）。 */
function CreateUser({ onCreated }: { onCreated: () => void }) {
  const queryClient = useQueryClient();
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState("editor");
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () =>
      api.adminCreateUser({ email: email.trim(), display_name: name.trim(), password, role }),
    onSuccess: () => {
      setEmail("");
      setName("");
      setPassword("");
      setError(null);
      queryClient.invalidateQueries({ queryKey: ["orgUsers"] });
      onCreated();
    },
    onError: (e) => setError((e as Error).message),
  });

  const valid = email.includes("@") && name.trim() && password.length >= 8;

  return (
    <div className="mt-5 border-t border-white/10 pt-4">
      <h4 className="text-xs font-bold text-neutral-400 mb-2">{S.settings.newUser}</h4>
      <div className="grid grid-cols-2 gap-2 mb-2">
        <input
          className="input-dark px-3 py-2 text-sm"
          placeholder={S.login.email}
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
        <input
          className="input-dark px-3 py-2 text-sm"
          placeholder={S.login.displayName}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input-dark px-3 py-2 text-sm"
          type="password"
          placeholder={S.settings.initialPassword}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <Dropdown
          value={role}
          onChange={setRole}
          options={[
            { value: "admin", label: S.members.roles.admin },
            { value: "editor", label: S.members.roles.editor },
            { value: "viewer", label: S.members.roles.viewer },
          ]}
        />
      </div>
      <div className="flex items-center gap-3">
        <button
          className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
          disabled={!valid || create.isPending}
          onClick={() => create.mutate()}
        >
          {S.settings.createUserBtn}
        </button>
        {error && <p className="text-xs text-rose-400">{error}</p>}
      </div>
    </div>
  );
}
