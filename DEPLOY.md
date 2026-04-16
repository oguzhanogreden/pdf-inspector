# Deployment

This branch adds a Dockerized HTTP API server for `pdf-inspector` and a [Kamal](https://kamal-deploy.org) deployment config.

The image bundles three binaries:
- `server` — HTTP API (default `ENTRYPOINT`)
- `pdf2md` — CLI extraction
- `detect-pdf` — CLI classification

## HTTP API

The server listens on `$PORT` (default `3000`) and exposes:

| Method | Path       | Description                                          |
|--------|------------|------------------------------------------------------|
| GET    | `/health`  | Liveness probe — `{"status":"ok"}`                  |
| POST   | `/extract` | Multipart `file=` PDF upload → full extraction JSON |
| POST   | `/detect`  | Multipart `file=` PDF upload → detection JSON       |

Body limit: 100 MB. Uploads must start with the `%PDF` magic bytes.

### `POST /extract` response

```json
{
  "pdf_type": "text_based",
  "page_count": 12,
  "has_text": true,
  "processing_time_ms": 140,
  "markdown": "# Title\n\n...",
  "markdown_length": 4821,
  "pages_needing_ocr": [],
  "is_complex": false,
  "pages_with_tables": [3, 7],
  "pages_with_columns": [],
  "has_encoding_issues": false,
  "confidence": 0.97,
  "title": "Annual Report 2024"
}
```

For scanned PDFs: `markdown` is `null`, `has_text` is `false`, and `pdf_type` is `"scanned"` or `"image_based"`. The response is still 200 — the caller decides whether to route to OCR.

### `POST /detect` response

```json
{
  "pdf_type": "scanned",
  "page_count": 8,
  "pages_needing_ocr": [1, 2, 3, 4, 5, 6, 7, 8],
  "processing_time_ms": 22,
  "has_encoding_issues": false,
  "confidence": 0.99,
  "title": null
}
```

### Error responses

```json
{ "error": "no_file",         "message": "Expected a multipart field named 'file'" }
{ "error": "not_pdf",         "message": "Uploaded file does not start with %PDF magic bytes" }
{ "error": "file_too_large",  "message": "PDF exceeds maximum size of 104857600 bytes" }
{ "error": "processing_error", "message": "..." }
```

| Code              | HTTP status |
|-------------------|-------------|
| `no_file`         | 400         |
| `bad_request`     | 400         |
| `not_pdf`         | 400         |
| `file_too_large`  | 413         |
| `processing_error`| 422         |

## Local Docker

```bash
docker build -t pdf-inspector .

docker run --rm -p 3000:3000 pdf-inspector

curl http://localhost:3000/health
curl -X POST -F "file=@./tests/fixtures/some.pdf" http://localhost:3000/extract | jq .
curl -X POST -F "file=@./tests/fixtures/some.pdf" http://localhost:3000/detect  | jq .
```

The CLI binaries are still available inside the container:

```bash
docker run --rm -v "$PWD:/data:ro" --entrypoint pdf2md     pdf-inspector /data/file.pdf
docker run --rm -v "$PWD:/data:ro" --entrypoint detect-pdf pdf-inspector /data/file.pdf
```

## Deploying with Kamal

`config/deploy.yml` is a template. Fill in the placeholders:

```yaml
image: YOUR_REGISTRY_USERNAME/pdf-inspector
servers:
  web:
    - YOUR_SERVER_IP
proxy:
  ssl: true
  host: YOUR_DOMAIN
registry:
  server: ghcr.io
  username: YOUR_REGISTRY_USERNAME
```

Provide the registry password via Kamal secrets (e.g. `.kamal/secrets`):

```bash
KAMAL_REGISTRY_PASSWORD=ghp_xxxxx
```

Then:

```bash
# First-time setup on the target server (installs kamal-proxy, deploys, gets SSL cert)
kamal setup

# Subsequent releases
kamal deploy
```

Kamal will build the image locally, push it to the registry, pull it on the server, run a health check against `/health`, then swap traffic atomically.

## Environment variables

| Variable                    | Default                       | Notes                                       |
|-----------------------------|-------------------------------|---------------------------------------------|
| `PORT`                      | `3000`                        | TCP port the server binds to                |
| `PDF_INSPECTOR_BCMAPS_DIR`  | `/opt/pdf-inspector/bcmaps`   | Set in Dockerfile; needed for CJK fonts     |
| `RUST_LOG`                  | unset                         | e.g. `info`, `pdf_inspector::detector=debug`|
