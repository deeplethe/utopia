"""Run a leakage-aware OntoLearner benchmark against public real wine reviews.

The OntoLearner JSON files are used only as a gold label/taxonomy set. They are
never verbalised into the extraction corpus. Gold labels are allowed as search
queries, while the downloaded article prose is the only input sent to OntoPilot.

Examples (run from ``backend``):

    python scripts/benchmark_ontolearner_realtext.py prepare --reviews 1200
    python scripts/benchmark_ontolearner_realtext.py ingest
    python scripts/benchmark_ontolearner_realtext.py extract --ks-id 7
    python scripts/benchmark_ontolearner_realtext.py score --ks-id 7

Use ``run`` to execute all four phases. The corpus, state, and reports live under
``data/benchmarks/ontolearner-wine-realtext`` (ignored by git). A Wikipedia API
source remains available as an optional fallback for networks that allow it.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import unicodedata
from collections import Counter
from collections.abc import Iterable
from datetime import UTC, datetime
from pathlib import Path
from urllib.parse import quote

import httpx


SCRIPT_DIR = Path(__file__).resolve().parent
BACKEND_DIR = SCRIPT_DIR.parent
DEFAULT_GOLD_DIR = BACKEND_DIR / "data" / "benchmarks" / "ontolearner-food_and_beverage" / "wine"
DEFAULT_RUN_DIR = BACKEND_DIR / "data" / "benchmarks" / "ontolearner-wine-realtext"
DEFAULT_REVIEWS_PARQUET = (
    BACKEND_DIR
    / "data"
    / "benchmarks"
    / "wine-reviews"
    / "data"
    / "validation-00000-of-00001.parquet"
)
DEFAULT_BASE_URL = "http://127.0.0.1:8000"
WIKIPEDIA_API = "https://en.wikipedia.org/w/api.php"
USER_AGENT = "OntoPilotBenchmark/1.0 (https://github.com/deeplethe/ontopilot)"

CORE_QUERIES = [
    "Wine",
    "Winemaking",
    "Viticulture",
    "Wine tasting",
    "Classification of wine",
    "Glossary of wine terms",
    "Wine color",
    "Sweetness of wine",
    "Wine fault",
    "Wine and food matching",
    "Red wine",
    "White wine",
    "Rosé wine",
    "Dessert wine",
    "Late harvest wine",
    "Port wine",
    "Sauternes wine",
    "Bordeaux wine",
    "Chianti wine",
    "Riesling wine",
    "Italian wine",
    "French wine",
    "Grape",
    "Wine grape",
    "Winery",
    "Vineyard",
    "Oenology",
    "Terroir",
]
RELEVANCE_TERMS = (
    "wine",
    "winery",
    "wines",
    "grape",
    "vineyard",
    "viticulture",
    "winemaking",
    "oenology",
    "sommelier",
    "bordeaux",
    "riesling",
    "chianti",
)
_CAMEL_BOUNDARY = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")
_NON_WORD = re.compile(r"[^a-z0-9]+")


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


def read_json(path: Path) -> dict | list:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def normalise_label(value: str) -> str:
    value = _CAMEL_BOUNDARY.sub(" ", value or "")
    value = unicodedata.normalize("NFKD", value).encode("ascii", "ignore").decode("ascii")
    return _NON_WORD.sub(" ", value.lower()).strip()


def display_label(value: str) -> str:
    return _CAMEL_BOUNDARY.sub(" ", value).replace("_", " ").strip()


def slugify(value: str) -> str:
    slug = _NON_WORD.sub("-", normalise_label(value)).strip("-")
    return slug[:80] or "article"


def unique(values: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        key = normalise_label(value)
        if key and key not in seen:
            seen.add(key)
            result.append(value)
    return result


def load_gold(gold_dir: Path) -> dict:
    taxonomy_payload = read_json(gold_dir / "type_taxonomies.json")
    typing_payload = read_json(gold_dir / "term_typings.json")
    if not isinstance(taxonomy_payload, dict) or not isinstance(typing_payload, list):
        raise ValueError(f"Unexpected OntoLearner data shape in {gold_dir}")

    types = {normalise_label(value) for value in taxonomy_payload.get("types", [])}
    taxonomy = {
        (normalise_label(row["child"]), normalise_label(row["parent"]))
        for row in taxonomy_payload.get("taxonomies", [])
    }
    taxonomy.discard(("", ""))
    return {
        "types": types,
        "taxonomy": taxonomy,
        "type_labels": taxonomy_payload.get("types", []),
        "term_labels": [row["term"] for row in typing_payload if row.get("term")],
    }


class WikipediaClient:
    def __init__(self) -> None:
        self.client = httpx.Client(
            timeout=60,
            follow_redirects=True,
            trust_env=False,
            headers={"User-Agent": USER_AGENT, "Accept": "application/json"},
        )

    def close(self) -> None:
        self.client.close()

    def _get(self, params: dict) -> dict:
        response = self.client.get(WIKIPEDIA_API, params={"format": "json", "formatversion": 2, **params})
        response.raise_for_status()
        return response.json()

    def search(self, query: str, limit: int = 5) -> list[str]:
        payload = self._get(
            {
                "action": "query",
                "list": "search",
                "srsearch": query,
                "srnamespace": 0,
                "srlimit": limit,
                "srprop": "",
            }
        )
        return [row["title"] for row in payload.get("query", {}).get("search", [])]

    def pages(self, titles: list[str]) -> list[dict]:
        if not titles:
            return []
        payload = self._get(
            {
                "action": "query",
                "prop": "extracts|info|revisions",
                "titles": "|".join(titles),
                "redirects": 1,
                "inprop": "url",
                "rvprop": "ids|timestamp",
                "rvlimit": 1,
                "explaintext": 1,
                "exsectionformat": "plain",
            }
        )
        return [page for page in payload.get("query", {}).get("pages", []) if not page.get("missing")]


def relevance_score(page: dict, query: str) -> int:
    title = normalise_label(page.get("title", ""))
    text = normalise_label(page.get("extract", "")[:6000])
    query_terms = set(normalise_label(query).split()) - {"wine", "wines"}
    score = sum(2 for term in RELEVANCE_TERMS if term in text)
    score += 6 if "wine" in title or "winery" in title else 0
    score += sum(2 for term in query_terms if len(term) > 3 and term in title)
    score += sum(1 for term in query_terms if len(term) > 3 and term in text)
    return score


def trim_article(text: str, max_chars: int) -> str:
    text = text.replace("\r\n", "\n").strip()
    if len(text) <= max_chars:
        return text
    cut = text.rfind("\n\n", 0, max_chars)
    if cut < max_chars // 2:
        cut = text.rfind(". ", 0, max_chars)
    return text[: cut + 1 if cut > 0 else max_chars].strip()


def article_document(article: dict) -> str:
    return (
        f"Title: {article['title']}\n"
        f"Source: {article['source_url']}\n"
        f"Revision: {article['revision_url']}\n"
        f"Retrieved: {article['retrieved_at']}\n"
        "License: Wikipedia text, CC BY-SA 4.0; attribution via the source and revision links.\n\n"
        f"{article['text']}\n"
    )


def reset_corpus_dir(run_dir: Path) -> Path:
    corpus_dir = run_dir / "corpus"
    corpus_dir.mkdir(parents=True, exist_ok=True)
    for stale_file in corpus_dir.glob("*.txt"):
        stale_file.unlink()
    return corpus_dir


def prepare_wikipedia_corpus(gold_dir: Path, run_dir: Path, pages: int, max_chars: int) -> dict:
    gold = load_gold(gold_dir)
    corpus_dir = reset_corpus_dir(run_dir)

    queries = unique(
        [
            *CORE_QUERIES,
            *(f"{display_label(label)} wine" for label in gold["type_labels"]),
            *(f"{display_label(label)} wine" for label in gold["term_labels"]),
        ]
    )
    wiki = WikipediaClient()
    selected: list[dict] = []
    seen_page_ids: set[int] = set()
    try:
        for index, query in enumerate(queries, start=1):
            if len(selected) >= pages:
                break
            try:
                candidates = wiki.pages(wiki.search(query))
            except (httpx.HTTPError, ValueError) as exc:
                print(f"[prepare] query failed: {query!r}: {exc}", flush=True)
                time.sleep(1)
                continue

            candidates = [
                page
                for page in candidates
                if page.get("pageid") not in seen_page_ids and len(page.get("extract", "")) >= 1200
            ]
            if not candidates:
                continue
            candidates.sort(key=lambda page: relevance_score(page, query), reverse=True)
            page = candidates[0]
            if relevance_score(page, query) < 4:
                continue

            revision = (page.get("revisions") or [{}])[0]
            text = trim_article(page["extract"], max_chars)
            article = {
                "index": len(selected) + 1,
                "page_id": page["pageid"],
                "revision_id": revision.get("revid"),
                "revision_timestamp": revision.get("timestamp"),
                "title": page["title"],
                "query": query,
                "source_url": page.get("canonicalurl") or page.get("fullurl"),
                "revision_url": (
                    f"https://en.wikipedia.org/w/index.php?title={quote(page['title'].replace(' ', '_'))}"
                    f"&oldid={revision.get('revid')}"
                ),
                "retrieved_at": now_iso(),
                "license": "CC BY-SA 4.0",
                "chars": len(text),
                "text": text,
            }
            filename = f"{article['index']:03d}-{slugify(page['title'])}.txt"
            article["filename"] = filename
            (corpus_dir / filename).write_text(article_document(article), encoding="utf-8")
            selected.append(article)
            seen_page_ids.add(page["pageid"])
            print(
                f"[prepare] {len(selected):>3}/{pages}: {page['title']} ({len(text):,} chars)",
                flush=True,
            )
            time.sleep(0.08)
    finally:
        wiki.close()

    if len(selected) < pages:
        raise RuntimeError(f"Only found {len(selected)} suitable articles; requested {pages}")

    manifest_articles = [{key: value for key, value in row.items() if key != "text"} for row in selected]
    manifest = {
        "benchmark": "OntoLearner Wine + English Wikipedia real-text hybrid",
        "benchmark_kind": "hybrid_real_text_not_official_text2onto",
        "created_at": now_iso(),
        "selection_protocol": (
            "OntoLearner labels were used only as Wikipedia search queries. Gold taxonomy edges "
            "were never included in the extraction corpus."
        ),
        "gold": {
            "source": "SciKnowOrg/ontolearner-food_and_beverage",
            "ontology": "wine",
            "license": "MIT",
            "type_count": len(gold["types"]),
            "taxonomy_edge_count": len(gold["taxonomy"]),
        },
        "corpus": {
            "source": "English Wikipedia",
            "license": "CC BY-SA 4.0",
            "article_count": len(selected),
            "text_chars": sum(row["chars"] for row in selected),
            "max_chars_per_article": max_chars,
        },
        "documents": manifest_articles,
    }
    write_json(run_dir / "manifest.json", manifest)
    (run_dir / "CORPUS-LICENSE.md").write_text(
        "# Corpus licensing\n\n"
        "The files in `corpus/` contain excerpts from English Wikipedia and are reused under "
        "[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). Each document and "
        "`manifest.json` retain the article URL and exact revision URL for attribution.\n\n"
        "The OntoLearner Wine benchmark files are distributed by SciKnowOrg under the MIT license.\n",
        encoding="utf-8",
    )
    print(
        f"[prepare] complete: {len(selected)} articles / {manifest['corpus']['text_chars']:,} chars",
        flush=True,
    )
    return manifest


def review_value(row: dict, key: str, fallback: str = "") -> str:
    value = row.get(key)
    if value is None:
        return fallback
    text = str(value).strip()
    return text if text and text.lower() != "nan" else fallback


def review_search_text(row: dict) -> str:
    return normalise_label(
        " ".join(
            review_value(row, key)
            for key in ("title", "description", "designation", "country", "province", "region_1", "region_2", "variety", "winery")
        )
    )


def select_reviews(rows: list[dict], gold_types: set[str], count: int) -> list[tuple[int, dict]]:
    """Select deterministically without using gold edges.

    First reserve examples mentioning each gold type, then round-robin over grape varieties so
    the remaining corpus is broad rather than dominated by the most frequent class.
    """
    indexed = list(enumerate(rows))
    searchable = [(index, row, review_search_text(row)) for index, row in indexed]
    selected: list[tuple[int, dict]] = []
    selected_indexes: set[int] = set()

    for gold_type in sorted(gold_types):
        phrase = f" {gold_type} "
        matches = [
            (index, row)
            for index, row, text in searchable
            if index not in selected_indexes and phrase in f" {text} "
        ][:20]
        for index, row in matches:
            selected.append((index, row))
            selected_indexes.add(index)
            if len(selected) >= count:
                return selected

    by_variety: dict[str, list[tuple[int, dict]]] = {}
    for index, row in indexed:
        if index in selected_indexes:
            continue
        variety = normalise_label(review_value(row, "variety", "unknown")) or "unknown"
        by_variety.setdefault(variety, []).append((index, row))

    varieties = sorted(by_variety, key=lambda value: (-len(by_variety[value]), value))
    offset = 0
    while len(selected) < count:
        added = False
        for variety in varieties:
            bucket = by_variety[variety]
            if offset < len(bucket):
                selected.append(bucket[offset])
                added = True
                if len(selected) >= count:
                    break
        if not added:
            break
        offset += 1
    if len(selected) < count:
        raise RuntimeError(f"Dataset contains only {len(selected)} selectable rows; requested {count}")
    return selected


def format_review(record_number: int, source_row: int, row: dict) -> str:
    location = ", ".join(
        value
        for value in (
            review_value(row, "country"),
            review_value(row, "province"),
            review_value(row, "region_1"),
            review_value(row, "region_2"),
        )
        if value
    )
    fields = [
        f"Review {record_number} (source row {source_row})",
        f"Title: {review_value(row, 'title', 'Untitled wine')}",
        f"Wine variety: {review_value(row, 'variety', 'Unknown')}",
        f"Winery: {review_value(row, 'winery', 'Unknown')}",
        f"Designation: {review_value(row, 'designation', 'Unspecified')}",
        f"Origin: {location or 'Unknown'}",
        f"Review text: {review_value(row, 'description', 'No description')}",
    ]
    points = review_value(row, "points")
    price = review_value(row, "price")
    if points:
        fields.append(f"Rating: {points} points")
    if price:
        fields.append(f"Price: {price} USD")
    return "\n".join(fields)


def prepare_wine_reviews_corpus(
    gold_dir: Path,
    run_dir: Path,
    parquet_path: Path,
    reviews: int,
    reviews_per_document: int,
) -> dict:
    try:
        import pyarrow.parquet as parquet
    except ImportError as exc:
        raise RuntimeError("Reading Wine Reviews requires `pip install pyarrow`") from exc
    if not parquet_path.exists():
        raise FileNotFoundError(
            f"Wine Reviews parquet not found: {parquet_path}. Clone "
            "https://huggingface.co/datasets/spawn99/wine-reviews first."
        )

    gold = load_gold(gold_dir)
    corpus_dir = reset_corpus_dir(run_dir)
    rows = parquet.read_table(parquet_path).to_pylist()
    selected = select_reviews(rows, gold["types"], reviews)
    documents: list[dict] = []
    all_text_chars = 0

    for start in range(0, len(selected), reviews_per_document):
        batch = selected[start : start + reviews_per_document]
        document_index = len(documents) + 1
        body = "\n\n".join(
            format_review(start + offset + 1, source_row, row)
            for offset, (source_row, row) in enumerate(batch)
        )
        text = (
            "Dataset: Wine Reviews\n"
            "Source: https://www.kaggle.com/datasets/zynicide/wine-reviews\n"
            "Mirror: https://huggingface.co/datasets/spawn99/wine-reviews\n"
            "License: CC BY-NC-SA 4.0\n"
            "Attribution: Zackthoutt; reviews originally collected from Wine Enthusiast.\n\n"
            f"{body}\n"
        )
        filename = f"reviews-{document_index:03d}.txt"
        (corpus_dir / filename).write_text(text, encoding="utf-8")
        source_rows = [source_row for source_row, _ in batch]
        documents.append(
            {
                "index": document_index,
                "filename": filename,
                "record_count": len(batch),
                "source_rows": source_rows,
                "chars": len(text),
            }
        )
        all_text_chars += len(text)
        print(
            f"[prepare] {document_index:>3}: {filename} / {len(batch)} reviews / {len(text):,} chars",
            flush=True,
        )

    manifest = {
        "benchmark": "OntoLearner Wine + public Wine Reviews real-text hybrid",
        "benchmark_kind": "hybrid_real_text_not_official_text2onto",
        "created_at": now_iso(),
        "selection_protocol": (
            "OntoLearner type labels were used only to reserve matching review records. Remaining "
            "records were selected deterministically by round-robin over wine varieties. Gold "
            "taxonomy edges were never included in the extraction corpus."
        ),
        "gold": {
            "source": "SciKnowOrg/ontolearner-food_and_beverage",
            "ontology": "wine",
            "license": "MIT",
            "type_count": len(gold["types"]),
            "taxonomy_edge_count": len(gold["taxonomy"]),
        },
        "corpus": {
            "source": "Wine Reviews (Zackthoutt / Wine Enthusiast)",
            "mirror": "spawn99/wine-reviews",
            "source_split": parquet_path.name,
            "license": "CC BY-NC-SA 4.0",
            "document_count": len(documents),
            "record_count": len(selected),
            "text_chars": all_text_chars,
            "records_per_document": reviews_per_document,
        },
        "documents": documents,
    }
    write_json(run_dir / "manifest.json", manifest)
    (run_dir / "CORPUS-LICENSE.md").write_text(
        "# Corpus licensing\n\n"
        "The files in `corpus/` are derived from the Wine Reviews dataset by Zackthoutt, "
        "originally collected from Wine Enthusiast, and reused for non-commercial benchmark "
        "evaluation under [CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/). "
        "The exact source row indexes are retained in `manifest.json`.\n\n"
        "The OntoLearner Wine benchmark files are distributed by SciKnowOrg under the MIT license.\n",
        encoding="utf-8",
    )
    print(
        f"[prepare] complete: {len(documents)} documents / {len(selected)} reviews / "
        f"{all_text_chars:,} chars",
        flush=True,
    )
    return manifest


def prepare_corpus(
    gold_dir: Path,
    run_dir: Path,
    source: str,
    pages: int,
    max_chars: int,
    parquet_path: Path,
    reviews: int,
    reviews_per_document: int,
) -> dict:
    if source == "wikipedia-api":
        return prepare_wikipedia_corpus(gold_dir, run_dir, pages, max_chars)
    return prepare_wine_reviews_corpus(
        gold_dir,
        run_dir,
        parquet_path,
        reviews,
        reviews_per_document,
    )


class OntoPilotClient:
    def __init__(self, base_url: str, username: str, password: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.client = httpx.Client(
            base_url=self.base_url,
            timeout=120,
            follow_redirects=True,
            trust_env=False,
        )
        response = self.client.post("/api/auth/login", json={"username": username, "password": password})
        response.raise_for_status()

    def close(self) -> None:
        self.client.close()

    def get(self, path: str) -> object:
        response = self.client.get(path)
        response.raise_for_status()
        return response.json()

    def post(self, path: str, **kwargs) -> object:
        response = self.client.post(path, **kwargs)
        response.raise_for_status()
        return response.json()


def load_state(run_dir: Path) -> dict:
    path = run_dir / "run-state.json"
    return read_json(path) if path.exists() else {}


def save_state(run_dir: Path, state: dict) -> None:
    state["updated_at"] = now_iso()
    write_json(run_dir / "run-state.json", state)


def ingest_corpus(client: OntoPilotClient, run_dir: Path, name: str, model: str | None) -> dict:
    manifest_path = run_dir / "manifest.json"
    if not manifest_path.exists():
        raise FileNotFoundError(f"Prepare the corpus first: {manifest_path}")
    manifest = read_json(manifest_path)
    state = load_state(run_dir)
    if state.get("ks_id"):
        raise RuntimeError(f"This run directory is already bound to knowledge system {state['ks_id']}")

    description = (
        "Public capability test: OntoLearner Wine taxonomy gold (MIT) evaluated against real "
        f"{manifest['corpus']['source']} text ({manifest['corpus']['license']}), "
        f"{manifest['corpus'].get('record_count', manifest['corpus'].get('article_count'))} records. "
        "Gold edges are held out and never injected into the corpus."
    )
    body = {"name": name, "description": description}
    if model:
        body["llm_model"] = model
    ks = client.post("/api/knowledge", json=body)
    state = {
        "ks_id": ks["id"],
        "ks_public_id": ks["public_id"],
        "ks_name": ks["name"],
        "graph_iri": ks["graph_iri"],
        "created_at": now_iso(),
        "documents": [],
    }
    save_state(run_dir, state)
    print(f"[ingest] created KS {ks['id']}: {ks['name']}", flush=True)

    documents = manifest["documents"]
    corpus_dir = run_dir / "corpus"
    for index, document_meta in enumerate(documents, start=1):
        path = corpus_dir / document_meta["filename"]
        with path.open("rb") as handle:
            document = client.post(
                f"/api/knowledge/{ks['id']}/documents/upload",
                files={"file": (path.name, handle, "text/plain; charset=utf-8")},
                data={"folder": "/benchmark/wikipedia"},
            )
        parsed = client.post(f"/api/knowledge/{ks['id']}/documents/{document['id']}/parse")
        if parsed["parse_status"] != "parsed":
            raise RuntimeError(f"Parsing failed for {path.name}: {parsed.get('error')}")
        state["documents"].append(
            {
                "document_id": document["id"],
                "filename": path.name,
                "chars": parsed["text_char_count"],
                "chunks": parsed["chunk_count"],
            }
        )
        save_state(run_dir, state)
        print(
            f"[ingest] {index:>3}/{len(documents)}: {path.name} -> {parsed['chunk_count']} chunks",
            flush=True,
        )

    state["document_count"] = len(state["documents"])
    state["text_chars"] = sum(row["chars"] for row in state["documents"])
    state["chunk_count"] = sum(row["chunks"] for row in state["documents"])
    state["ingested_at"] = now_iso()
    save_state(run_dir, state)
    print(
        f"[ingest] complete: {state['document_count']} documents / {state['chunk_count']} chunks",
        flush=True,
    )
    return state


def collect_chunk_ids(client: OntoPilotClient, ks_id: int, state: dict) -> list[int]:
    chunk_ids: list[int] = []
    for document in state.get("documents", []):
        chunks = client.get(f"/api/knowledge/{ks_id}/documents/{document['document_id']}/chunks")
        chunk_ids.extend(chunk["id"] for chunk in chunks)
    return chunk_ids


def extract_corpus(
    client: OntoPilotClient,
    run_dir: Path,
    ks_id: int,
    model: str | None,
    max_chunks: int,
    timeout_seconds: int,
    mode: str = "tbox",
    agentic_resolution: bool | None = None,
) -> dict:
    state = load_state(run_dir)
    if state.get("ks_id") != ks_id:
        raise RuntimeError(f"Run state belongs to KS {state.get('ks_id')}, not {ks_id}")
    chunk_ids = collect_chunk_ids(client, ks_id, state)
    if max_chunks:
        chunk_ids = chunk_ids[:max_chunks]
    if not chunk_ids:
        raise RuntimeError("No parsed chunks found")

    payload: dict[str, object] = {"chunk_ids": chunk_ids}
    if model:
        payload["model"] = model
    if agentic_resolution is not None:
        payload["agentic_resolution"] = agentic_resolution
    started = time.monotonic()
    endpoint = {
        "tbox": "extract",
        "both": "extract-all",
        "abox": "extract-instances",
    }[mode]
    job = client.post(f"/api/knowledge/{ks_id}/{endpoint}", json=payload)
    state["job_id"] = job["id"]
    state["extraction_mode"] = mode
    state["agentic_resolution"] = agentic_resolution
    state["extraction_started_at"] = now_iso()
    state["extracted_chunk_count"] = len(chunk_ids)
    save_state(run_dir, state)
    print(f"[extract:{mode}] job {job['id']} started for {len(chunk_ids)} chunks", flush=True)

    last_progress = (-1, "")
    while time.monotonic() - started < timeout_seconds:
        job = client.get(f"/api/knowledge/{ks_id}/jobs/{job['id']}")
        progress = (job.get("processed_chunks", 0), job.get("phase", ""))
        if progress != last_progress:
            print(
                f"[extract:{mode}] {progress[0]}/{job['total_chunks']} chunks; phase={progress[1] or job['status']}",
                flush=True,
            )
            last_progress = progress
        if job["status"] in {"completed", "failed"}:
            break
        time.sleep(3)
    else:
        raise TimeoutError(f"Extraction job {job['id']} exceeded {timeout_seconds} seconds")

    state["extraction_finished_at"] = now_iso()
    state["extraction_elapsed_seconds"] = round(time.monotonic() - started, 2)
    state["job"] = job
    save_state(run_dir, state)
    if job["status"] != "completed":
        raise RuntimeError(f"Extraction job {job['id']} failed: {job.get('error')}")
    print(
        f"[extract:{mode}] complete in {state['extraction_elapsed_seconds']}s: "
        f"+{job['classes_added']} classes / +{job['properties_added']} properties / "
        f"+{job['axioms_added']} axioms / +{job.get('individuals_added', 0)} individuals / "
        f"+{job.get('assertions_added', 0)} assertions",
        flush=True,
    )
    return state


def score_pairs(predicted: set[tuple[str, str]], gold: set[tuple[str, str]]) -> dict:
    true_positive = len(predicted & gold)
    false_positive = len(predicted - gold)
    false_negative = len(gold - predicted)
    precision = true_positive / (true_positive + false_positive) if predicted else 0.0
    recall = true_positive / (true_positive + false_negative) if gold else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return {
        "precision": round(precision, 4),
        "recall": round(recall, 4),
        "f1": round(f1, 4),
        "tp": true_positive,
        "fp": false_positive,
        "fn": false_negative,
    }


def pairs_from_view(view: dict) -> set[tuple[str, str]]:
    labels = view.get("labels", {})

    def label(iri: str) -> str:
        local = re.split(r"[#/]", iri.rstrip("#/"))[-1]
        return normalise_label(labels.get(iri) or local)

    return {
        (label(row["sub"]), label(row["super"]))
        for row in view.get("axioms", {}).get("subclass_of", [])
    }


def corpus_gold_mentions(run_dir: Path, gold_types: set[str]) -> set[str]:
    combined = "\n".join(path.read_text(encoding="utf-8") for path in sorted((run_dir / "corpus").glob("*.txt")))
    normalised = f" {normalise_label(combined)} "
    return {label for label in gold_types if f" {label} " in normalised}


def markdown_report(result: dict) -> str:
    projected = result["metrics"]["projected_taxonomy"]
    open_score = result["metrics"]["open_taxonomy"]
    coverage = result["metrics"]["class_coverage"]
    pipeline = result["pipeline"]
    terminology = result["terminology"]
    lines = [
        "# OntoLearner Wine × Public Wine Reviews Benchmark",
        "",
        "> This is a hybrid real-text evaluation, not the synthetic OntoLearner Text2Onto task. "
        "OntoLearner supplies held-out labels and taxonomy edges; public reviews supply the only extraction text.",
        "",
        "## Run",
        "",
        f"- Knowledge system: `{result['knowledge_system']['id']}` — {result['knowledge_system']['name']}",
        f"- Model: `{result['extraction']['model']}`",
        f"- Documents: {result['corpus']['documents']}",
        f"- Text characters: {result['corpus']['text_chars']:,}",
        f"- Chunks extracted: {result['corpus']['chunks_extracted']}",
        f"- Extraction time: {result['extraction']['elapsed_seconds']} seconds",
        f"- Successful chunks: {pipeline['successful_chunks']}/{pipeline['total_chunks']} ({pipeline['success_rate']:.2%})",
        "",
        "## Output",
        "",
        f"- Classes: {result['prediction']['classes']}",
        f"- Properties: {result['prediction']['properties']}",
        f"- Direct subclass edges: {result['prediction']['taxonomy_edges']}",
        f"- Gold types present in corpus: {result['corpus']['gold_types_mentioned']}/{result['gold']['types']}",
        f"- Controlled terms: {terminology['concepts']}",
        f"- Pending terminology proposals: {terminology['pending_proposals']}",
        f"- Open conflicts: {pipeline['open_conflicts']} ({', '.join(f'{key}={value}' for key, value in pipeline['conflict_types'].items()) or 'none'})",
        "",
        "## Metrics",
        "",
        "| Metric | Precision | Recall | F1 | TP | FP | FN |",
        "|---|---:|---:|---:|---:|---:|---:|",
        f"| OntoLearner-projected taxonomy | {projected['precision']:.4f} | {projected['recall']:.4f} | {projected['f1']:.4f} | {projected['tp']} | {projected['fp']} | {projected['fn']} |",
        f"| Open ontology taxonomy | {open_score['precision']:.4f} | {open_score['recall']:.4f} | {open_score['f1']:.4f} | {open_score['tp']} | {open_score['fp']} | {open_score['fn']} |",
        "",
        f"Class coverage: **{coverage['matched']}/{coverage['gold']} ({coverage['recall']:.2%})**.",
        "",
        "## Projected Errors",
        "",
        f"- True positives: {', '.join(' → '.join(pair) for pair in result['errors']['projected_true_positives']) or 'None'}",
        f"- False positives: {', '.join(' → '.join(pair) for pair in result['errors']['projected_false_positives']) or 'None'}",
        f"- False negatives: {', '.join(' → '.join(pair) for pair in result['errors']['projected_false_negatives']) or 'None'}",
        f"- Chunk errors: {', '.join(pipeline['chunk_errors']) or 'None'}",
        "",
        "## Protocol",
        "",
        "Gold labels were used only to reserve matching review records. Gold parent-child edges were never written into the corpus. "
        "The projected score filters OntoPilot edges to the 20 candidate types in the public OntoLearner task; the open score also counts every additional learned edge.",
        "",
    ]
    return "\n".join(lines)


def score_knowledge_system(
    client: OntoPilotClient,
    gold_dir: Path,
    run_dir: Path,
    ks_id: int,
) -> dict:
    state = load_state(run_dir)
    gold = load_gold(gold_dir)
    ks = client.get(f"/api/knowledge/{ks_id}")
    view = client.get(f"/api/knowledge/{ks_id}/ontology")
    predicted_pairs = pairs_from_view(view)
    projected_pairs = {
        pair for pair in predicted_pairs if pair[0] in gold["types"] and pair[1] in gold["types"]
    }
    predicted_classes = {normalise_label(row["label"]) for row in view.get("classes", [])}
    matched_classes = predicted_classes & gold["types"]
    mentions = corpus_gold_mentions(run_dir, gold["types"])
    projected_score = score_pairs(projected_pairs, gold["taxonomy"])
    open_score = score_pairs(predicted_pairs, gold["taxonomy"])

    job = state.get("job", {})
    chunk_errors = [line for line in job.get("log", "").splitlines() if "ERROR" in line]
    conflicts = client.get(f"/api/knowledge/{ks_id}/conflicts")
    open_conflicts = [row for row in conflicts if row.get("status") == "open"]
    vocabulary = client.get(f"/api/knowledge/{ks_id}/vocabulary")
    proposal_payload = client.get(f"/api/knowledge/{ks_id}/vocabulary/proposals")
    proposals = proposal_payload.get("items", []) if isinstance(proposal_payload, dict) else proposal_payload
    total_chunks = state.get("extracted_chunk_count", 0)
    successful_chunks = max(0, total_chunks - len(chunk_errors))
    result = {
        "benchmark": "OntoLearner Wine + public Wine Reviews real-text hybrid",
        "benchmark_kind": "hybrid_real_text_not_official_text2onto",
        "scored_at": now_iso(),
        "knowledge_system": {"id": ks["id"], "name": ks["name"], "graph_iri": ks["graph_iri"]},
        "gold": {"types": len(gold["types"]), "taxonomy_edges": len(gold["taxonomy"])},
        "corpus": {
            "documents": state.get("document_count", len(state.get("documents", []))),
            "text_chars": state.get("text_chars", 0),
            "chunks": state.get("chunk_count", 0),
            "chunks_extracted": state.get("extracted_chunk_count", 0),
            "gold_types_mentioned": len(mentions),
        },
        "extraction": {
            "job_id": state.get("job_id"),
            "model": job.get("model") or ks.get("llm_model") or "system default",
            "elapsed_seconds": state.get("extraction_elapsed_seconds"),
            "status": job.get("status", "unknown"),
        },
        "prediction": {
            "classes": len(view.get("classes", [])),
            "properties": len(view.get("object_properties", [])) + len(view.get("data_properties", [])),
            "taxonomy_edges": len(predicted_pairs),
            "projected_taxonomy_edges": len(projected_pairs),
        },
        "pipeline": {
            "total_chunks": total_chunks,
            "successful_chunks": successful_chunks,
            "failed_chunks": len(chunk_errors),
            "success_rate": successful_chunks / total_chunks if total_chunks else 0.0,
            "chunk_errors": chunk_errors,
            "open_conflicts": len(open_conflicts),
            "conflict_types": dict(sorted(Counter(row["ctype"] for row in open_conflicts).items())),
        },
        "terminology": {
            "schemes": len(vocabulary.get("schemes", [])),
            "concepts": len(vocabulary.get("concepts", [])),
            "proposals": len(proposals),
            "pending_proposals": sum(row.get("status") == "pending" for row in proposals),
            "proposal_actions": dict(sorted(Counter(row["action"] for row in proposals).items())),
        },
        "metrics": {
            "class_coverage": {
                "matched": len(matched_classes),
                "gold": len(gold["types"]),
                "recall": len(matched_classes) / len(gold["types"]) if gold["types"] else 0.0,
            },
            "projected_taxonomy": projected_score,
            "open_taxonomy": open_score,
        },
        "matched_gold_types": sorted(matched_classes),
        "missing_gold_types": sorted(gold["types"] - matched_classes),
        "corpus_gold_types": sorted(mentions),
        "errors": {
            "projected_true_positives": sorted(projected_pairs & gold["taxonomy"]),
            "projected_false_positives": sorted(projected_pairs - gold["taxonomy"]),
            "projected_false_negatives": sorted(gold["taxonomy"] - projected_pairs),
        },
    }
    write_json(run_dir / "result.json", result)
    (run_dir / "REPORT.md").write_text(markdown_report(result), encoding="utf-8")
    print(markdown_report(result), flush=True)
    print(f"[score] wrote {run_dir / 'result.json'} and {run_dir / 'REPORT.md'}", flush=True)
    return result


def make_client(args: argparse.Namespace) -> OntoPilotClient:
    return OntoPilotClient(
        args.base_url,
        args.username or os.getenv("ONTOPILOT_USERNAME", "admin"),
        args.password or os.getenv("ONTOPILOT_PASSWORD", "admin"),
    )


def command_prepare(args: argparse.Namespace) -> None:
    prepare_corpus(
        args.gold_dir,
        args.run_dir,
        args.source,
        args.pages,
        args.max_chars,
        args.reviews_parquet,
        args.reviews,
        args.reviews_per_document,
    )


def command_ingest(args: argparse.Namespace) -> None:
    client = make_client(args)
    try:
        ingest_corpus(client, args.run_dir, args.name, args.model)
    finally:
        client.close()


def command_extract(args: argparse.Namespace) -> None:
    client = make_client(args)
    try:
        extract_corpus(
            client, args.run_dir, args.ks_id, args.model, args.max_chunks, args.timeout, args.mode,
            None if args.resolution_mode == "default" else args.resolution_mode == "agentic",
        )
    finally:
        client.close()


def command_score(args: argparse.Namespace) -> None:
    client = make_client(args)
    try:
        score_knowledge_system(client, args.gold_dir, args.run_dir, args.ks_id)
    finally:
        client.close()


def command_run(args: argparse.Namespace) -> None:
    prepare_corpus(
        args.gold_dir,
        args.run_dir,
        args.source,
        args.pages,
        args.max_chars,
        args.reviews_parquet,
        args.reviews,
        args.reviews_per_document,
    )
    client = make_client(args)
    try:
        state = ingest_corpus(client, args.run_dir, args.name, args.model)
        extract_corpus(
            client, args.run_dir, state["ks_id"], args.model, args.max_chunks, args.timeout, args.mode,
            None if args.resolution_mode == "default" else args.resolution_mode == "agentic",
        )
        score_knowledge_system(client, args.gold_dir, args.run_dir, state["ks_id"])
    finally:
        client.close()


def command_selftest(_: argparse.Namespace) -> None:
    assert normalise_label("ItalianWine") == "italian wine"
    assert normalise_label("Rosé_Wine") == "rose wine"
    score = score_pairs({("port", "red wine"), ("chianti", "red wine")}, {("port", "red wine")})
    assert score == {"precision": 0.5, "recall": 1.0, "f1": 0.6667, "tp": 1, "fp": 1, "fn": 0}
    print("self-test OK")


def add_common_paths(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--gold-dir", type=Path, default=DEFAULT_GOLD_DIR)
    parser.add_argument("--run-dir", type=Path, default=DEFAULT_RUN_DIR)


def add_api_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL)
    parser.add_argument("--username")
    parser.add_argument("--password")


def add_prepare_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--source", choices=("wine-reviews", "wikipedia-api"), default="wine-reviews")
    parser.add_argument("--reviews-parquet", type=Path, default=DEFAULT_REVIEWS_PARQUET)
    parser.add_argument("--reviews", type=int, default=1200)
    parser.add_argument("--reviews-per-document", type=int, default=20)
    parser.add_argument("--pages", type=int, default=80)
    parser.add_argument("--max-chars", type=int, default=6000)


def add_ingest_args(parser: argparse.ArgumentParser, *, include_model: bool = True) -> None:
    parser.add_argument("--name", default="Benchmark · OntoLearner Wine · 1,200 Real Reviews")
    if include_model:
        parser.add_argument("--model")


def add_extract_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--model")
    parser.add_argument(
        "--mode", choices=("tbox", "abox", "both"), default="tbox",
        help=("tbox runs schema only; abox runs instances against the existing schema; "
              "both runs schema then instances over the same chunks"),
    )
    parser.add_argument(
        "--resolution-mode", choices=("default", "agentic", "fast"), default="default",
        help="agentic asks an LLM about ambiguous entity matches; fast uses embedding thresholds",
    )
    parser.add_argument("--max-chunks", type=int, default=0, help="0 extracts every parsed chunk")
    parser.add_argument("--timeout", type=int, default=7200)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare", help="download and freeze the real-text corpus")
    add_common_paths(prepare)
    add_prepare_args(prepare)
    prepare.set_defaults(func=command_prepare)

    ingest = subparsers.add_parser("ingest", help="create a KS, upload documents, and parse them")
    add_common_paths(ingest)
    add_api_args(ingest)
    add_ingest_args(ingest)
    ingest.set_defaults(func=command_ingest)

    extract = subparsers.add_parser("extract", help="run TBox extraction for the ingested corpus")
    add_common_paths(extract)
    add_api_args(extract)
    add_extract_args(extract)
    extract.add_argument("--ks-id", type=int, required=True)
    extract.set_defaults(func=command_extract)

    score = subparsers.add_parser("score", help="score an extracted KS against OntoLearner gold")
    add_common_paths(score)
    add_api_args(score)
    score.add_argument("--ks-id", type=int, required=True)
    score.set_defaults(func=command_score)

    run = subparsers.add_parser("run", help="prepare, ingest, extract, and score")
    add_common_paths(run)
    add_api_args(run)
    add_prepare_args(run)
    add_ingest_args(run, include_model=False)
    add_extract_args(run)
    run.set_defaults(func=command_run)

    selftest = subparsers.add_parser("selftest", help="check normalisation and metric arithmetic")
    selftest.set_defaults(func=command_selftest)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    args.func(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
