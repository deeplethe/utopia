# Security

*[中文版](SECURITY.zh-CN.md)*

Utopia is at v0.1. Below are the **known, unresolved** limits — not a vulnerability report,
but the places the design has not reached yet.

## Before you put this on a public network

**The default database password is `utopia`.** By default the port binds to loopback
(`127.0.0.1:1517`), so nothing outside the host can connect. If you change `UTOPIA_DB_BIND`
to expose it, change `UTOPIA_DB_PASSWORD` in `.env` first.

**A data source is only as safe as its grants.** Registering one is a deployment-level
action, but the connection string reaches every workspace the source is granted to. Grant it
only where that database should be visible, and use a read-only database role in the string
itself — the SQL gate below is defence in depth, not a substitute for least privilege at the
source.

## What is in place

- **Credentials sealed at rest** — LLM API keys, Ask-the-Data connection strings, source
  tokens and push keys are AES-256-GCM encrypted before they reach Postgres. The key never
  enters the database: `UTOPIA_SECRET_KEY`, or `secret.key` generated in the data directory
  on first start. Back the key up together with the data directory — without it those values
  cannot be read. Rows written by earlier versions are sealed on the next start.
- **JWT signing key generated on first start** — 32 bytes from a CSPRNG, stored in the
  database. No deployment shares a default key.
- **`Secure` on session cookies behind TLS** — decided from `X-Forwarded-Proto`, so local
  HTTP development still works. Force it with `UTOPIA_COOKIE_SECURE=true` if your proxy
  omits the header.
- **Database port bound to loopback** — `127.0.0.1:1517`; the app reaches the database over
  the compose network.
- **Optional least-privilege runtime role** — set `UTOPIA_APP_DB_PASSWORD` and
  `UTOPIA_MIGRATION_URL`, and the app connects as a role that can only read and write
  business tables and append to the ledger, while migrations run as the owner.
- **Data sources reach only granted workspaces** — a registered database is mounted into a
  knowledge base only where an explicit grant exists. Before this, any base admin could
  mount any registered source, which crossed tenants.
- **Read-only gate on Ask-the-Data** — parser allowlist, read-only transaction, enforced row
  limit; three layers, so a statement past the parser still cannot write.
- **Accounts are deactivated, not deleted** — `users.deactivated_at` blocks sign-in while the
  ledger keeps that person's decisions attributable.
- **Passwords hashed with argon2.**

## Reporting a vulnerability

Email **security@deeplethe.com** rather than opening a public issue. Include the affected
version or commit, the endpoint or component, and steps to reproduce. You will get an
acknowledgement within a few days, and the release that carries the fix names you unless you
ask otherwise.
