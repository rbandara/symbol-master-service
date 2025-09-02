# Symbol Sync Service

A Rust-based service for syncing and maintaining a master list of stock symbols from **Finnhub**, including metadata like company name, sector, industry, IPO date, market capitalization, and active status. Designed for **TimescaleDB/Postgres** and supports **metrics** and **rate-limited API access**.

---

## Features

- Fetches all US stock symbols from Finnhub.
- Fetches detailed company profile for each symbol.
- Tracks **new symbols** and **delisted symbols**.
- Stores symbol metadata in a `symbol_master` table.
- Fully **idempotent upserts** to avoid duplicates.
- Tracks **job status** and exposes metrics via Prometheus.
- Handles API rate limits gracefully (60 calls/min by default).
- Logs using `tracing` with info/warn/error levels.

---

## Tech Stack

- **Rust** – for high-performance, async operations.
- **Tokio** – asynchronous runtime.
- **SQLx** – Postgres/TimescaleDB integration.
- **reqwest + serde** – for HTTP requests and JSON parsing.
- **Metrics + Prometheus exporter** – track API calls, new/delisted symbols, active symbols, and errors.
- **Governor** – rate limiting for API calls.
- **dotenvy** – load environment variables from `.env`.

---

## Database Schema

### `symbol_master`
| Column        | Type        | Description |
|---------------|------------|------------|
| symbol        | TEXT       | Stock ticker, unique |
| exchange      | TEXT       | Exchange code (e.g., NASDAQ) |
| name          | TEXT       | Company name |
| sector        | TEXT       | Sector (optional) |
| industry      | TEXT       | Industry (optional) |
| currency      | TEXT       | Trading currency |
| country       | TEXT       | Headquarters country |
| ipo_date      | DATE       | IPO listing date |
| market_cap    | BIGINT     | Market capitalization |
| is_active     | BOOLEAN    | Active/tracked status |
| data_source   | TEXT       | Data provider (default: Finnhub) |
| last_updated  | TIMESTAMP  | Last update timestamp |

### `job_status`
Tracks completion of sync jobs:

| Column    | Type       | Description |
|-----------|-----------|------------|
| job_name  | TEXT      | Name of the job (e.g., symbol_sync) |
| last_run  | TIMESTAMP | Last execution time |
| status    | TEXT      | Status (`success` / `failed`) |
| details   | TEXT      | Optional job details |

---

## Docker Setup

### `Dockerfile`

- Multi-stage build:
  1. **Builder stage**: Compile Rust binary.
  2. **Runtime stage**: Minimal Debian image with `libpq` for Postgres.
- Binary copied to `/usr/local/bin/symbol-sync`.

### `docker-compose.yml` (example)

```yaml
version: "3.9"

services:
  timescaledb:
    image: timescale/timescaledb-ha:pg15-latest
    environment:
      POSTGRES_USER: xxxx
      POSTGRES_PASSWORD: xxxx
      POSTGRES_DB: xxxdb
    ports:
      - "5432:5432"
    volumes:
      - timescale_data:/var/lib/postgresql/data

  symbol-sync:
    build: .
    depends_on:
      - timescaledb
    environment:
      DATABASE_URL: postgres://postgres:postgres@timescaledb:5432/marketdb
      FINNHUB_API_KEY: your_api_key_here
    command: ["symbol-sync"]

volumes:
  timescale_data:
```

---

## Environment Variables

| Variable          | Description |
|------------------|------------|
| `DATABASE_URL`    | Postgres/TimescaleDB connection string. |
| `FINNHUB_API_KEY` | Finnhub API key. |
| `DOTENVY_PATH`    | Optional path to `.env` for local testing. |

---

## Running Locally

1. Clone the repo:
```bash
git clone https://github.com/your-repo/symbol-sync.git
cd symbol-sync
```

2. Create a `.env` file:
```env
DATABASE_URL=postgres://postgres:postgres@localhost:5432/marketdb
FINNHUB_API_KEY=your_api_key_here
```

3. Start TimescaleDB and build/run the service:
```bash
docker compose up --build
```

4. The service will fetch symbols, populate `symbol_master`, and expose metrics for Prometheus scraping.

---

## Metrics

Prometheus metrics exported via `metrics_exporter_prometheus`:

- `symbol_sync_total_symbols` – total symbols fetched.
- `symbol_sync_new_symbols` – new symbols inserted.
- `symbol_sync_delisted_symbols` – symbols marked inactive.
- `symbol_sync_active_symbols` – current active symbols.
- `symbol_sync_api_calls` – number of Finnhub API calls.
- `symbol_sync_errors` – count of errors during sync.
- `symbol_sync_completed` – completed sync jobs.

---

## Logging

Uses `tracing`:

- **INFO** – progress and symbol processing.
- **WARN** – temporary rate limits or retries.
- **ERROR** – failed fetches, parse errors, or validation issues.

---

## Notes

- Handles **rate limiting** gracefully using `Governor`.
- Automatically upserts new symbols and marks delisted symbols as inactive.
- Can be extended to fetch and sync earnings data or other financial information.

