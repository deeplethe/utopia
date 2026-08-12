# OntoPilot backend: FastAPI + Oxigraph RDF store + PostgreSQL/SQLite metadata.
FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app ./app

# Documents, Oxigraph graphs, releases, and exports live under /app/data. PostgreSQL metadata
# is persisted by its own Docker Compose volume.
ENV PYTHONUNBUFFERED=1
EXPOSE 8000
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
