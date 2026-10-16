type SsoStatus = { enabled: boolean; password_login: boolean };

/** 登录页显示哪几块。密码登录关了、但单点登录也没配好时，表单留着——不能把人全锁在外面 */
export function loginSections(status: SsoStatus | undefined) {
  const sso = !!status?.enabled;
  return { password: !sso || status?.password_login !== false, sso };
}

/** 单点登录失败时查哪句措辞。密码表单藏起来了，就别再叫人「先用密码登录」 */
export function ssoErrorKey(code: string, passwordShown: boolean) {
  return code === "unlinked" && !passwordShown ? "unlinked_sso_only" : code;
}
