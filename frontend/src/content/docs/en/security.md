# Permissions, security, and audit

Human browser sessions and machine tokens are separate credentials. Knowledge-system roles and platform administration are evaluated independently.

| Role | Permissions |
| --- | --- |
| Owner | Members, API tokens, lifecycle, and high-risk operations |
| Editor | Documents, extraction, ontology, vocabulary, instances, and review |
| Viewer | Read-only workspace access |

Each API token belongs to one knowledge system and has independent scopes, expiry, and revocation. Tokens are sent only through the `Authorization` header.

External SPARQL accepts `SELECT` and `ASK` only, rejects `SERVICE`, `FROM`, `GRAPH`, and updates, fixes the dataset to the token's knowledge system, and enforces text and row limits.

Only selected chunks and bounded ontology context are sent to administrator-configured model endpoints. Models receive no unrestricted database, filesystem, or cross-project access.

Graph edits, review decisions, rollback, token management, and publish actions create audit records. Release artifacts are immutable and independently checksummed.

Production deployments should enable HTTPS and secure cookies, rotate default credentials, back up every stateful store, retain token encryption keys, and configure reverse-proxy limits and access logs.
