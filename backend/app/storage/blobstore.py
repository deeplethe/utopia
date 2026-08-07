"""Content-addressed blob store.

Files are stored by the SHA-256 of their bytes, sharded two levels deep to keep any
single directory small:  ``blobs/<aa>/<bb>/<full-sha256>``  where ``aa`` = first two
hex chars, ``bb`` = next two. Identical content is stored exactly once (idempotent).
"""
from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

from app.config import settings


@dataclass(frozen=True)
class StoredBlob:
    sha256: str
    relative_path: str  # e.g. "aa/bb/<sha256>", POSIX-style
    size_bytes: int
    existed: bool  # True if identical content was already stored


def _sharded_relpath(sha256: str) -> str:
    return f"{sha256[:2]}/{sha256[2:4]}/{sha256}"


def store_bytes(data: bytes) -> StoredBlob:
    """Store raw bytes, returning content hash and sharded relative path."""
    sha256 = hashlib.sha256(data).hexdigest()
    rel = _sharded_relpath(sha256)
    dest = settings.blob_dir / rel
    if dest.exists():
        return StoredBlob(sha256, rel, dest.stat().st_size, existed=True)

    dest.parent.mkdir(parents=True, exist_ok=True)
    # Write atomically: temp file in the same shard dir, then rename.
    tmp = dest.with_suffix(".tmp")
    tmp.write_bytes(data)
    tmp.replace(dest)
    return StoredBlob(sha256, rel, len(data), existed=False)


def abs_path(relative_path: str) -> Path:
    """Absolute filesystem path for a stored blob's relative path."""
    return settings.blob_dir / relative_path


def read_bytes(relative_path: str) -> bytes:
    return abs_path(relative_path).read_bytes()


def delete(relative_path: str) -> bool:
    """Delete a stored blob file. Returns True if a file was removed."""
    dest = abs_path(relative_path)
    if dest.exists():
        dest.unlink()
        return True
    return False
