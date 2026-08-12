"""Document chunking.

Docling documents use its native ``HybridChunker`` so headings, tables, lists and captions
survive chunking. Lightweight parser fallbacks use a paragraph-aware greedy packer.
"""
from __future__ import annotations

import logging
import math
import re
from dataclasses import dataclass
from typing import Any

from app.config import settings

logger = logging.getLogger(__name__)

_PARA_SPLIT = re.compile(r"\n\s*\n")
_SENTENCE_END = re.compile(r"[.!?;:。！？；：](?:[\"'”’)\]]*)\s+")
_TOKEN_PIECES = re.compile(
    r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]|[A-Za-z0-9_]+|[^\s]"
)


@dataclass
class ChunkSpan:
    idx: int
    text: str
    char_start: int
    char_end: int
    token_estimate: int


def _estimate_tokens(text: str) -> int:
    """Cheap multilingual estimate without downloading a model tokenizer.

    CJK characters generally tokenize individually, while Latin/numeric runs average roughly
    four characters per token. The estimate is intentionally conservative because it controls
    the HybridChunker's hard budget as well as the number displayed in the UI.
    """
    if not text:
        return 0
    total = 0
    for piece in _TOKEN_PIECES.findall(text):
        if piece[0].isascii() and piece[0].isalnum():
            total += max(1, math.ceil(len(piece) / 4))
        else:
            total += 1
    return max(1, total)


def chunk_docling_document(document: Any, *, max_tokens: int | None = None) -> list[ChunkSpan]:
    """Structure-aware chunks from a DoclingDocument.

    Imports stay local because Docling is an optional dependency. ``contextualize`` prepends
    the active heading/caption metadata, while Docling's table serializer emits compact
    row/column facts and repeats table headers when a large table is split.
    """
    from pydantic import ConfigDict

    from docling_core.transforms.chunker.hybrid_chunker import HybridChunker
    from docling_core.transforms.chunker.tokenizer.base import BaseTokenizer

    class ApproxTokenizer(BaseTokenizer):
        model_config = ConfigDict(arbitrary_types_allowed=True)

        max_tokens: int

        def count_tokens(self, text: str) -> int:
            return _estimate_tokens(text)

        def get_max_tokens(self) -> int:
            return self.max_tokens

        def get_tokenizer(self):
            return self

    tokenizer = ApproxTokenizer(max_tokens=max_tokens or settings.chunk_size_tokens)
    docling_chunker = HybridChunker(
        tokenizer=tokenizer,
        repeat_table_header=True,
        merge_peers=True,
    )

    spans: list[ChunkSpan] = []
    cursor = 0
    for chunk in docling_chunker.chunk(document):
        text = docling_chunker.contextualize(chunk).strip()
        if not text:
            continue
        start = cursor
        end = start + len(text)
        spans.append(ChunkSpan(len(spans), text, start, end, tokenizer.count_tokens(text)))
        cursor = end + 2
    return spans


def chunk_document(text: str, structured_document: Any | None = None) -> list[ChunkSpan]:
    """Use Docling structure when available, with a safe text-only fallback."""
    if structured_document is not None:
        try:
            spans = chunk_docling_document(structured_document)
            if spans:
                return spans
        except Exception as exc:  # noqa: BLE001
            logger.warning("Docling HybridChunker failed; using paragraph fallback: %s", exc)
    return chunk_text(text)


def _preferred_end(text: str, start: int, hard_end: int, size: int) -> int:
    """Move a hard character cut back to the best nearby structural boundary."""
    if hard_end >= len(text):
        return len(text)
    floor = start + max(1, int(size * 0.6))
    if floor >= hard_end:
        floor = start + 1
    window = text[floor:hard_end]

    paragraph = window.rfind("\n\n")
    if paragraph >= 0:
        return floor + paragraph + 2
    line = window.rfind("\n")
    if line >= 0:
        return floor + line + 1

    sentence_ends = list(_SENTENCE_END.finditer(window))
    if sentence_ends:
        return floor + sentence_ends[-1].end()
    for index in range(len(window) - 1, -1, -1):
        if window[index].isspace():
            return floor + index + 1
    return hard_end


