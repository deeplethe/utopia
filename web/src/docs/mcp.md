# Agents over MCP

Utopia serves every knowledge base as a **Model Context Protocol** server. Any MCP client — Claude Desktop, Cursor, an agent framework, a script — can search a base, read a document, look up an entity and ask what changed, with the same permissions as the person whose token it carries. This version exposes **read tools only**; nothing an agent does over MCP changes the base.

## Get a token

Tokens belong to people, not to bases: **Account → Agents & tokens → Personal access tokens**.

| Field | Meaning |
|---|---|
| Name | For your own bookkeeping; shows in the token list and in the audit log |
| Scope | `read` (default) or `write`. Scope is a **ceiling**, not a grant: a token can never do more than its owner can, and this version's MCP tools are read-only even for a `write` token |
| Knowledge bases | Optional. Leave empty and the token reaches every base its owner can open; pick some to narrow it |
| Expires in | 90 days by default |

The token value (`utp_pat_…`) is shown **once**, when it is issued. Revoking it takes effect on the next call.

Anything an agent does with the token is recorded in the base's Activity as the token's owner, with the tool name, so a token is never a way around the audit trail.

## The endpoint

```
POST /api/v1/kbs/{kb_id}/mcp
Authorization: Bearer utp_pat_…
Content-Type: application/json
```

One endpoint per knowledge base, speaking JSON-RPC 2.0 (MCP protocol version `2025-06-18`, stateless HTTP). The `kb_id` is in the base's URL in the browser: `/kb/{kb_id}/…`.

Three methods are served:

- `initialize` — capabilities and protocol version
- `tools/list` — the tools below, with their JSON schemas
- `tools/call` — run one

## The tools

| Tool | What it answers |
|---|---|
| `search_chunks` | Full-text + semantic search over the base's documents. Returns the six best-matching passages, each cut at 800 characters and carrying its `document_id` |
| `get_document` | The full text of one document, all sections in order, by `document_id`. Use it when a search hit is the right document but the excerpt does not carry the answer. Capped at 24,000 characters, and says so when it cuts |
| `find_entities` | Entities by (partial) name: id, type, and a disambiguator when several share a name |
| `entity_facts` | One entity's facts with validity ranges. Pass `at` (a date) to see the world as of that day; this is the tool for "who was X in 2024" |
| `changes` | What the graph learned or revised in a window of **record** time: asserted, corrected, rejected, merged. Needs no entity; use it when the question names a period, not a subject |
| `search_docs` | Utopia's own manual, for questions about how the platform works. Never the user's documents |

The two time axes matter here. `entity_facts` reads **world time** (when something was true); `changes` reads **record time** (when Utopia came to believe it, and when it revised that belief). An agent that confuses them will answer "what happened in March" with "what we learned in March".

## Try it from a shell

List the tools:

```bash
curl -s -X POST https://your-utopia/api/v1/kbs/$KB/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

Search, then read the whole document the best hit came from:

```bash
curl -s -X POST https://your-utopia/api/v1/kbs/$KB/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call",
       "params":{"name":"search_chunks","arguments":{"query":"Series C target"}}}'

curl -s -X POST https://your-utopia/api/v1/kbs/$KB/mcp \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call",
       "params":{"name":"get_document","arguments":{"document_id":"<id from the hit>"}}}'
```

A document id from another base, or a made-up one, comes back as "No document with that id in this knowledge base" — the tool cannot tell you what it is not allowed to see.

## Point a client at it

Most MCP clients accept a remote server as a URL plus headers. For a client that reads a JSON config:

```json
{
  "mcpServers": {
    "utopia-general": {
      "url": "https://your-utopia/api/v1/kbs/<kb_id>/mcp",
      "headers": { "Authorization": "Bearer utp_pat_…" }
    }
  }
}
```

One entry per knowledge base you want the agent to reach. The agent sees the same base the token's owner sees, and nothing else.

## What is not here yet

- **Writing.** `remember` (recording an episode) and `query_data` (SQL over a mounted database) are chat tools today and are not served over MCP. When they are, the write path will go through the same confirmation step a person's own "remember" does: an agent proposes, a person confirms, and only then does anything enter the graph.
- **Streaming.** Responses are single JSON-RPC replies; there is no server-sent event channel.
