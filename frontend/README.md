# OntoPilot Frontend

The OntoPilot frontend is a React and TypeScript single-page application for managing documents,
reviewing extraction jobs, exploring RDF/OWL ontologies, resolving conflicts, and auditing changes.

For the complete project overview and deployment instructions, see the repository
[README](../README.md).

## Stack

- React 19
- TypeScript 6
- Vite 8
- Tailwind CSS 4
- Radix UI and shadcn components
- React Flow and Dagre for ontology visualization
- Recharts for metrics
- KaTeX for ontology expressions

## Development

Requirements:

- Node.js 22+
- pnpm
- OntoPilot backend running at `http://127.0.0.1:8000`

```powershell
pnpm install
pnpm dev
```

The Vite server runs at <http://localhost:5173> and proxies `/api` requests to the backend.

## Validation

```powershell
pnpm lint
pnpm build
```

## Production Image

The frontend Dockerfile builds the static Vite bundle and serves it through Nginx. Nginx also
proxies `/api` to the backend service, keeping browser requests same-origin.
