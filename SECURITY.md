# Security Policy

## Reporting a Vulnerability

Do not open a public issue for a suspected vulnerability. Use the repository's private [GitHub Security Advisory form](https://github.com/deeplethe/ontopilot/security/advisories/new).

Include:

- affected version or commit;
- deployment topology;
- reproduction steps or proof of concept;
- potential impact;
- any known mitigation.

Please avoid accessing data that is not yours, disrupting services, or publishing details before a coordinated fix is available.

## Supported Versions

Until the first stable release, security fixes target the latest commit on the default branch. After stable releases begin, this document will list supported release lines explicitly.

## Deployment Responsibilities

OntoPilot is self-hosted and connects to administrator-configured model providers. Operators are responsible for:

- HTTPS termination and `COOKIE_SECURE=true`;
- strong administrator and PostgreSQL credentials;
- network restrictions for PostgreSQL, Oxigraph data, and backend APIs;
- stable backup of the token-encryption key;
- access control for document, export, and release-artifact volumes;
- model-provider data-processing agreements and retention settings;
- reverse-proxy request-size, timeout, and rate limits;
- regular dependency and container updates.

## Sensitive Data

Never attach production documents, database files, Oxigraph directories, `.env` files, API tokens, encryption keys, or raw model credentials to an issue. Use synthetic reproductions and redact identifiers.

## External API Tokens

Knowledge-system tokens are scoped, revocable, and stored by hash with an encrypted revealable copy. Treat the encryption key and database as a combined secret. Rotate affected tokens after any suspected disclosure.
