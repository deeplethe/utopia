# 0069 · The identity provider vouches for listed domains

- **Status**: Implemented · 2026-10-16 · #1237, #1258, #1362 · open: no per-domain role; an SSO-opened account has no password and no reset; no global case-insensitive email uniqueness
- **Written**: 2026-10-16 (conventions in the [README](README.md))
- **Related**: migration `0056` (an identity is linked by its owner); [0066](0066-another-app-may-trade-a-fresh-id-token-for-a-session.md) (the exchange, whose mapping this widens); [0014](0014-identity-from-the-person-scope-from-the-token.md) (identity comes from the person); [#1237](https://github.com/deeplethe/utopia/pull/1237)

## Problem

Until now an identity reached an account only when its owner linked it: sign in with a password, then go through SSO from the account page (0056). A company that puts Utopia behind its own identity provider wants the opposite: everyone with a company address signs in with SSO on day one, nobody keeps a Utopia password, and the password form is gone. With linking by hand only, each of those people would first need a password account, which is what the company is trying to get rid of.

Migration 0056 rejected linking by email for a reason that still holds in general: an email is a mutable claim, and letting it decide which account a person is hands that decision to whoever controls the claim. This record says when that reason does not apply, and what the deployment then trusts.

## Decisions

1. **`UTOPIA_OIDC_ALLOWED_DOMAINS` lists the domains the identity provider is authoritative for.** On a login or an exchange whose subject is not linked, a token whose `email` is in a listed domain and whose `email_verified` is true is mapped by email: to the active account with that address (case-insensitive), or to a new one. The subject is then linked to it, so later sign-ins go by subject as before. The list is empty by default, and an empty list keeps 0056's rule exactly.

   **Revision 2026-10-22 (#1355).** The `email` claim arrives only when the authorization request asks for the `email` scope. The scopes are now `UTOPIA_OIDC_SCOPES` (default `openid profile email`), so a deployment that lists domains must keep `email` in them, or no address reaches this rule and nobody is linked or opened by domain; the server warns at startup when the list is set and `email` is missing.

   **Revision 2026-10-16 (#1258).** This assumed the active match was unique. Checking `users_email_active_idx` showed that it compares the stored spelling, while registration and administrator-created accounts preserve casing: two active accounts can differ only in email case. `account_for_sso` now refuses multiple active case-insensitive matches as `unlinked` before creating an account or writing an identity, rather than choosing one. The check covers the query snapshot; a unique index on `lower(email)`, normalization at registration and administrator creation, and a decision about existing duplicates remain separate work.

2. **The trust assumption.** Listing a domain says: every token this provider signs with a verified address in that domain belongs to the person who owns that address. That holds for a provider that only knows the company's directory. It does not hold for a plain Google OAuth client: Google also signs verified tokens for consumer accounts someone registered with a company address, and that person need not be, or still be, an employee. A deployment on such a client should put a gate in front that admits only the company's Workspace accounts, or restrict the client to its organization at the provider.

3. **The account and its link are written in one transaction** (`accounts::account_for_sso`). If the link fails (the subject is already linked to someone else, or the account already has an identity at this issuer), no account is left behind that nobody could sign in to.

4. **A deactivated account keeps its address.** The email unique index covers active accounts only, so without a check a deactivated person in a listed domain would get a fresh viewer account on the next sign-in. When the only account holding the address is deactivated, the sign-in is refused as `unlinked` and nothing is created.

5. **A new account is a viewer of the default workspace with no password**, the way registration places a second user; when the deployment has no one yet, it is the first admin. Its password hash is `!`, which no password verifies. Password login still runs a full argon2 check against a dummy hash for it, so the response time does not tell which accounts are SSO-only.

6. **The domain list overrides closed registration.** `UTOPIA_OPEN_REGISTRATION=false` and the admin toggle govern self-service password sign-up. A deployment that lists domains has already decided who may come in, and it is the same deployment that closes registration to stop strangers signing up with a password. Honouring the toggle here would make provisioning useless where it is wanted.

7. **`UTOPIA_PASSWORD_LOGIN=false` turns off password login and registration, only while SSO is configured.** Without SSO it would lock everyone out, so the server ignores it, as the login page does, and says so in a warning at startup. It also warns when password login is off and the domain list is empty, because then only people who linked an identity earlier can sign in.

8. **Keep one admin with a password.** Turning `UTOPIA_PASSWORD_LOGIN` back on is the way in when the identity provider fails, but an SSO-opened account has no password and no reset path, and that includes a first admin bootstrapped through SSO. A deployment that turns password login off should first make sure one admin account has a password.

9. **Small hardening of the claims.** `email_verified` is read leniently: `true` or the string `"true"` counts, anything else does not, because every token passes through this parser and a provider sending a string would otherwise stop every login. The `name` claim is trimmed and cut to 64 characters, and an empty one falls back to the address's local part. The exchange does not return the internal reason it refused a mapping; it logs it.

## Dead ends

- **Linking by email without a domain list.** Any provider account with a matching verified address could take over a Utopia account, including one that was registered with a password. That is 0056's objection, and it stands.
- **Respecting closed registration for new accounts.** See decision 6: it would only link people who already had a password account, which is the case the feature exists to remove.

## Open questions

- **Role.** Every provisioned account is a viewer of the default workspace. A mapping from provider groups to workspace roles would let a company skip assigning roles by hand.
- **Recovery for SSO-only accounts.** An SSO-opened account has no password and no reset path; if every admin is SSO-only and the provider is down, nobody can administer the deployment.
