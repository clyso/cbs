# Monitoring cbsd-rs

The server exposes a single Prometheus `/metrics` endpoint that carries **both**
its own series (`cbsd_*` — queue, build outcomes/durations, dispatch, HTTP, DB
pool, periodic) and the per-worker series (`cbsd_worker_*`) that workers push to
it over the existing WebSocket. One scrape target therefore covers the whole
fleet — there is no per-worker scrape endpoint and no service discovery, so
remote or NAT'd workers need no inbound port. Host OS metrics come from a
`node_exporter` running beside the server.

## 1. Enable metrics

**Server** (`server.yaml`):

```yaml
metrics:
  enabled: true
  bind: "0.0.0.0:9090" # dedicated listener (default); null → main listen-addr
  stale-after-secs: 45 # prune a silent worker's gauges after this
  gauge-refresh-secs: 5 # server-owned gauge resync interval
```

`bind` defaults to `0.0.0.0:9090` (an omitted line resolves to the same), a
dedicated listener that keeps `/metrics` off the public API listener (see
Security below); it must differ from `listen-addr`. Set `bind: null` explicitly
to instead expose a top-level `/metrics` route on the main `listen-addr` (it is
served at `/metrics`, not under `/api`). `gauge-refresh-secs` must be less than
`stale-after-secs`.

**Worker** (`worker.yaml`):

```yaml
metrics:
  enabled: true
  push-interval-secs: 15
  ccache-interval-secs: 60 # slower cadence for `ccache --print-stats`
```

The worker exposes no endpoint; it only pushes, and only when the server (with
metrics enabled) asks it to. Disable on either side to turn collection off.

### Cadence tuning (keep as a set)

- `push-interval-secs` ≈ Prometheus `scrape_interval`.
- `stale-after-secs` ≥ ~3× `push-interval-secs`, so one dropped push does not
  prune a worker's host gauges.
- `gauge-refresh-secs` < `stale-after-secs`, so server-owned gauges are
  re-touched and never idle-expire between scrapes.

## 2. Prometheus

Use the provided scrape config `container/monitoring/prometheus.yml` (jobs
`cbsd-server` and `node-server`). Point the `cbsd-server` target at the server's
`metrics.bind` address and the `node-server` target at the host's
`node_exporter` (`:9100`). Keep `scrape_interval` aligned with the worker
`push-interval-secs` (default 15s). Set retention/storage per your environment.

## 3. Grafana

Provision from `container/monitoring/grafana/` (no click-ops):

- `provisioning/datasources/prometheus.yml` — the Prometheus datasource.
- `provisioning/dashboards/cbsd.yml` — loads the dashboards below from
  `dashboards/*.json`.

Six dashboards ship: Build Queue & Throughput, Build Duration & SLOs,
Per-Worker, Worker Host & ccache, Build ↔ Resource Correlation, and Fleet &
Server Health. The per-worker dashboards use a `worker` template variable. The
server host's `node_exporter` is covered by any standard board (e.g. 1860).

## 4. Security

`/metrics` is **unauthenticated**. Keep it on an internal network or bound to a
private interface, and never publish the `metrics.bind` port to the public
internet. `node_exporter` is likewise internal-only. Scrape over a trusted
network (or a TLS-terminating reverse proxy / mesh) between Prometheus and the
targets.

## 5. Quick start (dev)

The compose stack wires all of this up for local use:

```bash
podman-compose -f podman-compose.cbsd-rs.yaml --profile dev up
```

Prometheus is published on `:9091` (debug UI) and Grafana on `:3000` (anonymous
admin — dev only). The server's `:9090` stays on the compose network. For
production, run Prometheus, Grafana, and `node_exporter` under your own
orchestrator, mounting the same `container/monitoring/` configs.