def _aligned_overlap_start(text: str, raw_start: int) -> int:
    """Align overlap context to a paragraph/sentence/line so it begins with readable context."""
    if raw_start <= 0:
        return 0
    search_start = max(0, raw_start - 400)
    window = text[search_start:raw_start]
    paragraph = window.rfind("\n\n")
    if paragraph >= 0:
        return search_start + paragraph + 2
    sentence_ends = list(_SENTENCE_END.finditer(window))
    if sentence_ends:
        return search_start + sentence_ends[-1].end()
    line = window.rfind("\n")
    if line >= 0:
        return search_start + line + 1
    for index in range(len(window) - 1, -1, -1):
        if window[index].isspace():
            return search_start + index + 1

    # Long unbroken identifiers are rare. Move forward rather than exposing a broken prefix.
    for index in range(raw_start, min(len(text), raw_start + 240)):
        if text[index].isspace():
            return index + 1
    return raw_start


def _coalesce_small_chunks(chunks: list[ChunkSpan], text: str, size: int) -> list[ChunkSpan]:
    """Fold tiny heading/tail fragments into a neighbor without creating oversized chunks."""
    minimum = max(120, int(size * 0.35))
    maximum = max(size, int(size * 1.25))
    merged: list[ChunkSpan] = []
    for chunk in chunks:
        if merged:
            previous = merged[-1]
            combined_length = chunk.char_end - previous.char_start
            if (len(previous.text) < minimum or len(chunk.text) < minimum) and combined_length <= maximum:
                combined = text[previous.char_start:chunk.char_end]
                merged[-1] = ChunkSpan(
                    previous.idx, combined, previous.char_start, chunk.char_end,
                    _estimate_tokens(combined),
                )
                continue
        merged.append(chunk)
    return [
        ChunkSpan(index, chunk.text, chunk.char_start, chunk.char_end, chunk.token_estimate)
        for index, chunk in enumerate(merged)
    ]


def chunk_text(
    text: str,
    *,
    size: int | None = None,
    overlap: int | None = None,
) -> list[ChunkSpan]:
    size = settings.chunk_size_chars if size is None else size
    overlap = settings.chunk_overlap_chars if overlap is None else overlap
    size = max(1, size)
    overlap = max(0, min(overlap, size - 1))
    if not text.strip():
        return []

    # Split into paragraphs, keeping track of absolute offsets in the original text.
    paras: list[tuple[int, str]] = []
    pos = 0
    for part in _PARA_SPLIT.split(text):
        start = text.find(part, pos)
        if start == -1:
            start = pos
        paras.append((start, part))
        pos = start + len(part)

    chunks: list[ChunkSpan] = []
    idx = 0
    buf_start: int | None = None
    buf_end = 0
    buf_len = 0

    def flush():
        nonlocal idx, buf_start, buf_end, buf_len
        if buf_start is None:
            return
        chunk_str = text[buf_start:buf_end]
        chunks.append(
            ChunkSpan(idx, chunk_str, buf_start, buf_end, _estimate_tokens(chunk_str))
        )
        idx += 1
        buf_start = None
        buf_len = 0

    for start, para in paras:
        plen = len(para)
        if not para.strip():
            continue

        # A single oversized paragraph: split near structure rather than through words/lines.
        if plen > size:
            flush()
            seg_start = start
            para_end = start + plen
            while seg_start < para_end:
                hard_end = min(seg_start + size, para_end)
                seg_end = _preferred_end(text, seg_start, hard_end, size)
                if seg_end <= seg_start:
                    seg_end = hard_end
                seg = text[seg_start:seg_end]
                chunks.append(
                    ChunkSpan(idx, seg, seg_start, seg_end, _estimate_tokens(seg))
                )
                idx += 1
                seg_start = seg_end
            continue

        candidate_end = start + plen
        if buf_start is not None and candidate_end - buf_start > size:
            flush()
        if buf_start is None:
            buf_start = start
        buf_end = candidate_end
        buf_len = buf_end - buf_start

    flush()

    chunks = _coalesce_small_chunks(chunks, text, size)

    # Apply overlap by extending each chunk's start backwards into the previous chunk's tail.
    if overlap > 0:
        overlapped: list[ChunkSpan] = []
        for c in chunks:
            raw_start = max(0, c.char_start - overlap) if c.idx > 0 else c.char_start
            new_start = _aligned_overlap_start(text, raw_start) if c.idx > 0 else c.char_start
            seg = text[new_start : c.char_end]
            overlapped.append(
                ChunkSpan(c.idx, seg, new_start, c.char_end, _estimate_tokens(seg))
            )
        chunks = overlapped

    return chunks
