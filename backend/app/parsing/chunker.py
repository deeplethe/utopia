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


def chunk_text(
    text: str,
    *,
    size: int | None = None,
    overlap: int | None = None,
) -> list[ChunkSpan]:
    size = size or settings.chunk_size_chars
    overlap = overlap or settings.chunk_overlap_chars
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
        # A single oversized paragraph: hard-split it into windows.
        if plen > size:
            flush()
            p = 0
            while p < plen:
                seg_start = start + p
                seg_end = min(start + p + size, start + plen)
                seg = text[seg_start:seg_end]
                chunks.append(
                    ChunkSpan(idx, seg, seg_start, seg_end, _estimate_tokens(seg))
                )
                idx += 1
                p += size - overlap if size > overlap else size
            continue

        if buf_start is None:
            buf_start = start
        buf_end = start + plen
        buf_len = buf_end - buf_start

        if buf_len >= size:
            flush()

    flush()

    # Apply overlap by extending each chunk's start backwards into the previous chunk's tail.
    if overlap > 0:
        overlapped: list[ChunkSpan] = []
        for c in chunks:
            new_start = max(0, c.char_start - overlap) if c.idx > 0 else c.char_start
            seg = text[new_start : c.char_end]
            overlapped.append(
                ChunkSpan(c.idx, seg, new_start, c.char_end, _estimate_tokens(seg))
            )
        chunks = overlapped

    return chunks
