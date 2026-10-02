# 0066 · Another app may trade a fresh ID token for a session

- **Status**: Implemented · 2026-10-02 · #1010, migration 0103 · open: no per-app scope; the session is a full seven-day JWT
- **Written**: 2026-10-02 (conventions in the [README](README.md))
- **Related**: [0014](0014-identity-from-the-person-scope-from-the-token.md) (identity comes from the person); migration `0056` (an identity is linked by its owner); [#1010](https://github.com/deeplethe/utopia/pull/1010)

## Problem

A second application signs its users in with the same identity provider as Utopia and needs to call Utopia's API as them: list their knowledge bases, or issue them a personal token for MCP. The only way to a Utopia session is the browser login on Utopia's own origin, which a server-side app cannot drive. Without another way, it would ask people for their Utopia password or keep a shared admin credential, and both break the rule of 0014 that what a caller does is attributed to a real person.

## Decisions

1. **`POST /api/v1/auth/oidc/exchange` takes `{ "id_token" }` and returns `{ token, expires_at }`**, the same session JWT as SSO login. The token must be signed by the configured issuer (JWKS), carry an audience from `UTOPIA_OIDC_EXCHANGE_AUDIENCES`, carry an `azp` from that list when it has one (or when it has several audiences), and have been issued in the last ten minutes.

2. **Trusted audiences, not a shared client.** The calling app uses its own client id, and Utopia lists the ids it trusts. A deployment where the app shares Utopia's client may put Utopia's own client id in the list; that is a choice the operator makes, not a default. When the list is empty, the endpoint answers 404, as it does when SSO is not configured.

3. **The mapping is the login's mapping.** Only an identity the person linked themselves (`oidc_identities`) maps to an account, and only while the account is active; otherwise `oidc_unlinked`. The exchange never creates an account and never links one. An app that can exchange tokens therefore reaches no one who has not, at some point, signed in to Utopia with a password and linked their identity.

4. **Each token is spent once.** Login makes its `state` single-use through `oidc_flows`; the exchange has no state of its own, so it records the token. `oidc_exchanges` holds the SHA-256 of the whole ID token until the token can no longer be accepted, and a second exchange gets `oidc_replayed`. The row lives until `iat` plus eleven minutes: the ten-minute freshness check has no leeway, and one more minute covers a clock difference between the server and the database. It does not follow `exp`, because `exp` is validated with a 60-second leeway, and a row that ended at `exp` would be swept while the token still verified. The hash of the whole token rather than `jti`: not every provider sends `jti`, and every token has a hash. Expired rows are swept on each exchange.

5. **A forced JWKS refresh has a cooldown.** An unknown `kid` forces a fetch from the provider, so that key rotation does not lock people out. In the login callback that fetch sits behind a validated flow; the exchange is unauthenticated, so a made-up `kid` would cost the provider a request each time. Forced refreshes of one URL are at most one per minute (`FORCE_COOLDOWN`), for login and exchange alike; a refused force falls back to the cache. The moment is recorded before the fetch, so a burst of requests sends one.

6. **No nonce.** The token was minted for the other app's flow, and Utopia never saw its nonce. The audience list, the ten-minute window and single use are what limit replay.

7. **Audited** as `auth.oidc_exchange`, with the issuer and the client (`azp`, or the single audience).

## Dead ends

- **Shared client only** (the first version of #1010). The calling app had to hold Utopia's client secret, and Utopia could not tell the app's sign-ins from its own.
- **Token exchange per RFC 8693** at the identity provider. It depends on the provider supporting it and being configured for it, and Utopia's sessions are not tokens the provider issues.
- **Creating or linking an account on first exchange.** The same reason login does not: an identity provider's claim would decide which account a person is (0056).

## Open questions

- **Scope.** The session is the person's full session for seven days. An app that only needs to list bases or mint a personal token gets everything the person can do. A narrower, shorter session for exchanged tokens would follow 0014's "scope from the token".
- **Revocation.** Sessions are stateless JWTs; unlinking the identity stops new exchanges but not sessions already issued. Deactivating the account is still the way to cut access at once.
