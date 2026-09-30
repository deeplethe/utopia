# 0065 · The OCR reader chooses its provider

- **Status**: Implemented · 2026-09-30 · migration 0102 · open: none
- **Written**: 2026-09-30 (conventions in the [README](README.md))
- **Related**: [0040](0040-a-chunk-says-where-its-words-came-from.md) (readers and the evidence contract); [#1006](https://github.com/deeplethe/utopia/issues/1006) (the proposal, by Fonna); [#1007](https://github.com/deeplethe/utopia/pull/1007) (their implementation, which this record narrows)

## Problem

Scans and images are read by a MinerU service (0040 cut 2): the server submits a task, asks after it and takes the layout back. Volcengine Ark's Agent Plan offers no such service; it offers a vision model behind an OpenAI-shaped `chat/completions`. A model name cannot pick a wire protocol, so the OCR card needs to say which kind of service it is talking to.

The first implementation ([#1007](https://github.com/deeplethe/utopia/pull/1007)) also carried a persistent per-page checkpoint, a per-document process lock and compare-and-swap writes so that a failed page never re-pays the pages before it. That machinery was about 900 lines for a saving of a few cents per failed document.

## Decisions

1. **`llm_settings.ocr_provider` says the protocol**: `mineru` (the task service) or `ark` (a vision model). Existing rows default to `mineru`; their address, backend and key are untouched. `ocr_model` is the vision model's name and is only read on the Ark path. Ark is ready only with an address, a model and a key.
2. **A key belongs to its provider.** Saving with an empty key keeps the stored key while the provider is unchanged, and clears it when the provider changes; the comparison and the write are one SQL statement. An older client that omits the provider keeps the stored provider and any omitted model; its first save defaults to MinerU. The settings page clears the card when the provider is switched, for the same reason. The Ark card suggests the Agent Plan base URL (`https://ark.cn-beijing.volces.com/api/plan/v3`); other Ark base URLs remain valid.
3. **Ark reads a file in one pass and remembers nothing between passes.** An image is sent whole; a PDF is rendered page by page with the Poppler tools already in the runtime image (`pdftoppm -scale-to 3000`) and each page is one `chat/completions` call asking for the written text as `{"text": ...}` and nothing else. Pages become the same `Reading` as MinerU's (`mineru::reading`, real page numbers, no boxes, origin `ocr`).
4. **A page that fails for a transient reason is retried inside the pass** (429, 408, 5xx, transport errors and timeouts: three waits of 5, 15 and 45 seconds); any other failure ends the pass. A pass that fails leaves the document on its ordinary retries, and the next pass re-reads every page. No checkpoint, no lease, no lock.
5. **Limits are the reader's own**: 32 MiB per file, 100 pages, 12 MiB per page image, 1 MiB per reply, 180 s per page, 120 s per render. The reply limit is enforced while receiving, including responses without Content-Length and decompressed replies. Only PNG, JPEG, WebP and PDF are accepted, told apart by their file heads; nothing is decoded on the server.
6. **The key never leaves the request header**: the base URL may carry no credentials or query, redirects are not followed, and errors do not echo the reply.

## Consequences

- A 100-page scan is one job that may run for over an hour; the queue has no running-job timeout, so this is allowed, and it is the same shape as the MinerU wait.
- Re-reading after a failed pass costs one model call per page. If that ever matters, the fix is a retry budget per page, not a checkpoint.
- Ark transcription is not covered: the plan documents only a WebSocket interface, and this project does not add streaming clients.
