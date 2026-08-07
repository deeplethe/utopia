# OntoPilot backend — FastAPI + embedded Oxigraph RDF store + SQLite.
FROM python:3.12-slim

WORKDIR /app

# Most deps ship manylinux wheels (pyoxigraph, bcrypt, rdflib, pyshacl…), so no build toolchain
# is needed. If a future dep requires compiling, add: RUN apt-get update && apt-get install -y gcc
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app ./app

# All runtime state (SQLite DB, document blobs, Oxigraph store) lives under /app/data — mount a
# volume there to persist it (see docker-compose.yml).
ENV PYTHONUNBUFFERED=1
EXPOSE 8000
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
