# Security

*[中文版](SECURITY.zh-CN.md)*

Utopia is at v0.1. What follows are the **known, unresolved** limits — not a vulnerability
report, but the places the design has not reached yet. They are written down because a
project whose selling point is "every conclusion is traceable" has no business being vague
about its own weak spots.

## Before you put this on a public network

### Credentials are stored in the database in the clear

The LLM API keys entered in the UI, and the database connection strings registered for
Ask-the-Data, are stored as plain text in Postgres — see `llm_settings.chat_api_key` and
`data_sources.conn_string`.

Anyone who can read the database can read them. Encryption at rest is a hardening item
before 1.0. Until then, deploy this system and its database inside a trusted network, and
give the database its own access control.

### Default database password

The default database password in compose is `utopia`. Under the default configuration the
port is bound to loopback only (`127.0.0.1:1517`), so nothing outside the host can connect
and this is not urgent. But if you change `UTOPIA_DB_BIND` to expose it, change
`UTOPIA_DB_PASSWORD` in `.env` first.

### A data source's credentials are as strong as its grants

Registering a data source is a deployment-level action, but the connection string it carries
reaches every workspace it is granted to. Grant a source only to the workspaces that should
see that database, and prefer a read-only database role in the connection string itself —
the SQL gate below is defense in depth, not a substitute for least privilege at the source.

## What is in place

- **JWT signing key is generated on first start.** 32 bytes from a CSPRNG, stored in the
  database. There is no such thing as "every deployment shares the same default key".
- **Session cookies get `Secure` automatically behind TLS.** Decided from the request's
  `X-Forwarded-Proto`: set over HTTPS, so the browser will not send the token over a
  cleartext hop. Not set when there is no reverse proxy, so local HTTP development still
  works. If your proxy does not send that header, force it with `UTOPIA_COOKIE_SECURE=true`.
- **The database port binds to loopback.** `127.0.0.1:1517`; the app reaches the database
  over the compose network, and that mapping only serves local development.
- **Optional least-privilege runtime role.** Set `UTOPIA_APP_DB_PASSWORD` and
  `UTOPIA_MIGRATION_URL` and the application connects as a role that can only read and write
  business tables and can only append to the ledger, while migrations run under the owner
  identity.
- **Data sources reach only the workspaces they were granted to.** A registered database is
  not visible deployment-wide: it is mounted into a knowledge base only where an explicit
  grant exists. Before this existed, any knowledge-base admin could list every registered
  data source in the deployment and mount any of them — and every Viewer of that base could
  then run read-only SQL against it. On a multi-workspace deployment that crossed tenants.
- **Ask-the-Data runs behind a read-only gate.** A parser allowlist, a read-only transaction,
  and an enforced row limit — three layers, so a statement that slips past the parser still
  cannot write and still cannot run the database out of resources.
- **Accounts are deactivated, not deleted.** `users.deactivated_at` blocks sign-in and
  authentication while everything that person decided stays attributable in the ledger —
  removing the row would rewrite history, which is the one thing this system must not do.
- **Passwords are hashed with argon2.**

## Reporting a vulnerability

If you find something not listed above, open an issue. If it involves exploitable detail,
start with the minimum needed to reproduce it and we will follow up privately for the rest.
