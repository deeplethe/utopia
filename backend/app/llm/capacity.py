"""Process-wide concurrency buckets keyed by configured model endpoint."""
from __future__ import annotations

import asyncio
import contextlib
import contextvars
import threading
from collections.abc import AsyncIterator, Iterator

_condition = threading.Condition()
_active: dict[str, int] = {}
_held_keys: contextvars.ContextVar[tuple[str, ...]] = contextvars.ContextVar(
    "model_capacity_held_keys", default=(),
)


def _try_acquire(key: str, limit: int) -> bool:
    with _condition:
        active = _active.get(key, 0)
        if active >= max(1, limit):
            return False
        _active[key] = active + 1
        return True


async def _acquire_async(key: str, limit: int) -> None:
    while not _try_acquire(key, limit):
        await asyncio.sleep(0.05)


def _acquire_sync(key: str, limit: int) -> None:
    with _condition:
        while _active.get(key, 0) >= max(1, limit):
            _condition.wait(timeout=0.5)
        _active[key] = _active.get(key, 0) + 1


def _release(key: str) -> None:
    with _condition:
        remaining = max(0, _active.get(key, 0) - 1)
        if remaining:
            _active[key] = remaining
        else:
            _active.pop(key, None)
        _condition.notify_all()


@contextlib.asynccontextmanager
async def async_slot(key: str, limit: int) -> AsyncIterator[None]:
    """Acquire one async slot for an endpoint; nested calls for that endpoint reuse it."""
    held = _held_keys.get()
    if key in held:
        yield
        return
    await _acquire_async(key, limit)
    token = _held_keys.set((*held, key))
    try:
        yield
    finally:
        _held_keys.reset(token)
        _release(key)


@contextlib.contextmanager
def sync_slot(key: str, limit: int) -> Iterator[None]:
    """Acquire one blocking slot for an endpoint; nested calls for that endpoint reuse it."""
    held = _held_keys.get()
    if key in held:
        yield
        return
    _acquire_sync(key, limit)
    token = _held_keys.set((*held, key))
    try:
        yield
    finally:
        _held_keys.reset(token)
        _release(key)
