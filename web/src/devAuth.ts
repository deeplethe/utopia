/* 本地开发便利：跳过登录页，用环境里的凭据直接建立会话，于是 `vite` 一开就是
   登录后的应用（首页 = 图谱），不必每次手动登一遍。

   三重收口，确保它只活在本地开发、且默认关闭：
   1. `import.meta.env.DEV`——生产构建里这是**静态 false**，整段被摇树删掉，
      连同下面读到的 VITE_DEV_* 一起不会进产物。
   2. 凭据来自环境（`.env.local`，已 gitignore），**不写死**任何账号口令。
   3. 失败即闭合：任一变量没设、后端不可达、或登录被拒，都静默退出，
      让应用照常走它自己的未登录分支（登录页）。这不是鉴权旁路，只是替你
      把登录表单填了一次。 */
export async function ensureDevSession(): Promise<void> {
  if (!import.meta.env.DEV) return;

  const email = import.meta.env.VITE_DEV_EMAIL;
  const password = import.meta.env.VITE_DEV_PASSWORD;
  if (!email || !password) return;

  try {
    // 已经有会话就什么都不做——避免每次刷新都白登一遍，也不覆盖手动登的别的账号
    const me = await fetch("/api/v1/auth/me", { credentials: "include" });
    if (me.ok) return;

    await fetch("/api/v1/auth/login", {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
  } catch {
    // 后端没起或网络不可达：交回应用自己处理（它会显示服务器不可用/登录页）
  }
}
