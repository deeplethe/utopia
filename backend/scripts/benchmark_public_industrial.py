"""Run reproducible cross-domain ontology-learning experiments on public industrial corpora.

The prose corpus and the official ontology are deliberately kept separate.  OntoPilot only
receives the files under ``prepared/<dataset>/corpus``; RDF files under ``gold`` are read by this
script after extraction and are never uploaded to a knowledge system.

Datasets:
* W3C/OGC SOSA/SSN -- sensors, observations, sampling and actuation.
* ETSI SAREF4INMA -- industry and manufacturing.
* Brick -- buildings and building-management systems.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict, deque
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from datetime import UTC, datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import threading
import time
import unicodedata
from urllib.parse import urldefrag, urljoin, urlparse

from bs4 import BeautifulSoup
import httpx
from rdflib import Graph, Literal, RDF, RDFS, OWL, SKOS, URIRef


BACKEND_DIR = Path(__file__).resolve().parents[1]
BENCHMARK_DIR = BACKEND_DIR / "data" / "benchmarks" / "public_industrial"
SOURCES_DIR = BENCHMARK_DIR / "sources"
PREPARED_DIR = BENCHMARK_DIR / "prepared"
RUNS_DIR = BENCHMARK_DIR / "runs"
PRINT_LOCK = threading.Lock()


@dataclass(frozen=True)
class DatasetSpec:
    slug: str
    name: str
    domain: str
    source_url: str
    ontology_url: str


DATASETS = {
    "ssn-sosa": DatasetSpec(
        slug="ssn-sosa",
        name="W3C-OGC SOSA/SSN",
        domain="sensors, observations, sampling, actuation, and industrial IoT",
        source_url="https://www.w3.org/TR/vocab-ssn-2023/",
        ontology_url="https://github.com/w3c/sdw-sosa-ssn",
    ),
    "saref4inma": DatasetSpec(
        slug="saref4inma",
        name="ETSI SAREF4INMA",
        domain="industry and manufacturing",
        source_url="https://saref.etsi.org/saref4inma/",
        ontology_url="https://labs.etsi.org/rep/saref/saref4inma",
    ),
    "brick": DatasetSpec(
        slug="brick",
        name="Brick Schema",
        domain="buildings and building-management systems",
        source_url="https://docs.brickschema.org/brick/overview.html",
        ontology_url="https://brickschema.org/schema/Brick.ttl",
    ),
}


SSN_CHAPTERS = (
    "Introduction.html",
    "Overview.html",
    "Common.html",
    "Observation.html",
    "Actuation.html",
    "Sampling.html",
    "System-capabilities.html",
    "ModelFOI.html",
    "ModelPropertyDefinition.html",
    "ModelSystemType.html",
    "ModelSystemInstance.html",
    "ModelComplexSystem.html",
    "ModelLocation.html",
    "ModelTimes.html",
    "ModelTimeSeries.html",
)

SAREF_DOCUMENTS = (
    "abstract.md",
    "scope.md",
    "description.md",
    "examples.md",
    "annexes.md",
)

BRICK_ALLOWED_PREFIXES = ("/brick/", "/modeling/", "/software/")
BRICK_SEEDS = (
    "https://docs.brickschema.org/brick/overview.html",
    "https://docs.brickschema.org/brick/concepts.html",
    "https://docs.brickschema.org/brick/relationships.html",
    "https://docs.brickschema.org/modeling/conventions.html",
    "https://docs.brickschema.org/modeling/collections.html",
    "https://docs.brickschema.org/modeling/timeseries.html",
)


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


def log(dataset: str, message: str) -> None:
    with PRINT_LOCK:
        print(f"[{dataset}] {message}", flush=True)


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def repo_commit(path: Path) -> str | None:
    head = path / ".git" / "HEAD"
    if not head.exists():
        return None
    value = head.read_text(encoding="utf-8").strip()
    if value.startswith("ref: "):
        ref = path / ".git" / value[5:]
        if ref.exists():
            return ref.read_text(encoding="utf-8").strip()
        packed = path / ".git" / "packed-refs"
        if packed.exists():
            suffix = value[5:]
            for line in packed.read_text(encoding="utf-8").splitlines():
                if line and not line.startswith(("#", "^")) and line.endswith(f" {suffix}"):
                    return line.split(" ", 1)[0]
    return value if re.fullmatch(r"[0-9a-f]{40}", value) else None


def clean_line(value: str) -> str:
    return re.sub(r"\s+", " ", value.replace("\u200b", " ")).strip()


def html_to_markdown(html: str, fallback_title: str) -> tuple[str, str]:
    soup = BeautifulSoup(html, "html.parser")
    root = (
        soup.select_one("main")
        or soup.select_one('[role="main"]')
        or soup.select_one("article")
        or soup.select_one(".document")
        or soup.body
        or soup
    )
    for element in root.select(
        "script, style, nav, footer, header, aside, svg, pre, .toctree-wrapper, "
        ".related, .sphinxsidebar, .wy-nav-side, .rst-footer-buttons"
    ):
        element.decompose()

    title_node = root.find(["h1", "h2"]) or soup.title
    title = clean_line(title_node.get_text(" ", strip=True)) if title_node else fallback_title
    lines: list[str] = []
    seen: set[str] = set()
    boilerplate = {
        "edit on github",
        "table of contents",
        "previous",
        "next",
        "contents",
    }
    for node in root.find_all(["h1", "h2", "h3", "h4", "p", "li", "dt", "dd"]):
        text = clean_line(node.get_text(" ", strip=True))
        if not text or text.casefold() in boilerplate:
            continue
        key = text.casefold()
        if key in seen:
            continue
        seen.add(key)
        if node.name and node.name.startswith("h"):
            line = f"{'#' * min(int(node.name[1]), 4)} {text}"
        elif node.name == "li":
            line = f"- {text}"
        else:
            line = text
        lines.append(line)
    body = "\n\n".join(lines).strip()
    return title, f"# {title}\n\n{body}\n" if body else f"# {title}\n"


def truncate_markdown(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    boundary = text.rfind("\n\n", 0, limit)
    if boundary < max(1000, limit // 2):
        boundary = limit
    return text[:boundary].rstrip() + "\n"


def reset_prepared_dir(slug: str) -> tuple[Path, Path]:
    target = PREPARED_DIR / slug
    if target.exists():
        shutil.rmtree(target)
    corpus = target / "corpus"
    gold = target / "gold"
    corpus.mkdir(parents=True)
    gold.mkdir(parents=True)
    return corpus, gold


def write_corpus_documents(
    corpus_dir: Path,
    documents: list[tuple[str, str, str]],
    max_chars: int,
) -> list[dict]:
    records: list[dict] = []
    remaining = max_chars
    for filename, source, text in documents:
        if remaining < 1200:
            break
        content = truncate_markdown(text, remaining)
        if len(content) < 500:
            continue
        path = corpus_dir / filename
        path.write_text(content, encoding="utf-8")
        records.append(
            {
                "filename": filename,
                "source": source,
                "chars": len(content),
                "sha256": sha256(path),
            }
        )
        remaining -= len(content)
    return records


def prepare_ssn(max_chars: int) -> dict:
    spec = DATASETS["ssn-sosa"]
    repo = SOURCES_DIR / "ssn"
    chapters = repo / "ssn" / "chapters"
    gold_source = repo / "ssn" / "rdf" / "ontology" / "core"
    corpus_dir, gold_dir = reset_prepared_dir(spec.slug)
    documents: list[tuple[str, str, str]] = []
    for source_name in SSN_CHAPTERS:
        path = chapters / source_name
        if not path.exists():
            raise FileNotFoundError(path)
        title, text = html_to_markdown(path.read_text(encoding="utf-8"), path.stem)
        filename = f"{len(documents) + 1:02d}-{re.sub(r'[^a-z0-9]+', '-', path.stem.casefold()).strip('-')}.md"
        documents.append((filename, f"{spec.source_url}#{path.stem}", text))
    corpus_records = write_corpus_documents(corpus_dir, documents, max_chars)

    gold_records = []
    for source in sorted(gold_source.glob("*.ttl")):
        if "deprecated" in source.name:
            continue
        target = gold_dir / source.name
        shutil.copy2(source, target)
        gold_records.append({"filename": target.name, "sha256": sha256(target)})
    return make_manifest(spec, corpus_records, gold_records, repo_commit(repo))


def prepare_saref(max_chars: int) -> dict:
    spec = DATASETS["saref4inma"]
    repo = SOURCES_DIR / "saref4inma"
    docs = repo / "documentation"
    corpus_dir, gold_dir = reset_prepared_dir(spec.slug)
    documents: list[tuple[str, str, str]] = []
    for source_name in SAREF_DOCUMENTS:
        path = docs / source_name
        if not path.exists():
            raise FileNotFoundError(path)
        text = path.read_text(encoding="utf-8").strip() + "\n"
        documents.append((source_name, f"{spec.source_url}{source_name}", text))
    corpus_records = write_corpus_documents(corpus_dir, documents, max_chars)

    source = repo / "ontology" / "saref4inma.ttl"
    target = gold_dir / source.name
    shutil.copy2(source, target)
    gold_records = [{"filename": target.name, "sha256": sha256(target)}]
    return make_manifest(spec, corpus_records, gold_records, repo_commit(repo))


def canonical_brick_url(value: str) -> str | None:
    value = urldefrag(value)[0]
    parsed = urlparse(value)
    if parsed.scheme not in {"http", "https"} or parsed.netloc != "docs.brickschema.org":
        return None
    if not parsed.path.endswith(".html") or not parsed.path.startswith(BRICK_ALLOWED_PREFIXES):
        return None
    if any(part in parsed.path for part in ("genindex", "search", "py-modindex")):
        return None
    return f"https://docs.brickschema.org{parsed.path}"


def crawl_brick(max_chars: int, max_pages: int = 24) -> list[tuple[str, str, str]]:
    queue = deque(BRICK_SEEDS)
    queued = set(BRICK_SEEDS)
    visited: set[str] = set()
    documents: list[tuple[str, str, str]] = []
    total_chars = 0
    headers = {"User-Agent": "OntoPilot public benchmark preparation/1.0"}
    with httpx.Client(timeout=45, follow_redirects=True, trust_env=False, headers=headers) as client:
        while queue and len(documents) < max_pages and total_chars < max_chars:
            url = queue.popleft()
            if url in visited:
                continue
            visited.add(url)
            response = client.get(url)
            if response.status_code != 200 or "html" not in response.headers.get("content-type", ""):
                continue
            title, text = html_to_markdown(response.text, Path(urlparse(url).path).stem)
            if len(text) >= 500:
                stem = re.sub(r"[^a-z0-9]+", "-", title.casefold()).strip("-")[:70]
                filename = f"{len(documents) + 1:02d}-{stem or 'brick-doc'}.md"
                documents.append((filename, url, text))
                total_chars += len(text)
            soup = BeautifulSoup(response.text, "html.parser")
            links = set()
            for anchor in soup.select("a[href]"):
                candidate = canonical_brick_url(urljoin(url, anchor.get("href", "")))
                if candidate and candidate not in visited and candidate not in queued:
                    links.add(candidate)
            for candidate in sorted(links):
                queue.append(candidate)
                queued.add(candidate)
    return documents


def prepare_brick(max_chars: int) -> dict:
    spec = DATASETS["brick"]
    repo = SOURCES_DIR / "brick"
    corpus_dir, gold_dir = reset_prepared_dir(spec.slug)
    documents = crawl_brick(max_chars)
    if not documents:
        raise RuntimeError("No Brick documentation pages could be downloaded")
    corpus_records = write_corpus_documents(corpus_dir, documents, max_chars)

    target = gold_dir / "Brick.ttl"
    headers = {"User-Agent": "OntoPilot public benchmark preparation/1.0"}
    with httpx.Client(timeout=90, follow_redirects=True, trust_env=False, headers=headers) as client:
        response = client.get(spec.ontology_url)
        response.raise_for_status()
        target.write_bytes(response.content)
    gold_records = [{"filename": target.name, "sha256": sha256(target)}]
    return make_manifest(spec, corpus_records, gold_records, repo_commit(repo))


def make_manifest(
    spec: DatasetSpec,
    corpus_records: list[dict],
    gold_records: list[dict],
    commit: str | None,
) -> dict:
    return {
        "benchmark": "OntoPilot Public Industrial Multi-domain Benchmark",
        "dataset": spec.slug,
        "name": spec.name,
        "domain": spec.domain,
        "prepared_at": now_iso(),
        "source_url": spec.source_url,
        "ontology_url": spec.ontology_url,
        "source_commit": commit,
        "protocol": {
            "input": "Only prose files listed in documents are uploaded to OntoPilot.",
            "held_out": "Official RDF files listed in gold are used only for offline scoring.",
            "evaluation": "Exact normalized labels, conditioned on terms mentioned in the frozen corpus.",
        },
        "documents": corpus_records,
        "gold": gold_records,
        "stats": {
            "documents": len(corpus_records),
            "text_chars": sum(row["chars"] for row in corpus_records),
            "gold_files": len(gold_records),
        },
    }


def prepare_all(dataset_slugs: list[str], max_chars: int) -> None:
    builders = {
        "ssn-sosa": prepare_ssn,
        "saref4inma": prepare_saref,
        "brick": prepare_brick,
    }
    for slug in dataset_slugs:
        manifest = builders[slug](max_chars)
        path = PREPARED_DIR / slug / "manifest.json"
        write_json(path, manifest)
        log(slug, f"prepared {manifest['stats']['documents']} documents / {manifest['stats']['text_chars']} chars")


class OntoPilotClient:
    def __init__(self, base_url: str, username: str, password: str) -> None:
        self.client = httpx.Client(
            base_url=base_url.rstrip("/"), timeout=180, follow_redirects=True, trust_env=False
        )
        response = self.client.post("/api/auth/login", json={"username": username, "password": password})
        response.raise_for_status()

    def close(self) -> None:
        self.client.close()

    def get(self, path: str, **kwargs) -> object:
        response = self.client.get(path, **kwargs)
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


def ingest_dataset(
    client: OntoPilotClient,
    slug: str,
    run_name: str,
    run_dir: Path,
    model: str | None,
) -> dict:
    state = load_state(run_dir)
    prepared = PREPARED_DIR / slug
    manifest = read_json(prepared / "manifest.json")
    if state.get("ks_id"):
        # A killed benchmark may have persisted the KS and only part of the document list.
        # Continue from the manifest instead of treating any ks_id as a completed ingestion.
        ks = client.get(f"/api/knowledge/{state['ks_id']}")
        state.setdefault("documents", [])
        log(slug, f"resuming knowledge system {ks['id']} with {len(state['documents'])} document(s)")
    else:
        body: dict[str, object] = {
            "name": f"[Bench {run_name}] {manifest['name']}",
            "description": (
                f"Public cross-domain capability test for {manifest['domain']}. "
                "Official ontology is held out and used only for offline evaluation."
            ),
        }
        if model:
            body["llm_model"] = model
        ks = client.post("/api/knowledge", json=body)
        state = {
            "dataset": slug,
            "round": run_name,
            "ks_id": ks["id"],
            "ks_public_id": ks["public_id"],
            "ks_name": ks["name"],
            "created_at": now_iso(),
            "documents": [],
        }
        save_state(run_dir, state)
        log(slug, f"created knowledge system {ks['id']}")

    completed_filenames = {row["filename"] for row in state["documents"]}
    for index, document_meta in enumerate(manifest["documents"], start=1):
        path = prepared / "corpus" / document_meta["filename"]
        if path.name in completed_filenames:
            continue
        with path.open("rb") as handle:
            document = client.post(
                f"/api/knowledge/{ks['id']}/documents/upload",
                files={"file": (path.name, handle, "text/markdown; charset=utf-8")},
                data={"folder": f"/benchmark/{slug}"},
            )
        parsed = client.post(f"/api/knowledge/{ks['id']}/documents/{document['id']}/parse")
        if parsed.get("parse_status") != "parsed":
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
        log(slug, f"parsed {index}/{len(manifest['documents'])}: {path.name} ({parsed['chunk_count']} chunks)")
    state["document_count"] = len(state["documents"])
    state["text_chars"] = sum(row["chars"] for row in state["documents"])
    state["chunk_count"] = sum(row["chunks"] for row in state["documents"])
    state["ingested_at"] = now_iso()
    save_state(run_dir, state)
    return state


def collect_chunk_ids(client: OntoPilotClient, ks_id: int, state: dict) -> list[int]:
    chunk_ids: list[int] = []
    for document in state.get("documents", []):
        chunks = client.get(f"/api/knowledge/{ks_id}/documents/{document['document_id']}/chunks")
        chunk_ids.extend(chunk["id"] for chunk in chunks)
    return chunk_ids


def wait_for_job(
    client: OntoPilotClient,
    slug: str,
    run_dir: Path,
    state: dict,
    job: dict,
    timeout_seconds: int,
) -> dict:
    started = time.monotonic()
    last_progress: tuple[int, str] | None = None
    while time.monotonic() - started < timeout_seconds:
        current = client.get(f"/api/knowledge/{state['ks_id']}/jobs/{job['id']}")
        progress = (current.get("processed_chunks", 0), current.get("phase", ""))
        if progress != last_progress:
            log(slug, f"job {job['id']}: {progress[0]}/{current['total_chunks']} phase={progress[1] or current['status']}")
            last_progress = progress
        if current["status"] in {"completed", "failed"}:
            state["job"] = current
            state["extraction_elapsed_seconds"] = round(time.monotonic() - started, 2)
            state["extraction_finished_at"] = now_iso()
            save_state(run_dir, state)
            if current["status"] != "completed":
                raise RuntimeError(f"Extraction job {job['id']} failed: {current.get('error')}")
            return current
        time.sleep(4)
    raise TimeoutError(f"Extraction job {job['id']} exceeded {timeout_seconds} seconds")


def extract_dataset(
    client: OntoPilotClient,
    slug: str,
    run_dir: Path,
    state: dict,
    mode: str,
    model: str | None,
    max_chunks: int,
    timeout_seconds: int,
    agentic_resolution: bool | None,
) -> dict:
    existing_job = state.get("job")
    if existing_job and existing_job.get("status") == "completed":
        return existing_job
    if state.get("job_id"):
        current = client.get(f"/api/knowledge/{state['ks_id']}/jobs/{state['job_id']}")
        if current["status"] in {"pending", "running"}:
            return wait_for_job(client, slug, run_dir, state, current, timeout_seconds)
        if current["status"] == "completed":
            state["job"] = current
            save_state(run_dir, state)
            return current

    chunk_ids = collect_chunk_ids(client, state["ks_id"], state)
    if max_chunks:
        chunk_ids = chunk_ids[:max_chunks]
    payload: dict[str, object] = {"chunk_ids": chunk_ids}
    if model:
        payload["model"] = model
    if agentic_resolution is not None:
        payload["agentic_resolution"] = agentic_resolution
    endpoint = {"tbox": "extract", "both": "extract-all", "abox": "extract-instances"}[mode]
    job = client.post(f"/api/knowledge/{state['ks_id']}/{endpoint}", json=payload)
    state["job_id"] = job["id"]
    state["extraction_mode"] = mode
    state["extracted_chunk_count"] = len(chunk_ids)
    state["extraction_started_at"] = now_iso()
    save_state(run_dir, state)
    log(slug, f"started {mode} job {job['id']} for {len(chunk_ids)} chunks")
    return wait_for_job(client, slug, run_dir, state, job, timeout_seconds)


def normalise_label(value: str) -> str:
    value = unicodedata.normalize("NFKC", value or "")
    value = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", value)
    value = value.replace("_", " ").replace("-", " ")
    value = "".join(character if character.isalnum() else " " for character in value.casefold())
    return re.sub(r"\s+", " ", value).strip()


def normalise_role_label(value: str) -> str:
    """Normalize a label without discarding an explicit QName/instance prefix.

    ``:hvac_system`` is a named RDF resource while ``HVAC_System`` is a class label.  General
    coverage matching may ignore punctuation, but a TBox/ABox role-collision check must retain
    that identity marker or it reports a false positive.
    """
    stripped = (value or "").strip()
    prefix_match = re.match(r"^(?P<prefix>(?:[A-Za-z][\w-]*)?:)", stripped)
    prefix = prefix_match.group("prefix").casefold() if prefix_match else ""
    body = stripped[prefix_match.end():] if prefix_match else stripped
    return prefix + normalise_label(body)


def local_name(iri: str) -> str:
    return re.split(r"[#/]", iri.rstrip("#/"))[-1]


def corpus_text(slug: str) -> str:
    corpus_dir = PREPARED_DIR / slug / "corpus"
    return "\n".join(path.read_text(encoding="utf-8") for path in sorted(corpus_dir.glob("*.md")))


def surface_mentioned(normalized_corpus: str, label: str) -> bool:
    normalized = normalise_label(label)
    if len(normalized) < 2:
        return False
    return re.search(rf"(?<![\w]){re.escape(normalized)}(?![\w])", normalized_corpus) is not None


def load_gold(slug: str) -> dict:
    graph = Graph()
    for path in sorted((PREPARED_DIR / slug / "gold").glob("*.ttl")):
        graph.parse(path, format="turtle")

    kinds: dict[str, str] = {}
    for rdf_type, kind in (
        (OWL.Class, "class"),
        (RDFS.Class, "class"),
        (OWL.ObjectProperty, "object_property"),
        (OWL.DatatypeProperty, "data_property"),
    ):
        for subject in graph.subjects(RDF.type, rdf_type):
            if isinstance(subject, URIRef):
                kinds[str(subject)] = kind

    labels: dict[str, str] = {}
    for iri in kinds:
        candidates: list[Literal] = []
        for predicate in (RDFS.label, SKOS.prefLabel):
            candidates.extend(
                value for value in graph.objects(URIRef(iri), predicate) if isinstance(value, Literal)
            )
        preferred = next((value for value in candidates if value.language == "en"), None)
        preferred = preferred or next((value for value in candidates if value.language is None), None)
        labels[iri] = str(preferred) if preferred is not None else local_name(iri)

    aliases: dict[str, set[str]] = {}
    for iri, label in labels.items():
        aliases[iri] = {normalise_label(label), normalise_label(local_name(iri))}
    text = normalise_label(corpus_text(slug))
    mentioned = {
        iri
        for iri, values in aliases.items()
        if any(value and surface_mentioned(text, value) for value in values)
    }
    taxonomy = {
        (str(subject), str(parent))
        for subject, parent in graph.subject_objects(RDFS.subClassOf)
        if isinstance(subject, URIRef)
        and isinstance(parent, URIRef)
        and kinds.get(str(subject)) == "class"
        and kinds.get(str(parent)) == "class"
    }
    canonical = {
        iri: normalise_label(labels[iri]) or normalise_label(local_name(iri))
        for iri in kinds
    }
    concepts: dict[tuple[str, str], dict] = {}
    for iri, kind in kinds.items():
        key = (kind, canonical[iri])
        concept = concepts.setdefault(
            key,
            {"kind": kind, "canonical": canonical[iri], "label": labels[iri], "aliases": set()},
        )
        concept["aliases"].update(aliases[iri])
    mentioned_concepts = {
        (kinds[iri], canonical[iri])
        for iri in mentioned
    }
    taxonomy_concepts = {
        (canonical[child], canonical[parent])
        for child, parent in taxonomy
    }
    return {
        "kinds": kinds,
        "labels": labels,
        "aliases": aliases,
        "mentioned": mentioned,
        "taxonomy": taxonomy,
        "canonical": canonical,
        "concepts": concepts,
        "mentioned_concepts": mentioned_concepts,
        "taxonomy_concepts": taxonomy_concepts,
    }


def score_set(predicted: set, gold: set) -> dict:
    true_positive = len(predicted & gold)
    precision = true_positive / len(predicted) if predicted else 0.0
    recall = true_positive / len(gold) if gold else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return {
        "precision": round(precision, 4),
        "recall": round(recall, 4),
        "f1": round(f1, 4),
        "tp": true_positive,
        "fp": len(predicted - gold),
        "fn": len(gold - predicted),
    }


def connected_components(nodes: set[str], edges: set[tuple[str, str]]) -> int:
    adjacency: dict[str, set[str]] = defaultdict(set)
    for left, right in edges:
        adjacency[left].add(right)
        adjacency[right].add(left)
    remaining = set(nodes)
    components = 0
    while remaining:
        components += 1
        queue = [remaining.pop()]
        while queue:
            for neighbour in adjacency.get(queue.pop(), set()) & remaining:
                remaining.remove(neighbour)
                queue.append(neighbour)
    return components


def fetch_all_individuals(client: OntoPilotClient, ks_id: int) -> list[dict]:
    items: list[dict] = []
    offset = 0
    while True:
        payload = client.get(f"/api/knowledge/{ks_id}/abox/individuals", params={"limit": 200, "offset": offset})
        items.extend(payload.get("items", []))
        if len(items) >= payload.get("total", 0) or not payload.get("items"):
            return items
        offset += len(payload["items"])


def entity_coverage(view_rows: list[dict], gold: dict, kind: str) -> dict:
    predicted_labels = {normalise_label(row.get("label") or local_name(row["iri"])) for row in view_rows}
    relevant = {key for key in gold["mentioned_concepts"] if key[0] == kind}
    matched = {
        key
        for key in relevant
        if any(alias in predicted_labels for alias in gold["concepts"][key]["aliases"])
    }
    return {
        "matched": len(matched),
        "mentioned_gold": len(relevant),
        "recall": round(len(matched) / len(relevant), 4) if relevant else 0.0,
        "matched_labels": sorted(gold["concepts"][key]["label"] for key in matched),
        "missing_labels": sorted(gold["concepts"][key]["label"] for key in relevant - matched),
    }


def taxonomy_score(view: dict, gold: dict) -> dict:
    predicted_labels = {
        row["iri"]: normalise_label(row.get("label") or local_name(row["iri"]))
        for row in view.get("classes", [])
    }
    gold_by_alias: dict[str, set[str]] = defaultdict(set)
    mentioned_class_concepts = {
        canonical for kind, canonical in gold["mentioned_concepts"] if kind == "class"
    }
    for key, concept in gold["concepts"].items():
        if key[0] == "class" and key[1] in mentioned_class_concepts:
            for alias in concept["aliases"]:
                gold_by_alias[alias].add(key[1])

    projected: set[tuple[str, str]] = set()
    raw_edges: set[tuple[str, str]] = set()
    for edge in view.get("axioms", {}).get("subclass_of", []):
        child, parent = edge.get("sub"), edge.get("super")
        if child not in predicted_labels or parent not in predicted_labels:
            continue
        raw_edges.add((child, parent))
        for gold_child in gold_by_alias.get(predicted_labels[child], set()):
            for gold_parent in gold_by_alias.get(predicted_labels[parent], set()):
                projected.add((gold_child, gold_parent))
    mentioned_gold = {
        edge for edge in gold["taxonomy_concepts"]
        if edge[0] in mentioned_class_concepts and edge[1] in mentioned_class_concepts
    }
    class_display = {
        key[1]: concept["label"]
        for key, concept in gold["concepts"].items()
        if key[0] == "class"
    }
    return {
        "metrics": score_set(projected, mentioned_gold),
        "predicted_projected_edges": len(projected),
        "predicted_raw_edges": len(raw_edges),
        "mentioned_gold_edges": len(mentioned_gold),
        "false_positive_labels": sorted(
            (class_display[child], class_display[parent])
            for child, parent in projected - mentioned_gold
        ),
        "false_negative_labels": sorted(
            (class_display[child], class_display[parent])
            for child, parent in mentioned_gold - projected
        ),
    }


def structural_metrics(view: dict, individuals: list[dict]) -> dict:
    class_rows = view.get("classes", [])
    class_iris = {row["iri"] for row in class_rows}
    edges = {
        (edge["sub"], edge["super"])
        for edge in view.get("axioms", {}).get("subclass_of", [])
        if edge.get("sub") in class_iris and edge.get("super") in class_iris
    }
    degree = Counter(value for edge in edges for value in edge)
    parents = {child for child, _ in edges}
    labels = [normalise_label(row.get("label") or local_name(row["iri"])) for row in class_rows]
    individual_labels = {normalise_role_label(row.get("label", "")) for row in individuals}
    duplicate_labels = sorted(label for label, count in Counter(labels).items() if label and count > 1)
    role_collisions = sorted(set(labels) & individual_labels - {""})
    xsd_classes = [
        row.get("label") or local_name(row["iri"])
        for row in class_rows
        if row["iri"].startswith("http://www.w3.org/2001/XMLSchema#")
        or normalise_label(row.get("label", "")).startswith("xsd ")
    ]
    return {
        "classes": len(class_iris),
        "taxonomy_edges": len(edges),
        "taxonomy_components": connected_components(class_iris, edges) if class_iris else 0,
        "taxonomy_isolated_classes": sum(iri not in degree for iri in class_iris),
        "taxonomy_isolated_ratio": round(sum(iri not in degree for iri in class_iris) / len(class_iris), 4)
        if class_iris
        else 0.0,
        "root_classes": len(class_iris - parents),
        "duplicate_class_labels": duplicate_labels,
        "xsd_declared_as_classes": sorted(xsd_classes),
        "tbox_abox_label_collisions": role_collisions,
    }


def safe_get(client: OntoPilotClient, path: str, default: object) -> object:
    try:
        return client.get(path)
    except httpx.HTTPError:
        return default


def score_dataset(client: OntoPilotClient, slug: str, run_dir: Path) -> dict:
    state = load_state(run_dir)
    ks_id = state["ks_id"]
    ks = client.get(f"/api/knowledge/{ks_id}")
    view = client.get(f"/api/knowledge/{ks_id}/ontology")
    gold = load_gold(slug)
    individuals = fetch_all_individuals(client, ks_id)
    conflicts = safe_get(client, f"/api/knowledge/{ks_id}/conflicts?status=all", [])
    resolution = safe_get(client, f"/api/knowledge/{ks_id}/resolution/queue?limit=1000", {"items": [], "total": 0})
    terminology = safe_get(
        client, f"/api/knowledge/{ks_id}/vocabulary/proposals?status=all&limit=1000", {"items": [], "total": 0}
    )
    validation = safe_get(client, f"/api/knowledge/{ks_id}/abox/validate", {"items": [], "counts": {}})
    job = state.get("job", {})
    errors = [
        line for line in job.get("log", "").splitlines()
        if "ERROR" in line or "PARTIAL (" in line
    ]
    result = {
        "benchmark": "OntoPilot Public Industrial Multi-domain Benchmark",
        "dataset": slug,
        "round": state.get("round"),
        "scored_at": now_iso(),
        "knowledge_system": {"id": ks_id, "name": ks["name"], "public_id": ks["public_id"]},
        "corpus": {
            "documents": state.get("document_count", 0),
            "chars": state.get("text_chars", 0),
            "chunks": state.get("chunk_count", 0),
            "extracted_chunks": state.get("extracted_chunk_count", 0),
        },
        "extraction": {
            "status": job.get("status"),
            "model": job.get("model") or ks.get("llm_model") or "system default",
            "elapsed_seconds": state.get("extraction_elapsed_seconds"),
            "classes_added": job.get("classes_added", 0),
            "properties_added": job.get("properties_added", 0),
            "axioms_added": job.get("axioms_added", 0),
            "individuals_added": job.get("individuals_added", 0),
            "assertions_added": job.get("assertions_added", 0),
            "chunk_errors": errors,
        },
        "gold": {
            "classes": sum(key[0] == "class" for key in gold["concepts"]),
            "object_properties": sum(key[0] == "object_property" for key in gold["concepts"]),
            "data_properties": sum(key[0] == "data_property" for key in gold["concepts"]),
            "mentioned_entities": len(gold["mentioned_concepts"]),
            "taxonomy_edges": len(gold["taxonomy_concepts"]),
            "raw_iris": len(gold["kinds"]),
        },
        "coverage": {
            "classes": entity_coverage(view.get("classes", []), gold, "class"),
            "object_properties": entity_coverage(view.get("object_properties", []), gold, "object_property"),
            "data_properties": entity_coverage(view.get("data_properties", []), gold, "data_property"),
        },
        "taxonomy": taxonomy_score(view, gold),
        "structure": structural_metrics(view, individuals),
        "abox": {
            "individuals": len(individuals),
            "untyped_individuals": sum(not row.get("types") for row in individuals),
            "pending_resolution": resolution.get("total", len(resolution.get("items", []))),
        },
        "review": {
            "conflicts": len(conflicts),
            "open_conflicts": sum(row.get("status") == "open" for row in conflicts),
            "conflict_types": dict(sorted(Counter(row.get("ctype", "unknown") for row in conflicts).items())),
            "terminology_proposals": terminology.get("total", len(terminology.get("items", []))),
            "pending_terminology": sum(row.get("status") == "pending" for row in terminology.get("items", [])),
            "validation_counts": validation.get("counts", {}),
        },
    }
    write_json(run_dir / "result.json", result)
    (run_dir / "REPORT.md").write_text(dataset_report(result), encoding="utf-8")
    log(slug, f"scored: class recall={result['coverage']['classes']['recall']:.3f}, taxonomy F1={result['taxonomy']['metrics']['f1']:.3f}")
    return result


def dataset_report(result: dict) -> str:
    coverage = result["coverage"]
    taxonomy = result["taxonomy"]["metrics"]
    structure = result["structure"]
    return "\n".join(
        [
            f"# {result['knowledge_system']['name']}",
            "",
            "## Result",
            "",
            "| Metric | Value |",
            "| --- | ---: |",
            f"| Mention-conditioned class recall | {coverage['classes']['recall']:.4f} |",
            f"| Mention-conditioned object-property recall | {coverage['object_properties']['recall']:.4f} |",
            f"| Mention-conditioned data-property recall | {coverage['data_properties']['recall']:.4f} |",
            f"| Direct taxonomy precision | {taxonomy['precision']:.4f} |",
            f"| Direct taxonomy recall | {taxonomy['recall']:.4f} |",
            f"| Direct taxonomy F1 | {taxonomy['f1']:.4f} |",
            f"| Taxonomy-isolated classes | {structure['taxonomy_isolated_classes']} / {structure['classes']} |",
            f"| TBox/ABox label collisions | {len(structure['tbox_abox_label_collisions'])} |",
            f"| Individuals | {result['abox']['individuals']} |",
            f"| Open conflicts | {result['review']['open_conflicts']} |",
            "",
            "## Protocol",
            "",
            "Only frozen prose files were uploaded. Official RDF remained offline. Coverage and taxonomy "
            "scores use exact normalized labels and only gold entities explicitly mentioned in the prose.",
            "",
        ]
    )


def round_summary(round_name: str, results: list[dict]) -> str:
    lines = [
        f"# Public Industrial Benchmark — {round_name}",
        "",
        "| Dataset | Docs | Chunks | Class recall | Property recall | Taxonomy F1 | Isolated | Individuals | Errors |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for result in sorted(results, key=lambda row: row["dataset"]):
        properties = result["coverage"]["object_properties"]
        data_properties = result["coverage"]["data_properties"]
        matched = properties["matched"] + data_properties["matched"]
        total = properties["mentioned_gold"] + data_properties["mentioned_gold"]
        property_recall = matched / total if total else 0.0
        lines.append(
            f"| {result['dataset']} | {result['corpus']['documents']} | {result['corpus']['chunks']} | "
            f"{result['coverage']['classes']['recall']:.4f} | {property_recall:.4f} | "
            f"{result['taxonomy']['metrics']['f1']:.4f} | "
            f"{result['structure']['taxonomy_isolated_classes']}/{result['structure']['classes']} | "
            f"{result['abox']['individuals']} | {len(result['extraction']['chunk_errors'])} |"
        )
    lines.extend(
        [
            "",
            "The official ontologies are held out. These are framework-level diagnostics, not official "
            "leaderboard scores and not a claim that every additional extracted concept is incorrect.",
            "",
        ]
    )
    return "\n".join(lines)


def run_one_dataset(
    slug: str,
    round_name: str,
    base_url: str,
    username: str,
    password: str,
    model: str | None,
    mode: str,
    max_chunks: int,
    timeout_seconds: int,
    agentic_resolution: bool | None,
) -> dict:
    run_dir = RUNS_DIR / round_name / slug
    run_dir.mkdir(parents=True, exist_ok=True)
    client = OntoPilotClient(base_url, username, password)
    try:
        state = ingest_dataset(client, slug, round_name, run_dir, model)
        job = extract_dataset(
            client,
            slug,
            run_dir,
            state,
            mode,
            model,
            max_chunks,
            timeout_seconds,
            agentic_resolution,
        )
        log(
            slug,
            f"completed: +{job.get('classes_added', 0)} classes, +{job.get('properties_added', 0)} properties, "
            f"+{job.get('individuals_added', 0)} individuals",
        )
        return score_dataset(client, slug, run_dir)
    finally:
        client.close()


def run_round(args: argparse.Namespace) -> None:
    missing = [slug for slug in args.datasets if not (PREPARED_DIR / slug / "manifest.json").exists()]
    if missing:
        raise FileNotFoundError(f"Prepare these datasets first: {', '.join(missing)}")
    agentic = {"default": None, "on": True, "off": False}[args.agentic_resolution]
    results: list[dict] = []
    failures: dict[str, str] = {}
    with ThreadPoolExecutor(max_workers=min(args.parallel, len(args.datasets))) as executor:
        futures = {
            executor.submit(
                run_one_dataset,
                slug,
                args.round,
                args.base_url,
                args.username,
                args.password,
                args.model,
                args.mode,
                args.max_chunks,
                args.timeout,
                agentic,
            ): slug
            for slug in args.datasets
        }
        for future in as_completed(futures):
            slug = futures[future]
            try:
                results.append(future.result())
            except Exception as exc:  # noqa: BLE001
                failures[slug] = f"{type(exc).__name__}: {exc}"
                log(slug, f"FAILED: {failures[slug]}")
    round_dir = RUNS_DIR / args.round
    write_json(round_dir / "summary.json", {"round": args.round, "results": results, "failures": failures})
    (round_dir / "SUMMARY.md").write_text(round_summary(args.round, results), encoding="utf-8")
    if failures:
        raise RuntimeError(f"{len(failures)} dataset(s) failed: {failures}")


def score_round(args: argparse.Namespace) -> None:
    results = []
    skipped: dict[str, str] = {}
    client = OntoPilotClient(args.base_url, args.username, args.password)
    try:
        for slug in args.datasets:
            run_dir = RUNS_DIR / args.round / slug
            state = load_state(run_dir)
            if not state.get("ks_id"):
                skipped[slug] = "no completed run state for this dataset in the selected round"
                log(slug, f"skipped: {skipped[slug]}")
                continue
            results.append(score_dataset(client, slug, run_dir))
    finally:
        client.close()
    round_dir = RUNS_DIR / args.round
    write_json(
        round_dir / "summary.json",
        {"round": args.round, "results": results, "failures": {}, "skipped": skipped},
    )
    (round_dir / "SUMMARY.md").write_text(round_summary(args.round, results), encoding="utf-8")


def compare_rounds(left: str, right: str) -> str:
    left_payload = read_json(RUNS_DIR / left / "summary.json")
    right_payload = read_json(RUNS_DIR / right / "summary.json")
    left_rows = {row["dataset"]: row for row in left_payload["results"]}
    right_rows = {row["dataset"]: row for row in right_payload["results"]}
    lines = [
        f"# Public Industrial Benchmark — {left} vs {right}",
        "",
        "| Dataset | Δ class recall | Δ taxonomy F1 | Δ isolated ratio | Δ role collisions |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for slug in sorted(left_rows.keys() & right_rows.keys()):
        before, after = left_rows[slug], right_rows[slug]
        lines.append(
            f"| {slug} | {after['coverage']['classes']['recall'] - before['coverage']['classes']['recall']:+.4f} | "
            f"{after['taxonomy']['metrics']['f1'] - before['taxonomy']['metrics']['f1']:+.4f} | "
            f"{after['structure']['taxonomy_isolated_ratio'] - before['structure']['taxonomy_isolated_ratio']:+.4f} | "
            f"{len(after['structure']['tbox_abox_label_collisions']) - len(before['structure']['tbox_abox_label_collisions']):+d} |"
        )
    return "\n".join(lines) + "\n"


def selected_datasets(value: str) -> list[str]:
    values = [part.strip() for part in value.split(",") if part.strip()]
    unknown = [value for value in values if value not in DATASETS]
    if unknown:
        raise argparse.ArgumentTypeError(f"Unknown datasets: {', '.join(unknown)}")
    return values


def add_dataset_arg(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--datasets",
        type=selected_datasets,
        default=list(DATASETS),
        help=f"Comma-separated dataset slugs (default: {','.join(DATASETS)})",
    )


def add_api_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--base-url", default="http://127.0.0.1:8000")
    parser.add_argument("--username", default=os.getenv("ONTOPILOT_USERNAME", "admin"))
    parser.add_argument("--password", default=os.getenv("ONTOPILOT_PASSWORD", "admin"))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare", help="Build frozen prose corpora and offline gold files")
    add_dataset_arg(prepare)
    prepare.add_argument("--max-chars", type=int, default=70000, help="Maximum prose characters per dataset")

    run = subparsers.add_parser("run", help="Create knowledge systems and run datasets concurrently")
    add_dataset_arg(run)
    add_api_args(run)
    run.add_argument("--round", default="round-01")
    run.add_argument("--parallel", type=int, default=3)
    run.add_argument("--mode", choices=("tbox", "both", "abox"), default="both")
    run.add_argument("--model")
    run.add_argument("--max-chunks", type=int, default=0)
    run.add_argument("--timeout", type=int, default=10800)
    run.add_argument("--agentic-resolution", choices=("default", "on", "off"), default="default")

    score = subparsers.add_parser("score", help="Re-score an existing round")
    add_dataset_arg(score)
    add_api_args(score)
    score.add_argument("--round", default="round-01")

    compare = subparsers.add_parser("compare", help="Compare two completed rounds")
    compare.add_argument("left")
    compare.add_argument("right")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "prepare":
        prepare_all(args.datasets, args.max_chars)
    elif args.command == "run":
        run_round(args)
    elif args.command == "score":
        score_round(args)
    elif args.command == "compare":
        report = compare_rounds(args.left, args.right)
        target = RUNS_DIR / f"COMPARE-{args.left}-vs-{args.right}.md"
        target.write_text(report, encoding="utf-8")
        print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
