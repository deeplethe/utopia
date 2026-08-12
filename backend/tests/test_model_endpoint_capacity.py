from __future__ import annotations

import asyncio

import pytest
from pydantic import ValidationError
from sqlmodel import Session, SQLModel, create_engine

from app import model_config
from app.api.providers import ProviderIn, ProviderPatch
from app.db.models import KnowledgeSystem, Provider
from app.llm import capacity


def test_same_endpoint_never_exceeds_its_limit() -> None:
    async def scenario() -> int:
        active = 0
        peak = 0
        guard = asyncio.Lock()

        async def worker() -> None:
            nonlocal active, peak
            async with capacity.async_slot("llm:test:same", 2):
                async with guard:
                    active += 1
                    peak = max(peak, active)
                await asyncio.sleep(0.03)
                async with guard:
                    active -= 1

        await asyncio.gather(*(worker() for _ in range(6)))
        return peak

    assert asyncio.run(scenario()) == 2


def test_different_endpoints_have_independent_capacity() -> None:
    async def scenario() -> None:
        first_entered = asyncio.Event()
        second_entered = asyncio.Event()
        release = asyncio.Event()

        async def worker(key: str, entered: asyncio.Event) -> None:
            async with capacity.async_slot(key, 1):
                entered.set()
                await release.wait()

        first = asyncio.create_task(worker("llm:test:first", first_entered))
        second = asyncio.create_task(worker("llm:test:second", second_entered))
        await asyncio.wait_for(
            asyncio.gather(first_entered.wait(), second_entered.wait()),
            timeout=0.5,
        )
        release.set()
        await asyncio.gather(first, second)

    asyncio.run(scenario())


def test_nested_slot_for_same_endpoint_reuses_reservation() -> None:
    async def scenario() -> None:
        async with capacity.async_slot("llm:test:nested", 1):
            async with capacity.async_slot("llm:test:nested", 1):
                return

    asyncio.run(asyncio.wait_for(scenario(), timeout=0.5))


def test_llm_and_embedding_endpoint_resolution_keeps_limits_separate() -> None:
    engine = create_engine("sqlite://", connect_args={"check_same_thread": False})
    SQLModel.metadata.create_all(engine)
    with Session(engine) as session:
        llm = Provider(
            name="shared service / chat",
            kind="llm",
            base_url="https://models.example/v1",
            model="chat-model",
            concurrency_limit=2,
        )
        embedding = Provider(
            name="shared service / embedding",
            kind="embedding",
            base_url="https://models.example/v1",
            model="embedding-model",
            concurrency_limit=7,
        )
        session.add(llm)
        session.add(embedding)
        session.commit()
        session.refresh(llm)
        session.refresh(embedding)
        ks = KnowledgeSystem(
            name="capacity test",
            llm_provider_id=llm.id,
            embedding_provider_id=embedding.id,
        )
        session.add(ks)
        session.commit()
        session.refresh(ks)

        with model_config.use_ks_connections(session, ks):
            assert model_config.llm_concurrency() == 2
            assert model_config.embedding_concurrency() == 7
            assert model_config.llm_capacity_key() == f"llm:provider:{llm.id}"
            assert model_config.embedding_capacity_key() == f"embedding:provider:{embedding.id}"
            assert model_config.llm_capacity_key() != model_config.embedding_capacity_key()


@pytest.mark.parametrize("value", [0, 65])
def test_provider_api_rejects_out_of_range_limit(value: int) -> None:
    with pytest.raises(ValidationError):
        ProviderIn(name="bad", concurrency_limit=value)
    with pytest.raises(ValidationError):
        ProviderPatch(concurrency_limit=value)
