/* 惩戒页（Punishment pages）：500 服务器失联 / 404 迷失之城。
   复用登录页场景做背景——报错也保持门面体面（自托管场景里这页常是运维第一现场）。 */
import { Home, RefreshCw } from "lucide-react";
import { Link } from "@tanstack/react-router";
import { S } from "../i18n";
import {
  Button,
  Wordmark,
} from "../ui";
import { usePageTitle } from "../useTitle";
import { LoginScene } from "./LoginScene";

function PunishmentPage({
  message,
  children,
}: {
  message: string;
  children: React.ReactNode;
}) {
  return (
    <div className="min-h-screen flex items-center justify-center px-4">
      <LoginScene />
      <div className="relative z-10 text-center u-rise">
        <h1 className="text-5xl font-normal">
          <Wordmark />
        </h1>
        <p className="u-balance mt-4 text-body text-ink-2">{message}</p>
        <div className="mt-6 flex items-center justify-center gap-4">
          {children}
          <a
            href={`${S.login.githubUrl}/issues`}
            target="_blank"
            rel="noreferrer"
            className="u-link text-small"
          >
            {S.nav.reportIssue}
          </a>
        </div>
      </div>
    </div>
  );
}

export function ServerDown() {
  usePageTitle(S.app.name, "Punishment 500");
  return (
    <PunishmentPage message={S.nav.serverUnreachable}>
      <Button variant="secondary" size="sm" className="flex items-center gap-2"
        onClick={() => window.location.reload()}
      >
        <RefreshCw size={12} />
        {S.nav.refresh}
      </Button>
    </PunishmentPage>
  );
}

export function NotFound() {
  usePageTitle(S.app.name, "Punishment 404");
  return (
    <PunishmentPage message={S.nav.notFound}>
      <Link to="/" className="u-btn u-btn-ghost px-4 py-2 text-small flex items-center gap-2">
        <Home size={12} />
        {S.nav.returnHome}
      </Link>
    </PunishmentPage>
  );
}
