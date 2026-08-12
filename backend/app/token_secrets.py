"""Authenticated encryption for owner-revealable API token secrets."""
from __future__ import annotations

import os
from functools import lru_cache
from pathlib import Path

from cryptography.fernet import Fernet, InvalidToken

from app.config import settings


class TokenSecretUnavailable(Exception):
    """The encrypted token cannot be recovered with the configured key."""


def _validated_key(value: bytes, source: str) -> bytes:
    value = value.strip()
    try:
        Fernet(value)
    except (TypeError, ValueError) as exc:
        raise RuntimeError(f"Invalid Fernet key in {source}") from exc
    return value


def _create_key_file(path: Path) -> bytes:
    key = Fernet.generate_key()
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_BINARY"):
        flags |= os.O_BINARY
    try:
        descriptor = os.open(path, flags, 0o600)
    except FileExistsError:
        return path.read_bytes()
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(key + b"\n")
    try:
        path.chmod(0o600)
    except OSError:
        pass
    return key


def _load_key() -> bytes:
    configured = settings.token_encryption_key.strip()
    if configured:
        return _validated_key(configured.encode("ascii"), "TOKEN_ENCRYPTION_KEY")

    path = settings.data_dir / "token-encryption.key"
    try:
        key = path.read_bytes()
    except FileNotFoundError:
        key = _create_key_file(path)
    return _validated_key(key, str(path))


@lru_cache
def _fernet() -> Fernet:
    return Fernet(_load_key())


def encrypt(plaintext: str) -> str:
    return _fernet().encrypt(plaintext.encode("utf-8")).decode("ascii")


def decrypt(ciphertext: str) -> str:
    try:
        return _fernet().decrypt(ciphertext.encode("ascii")).decode("utf-8")
    except (InvalidToken, UnicodeError, ValueError) as exc:
        raise TokenSecretUnavailable from exc
