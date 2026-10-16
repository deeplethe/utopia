type MemberAccess = { role: string; is_admin: boolean };

/** 弹窗保存时真正要发出去的改动：没变的字段不发 */
export function memberChanges(member: MemberAccess, draft: MemberAccess): Partial<MemberAccess> {
  return {
    ...(draft.role !== member.role && { role: draft.role }),
    ...(draft.is_admin !== member.is_admin && { is_admin: draft.is_admin }),
  };
}
