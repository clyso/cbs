# 023 — Metrics: Deployment & Dashboards

## Status

Draft v1. Third of three design documents implementing accepted proposal
[002](../proposals/002-20260625T0839-metrics-observability-websocket-push.md).
This document covers the **operational layer**: config example files, the
podman-compose monitoring stack, the Prometheus scrape config, server-host
node_exporter, and the **provisioned Grafana dashboards** (dashboards-as-code;
alerting rules deferred to a follow-up per the agreed scope).

Companions: 021 (wire protocol + worker collector), 022 (server exporter +
aggregation). Metric names/labels referenced below are defined in 022.

## Config examples

### `cbsd-rs/config/server.example.yaml`

Add a `metrics` section (keys are kebab-case; see 022 `MetricsConfig`):

```yaml
metrics:
  enabled: true
  # Dedicated listener, NOT published outside the monitoring network.
  # Set to null to instead serve /metrics on the main API listener.
  bind: "0.0.0.0:9090"
  stale-after-secs: 45
  gauge-refresh-secs: 5
```

### `cbsd-rs/config/worker.example.yaml`

```yaml
metrics:
  enabled: true
  push-interval-secs: 15
  ccache-interval-secs: 60
```

Both sections are optional and default-on (021/022). The worker has no bind — it
pushes over the existing WebSocket.

## Compose monitoring stack (`podman-compose.cbsd-rs.yaml`)

Add three services. **No worker-side services** — the defining property of
proposal 002 is zero metrics daemons on worker hosts.

| Service         | Image                | Role                       | Published?                                                   |
| --------------- | -------------------- | -------------------------- | ------------------------------------------------------------ |
| `prometheus`    | `prom/prometheus`    | scrape + TSDB              | only Grafana needs it; publish 9091→9090 for local debugging |
| `grafana`       | `grafana/grafana`    | dashboards                 | publish 3000 (or behind the existing Caddy)                  |
| `node-exporter` | `prom/node-exporter` | **server host** OS metrics | not published; scraped internally                            |

The cbsd-server `metrics.bind` port (`9090`) is exposed **only on the compose
network** (not in `ports:`), so Prometheus reaches `server-dev:9090` while the
endpoint stays off the public surface. node-exporter runs beside the server (the
reachable host); there is deliberately none on workers.

## Prometheus scrape config

`container/monitoring/prometheus.yml` (mounted into the prometheus service):

```yaml
global:
  scrape_interval: 15s # matches the worker push-interval default
scrape_configs:
  - job_name: cbsd-server
    static_configs:
      - targets: ["server-dev:9090"]
  - job_name: node-server
    static_configs:
      - targets: ["node-exporter:9100"]
```

`cbsd-server:9090` carries **both** families from 022 (server-owned `cbsd_*` and
the per-worker `cbsd_worker_*` pushed series), so a single scrape target covers
the whole fleet — no per-worker discovery. The scrape interval matches the
worker push cadence so the 45 s gauge idle-timeout (`stale_after ≈ 3×`) has
margin.

### Tuning the cadences as a unit

Four knobs are coupled and must be changed together, not in isolation (they live
in three files — worker `worker.example.yaml`, server `server.example.yaml`, and
this `prometheus.yml` — so the relationship is easy to break):

- `metrics.push-interval-secs` (worker, default 15) ≈ `scrape_interval`
  (Prometheus, default 15): the server only re-`set()`s a worker gauge when a
  push arrives, so scraping faster than workers push yields no new samples.
- `metrics.stale-after-secs` (server, default 45) ≥ ~3× `push-interval`: a
  worker gauge must survive a couple of missed pushes before it is idle-pruned.
- `metrics.gauge-refresh-secs` (server, default 5) < `stale-after-secs`
  (enforced in `validate()`, 022): the server-owned gauge refresh / `run_upkeep`
  tick must be well inside the idle window or those gauges vanish between
  scrapes.
- `metrics.ccache-interval-secs` (worker, default 60) is decoupled from
  staleness — the carried-forward ccache value rides every push (021), so this
  governs only `ccache -s` cost, not gauge freshness.

The defaults satisfy all four relationships; an operator changing one should
re-check the chain.

## Grafana provisioning (dashboards-as-code)

Provision the datasource and dashboards from files (no click-ops), mounted into
the grafana service:

```
container/monitoring/grafana/
  provisioning/datasources/prometheus.yml   # Prometheus datasource (default)
  provisioning/dashboards/cbsd.yml          # dashboard provider → /dashboards
  dashboards/*.json                          # the dashboards below
```

The dashboard JSON is generated from the panel/query spec below; this design
fixes the panels and their PromQL (the contract), the JSON is the build
artifact. Alerting rules are **out of scope** here (deferred follow-up).

### Dashboard: Build Queue & Throughput

| Panel                    | Type             | PromQL                                                                                                                                                       |
| ------------------------ | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Queue depth by priority  | stacked area     | `sum by (priority) (cbsd_builds_queued)`                                                                                                                     |
| Active builds            | stat/time series | `sum(cbsd_builds_active)`                                                                                                                                    |
| Build rate by result     | time series      | `sum by (result) (rate(cbsd_build_results_total[5m]))`                                                                                                       |
| Success ratio            | stat             | `sum(rate(cbsd_build_results_total{result="success"}[1h])) / clamp_min(sum(rate(cbsd_build_results_total[1h])), 1)` (guards the empty-window divide-by-zero) |
| Re-dispatch (retry) rate | time series      | `sum by (reason) (rate(cbsd_build_requeues_total[15m]))`                                                                                                     |
| Queue wait p50/p95       | time series      | `histogram_quantile(0.95, sum by (le) (rate(cbsd_build_queue_wait_seconds_bucket[15m])))`                                                                    |

### Dashboard: Build Duration & SLOs

| Panel                   | Type        | PromQL                                                                                                   |
| ----------------------- | ----------- | -------------------------------------------------------------------------------------------------------- |
| Duration p50/p90/p99    | time series | `histogram_quantile(0.99, sum by (le) (rate(cbsd_build_duration_seconds_bucket[1h])))`                   |
| Time-to-failure p50/p95 | time series | `histogram_quantile(0.95, sum by (le) (rate(cbsd_build_duration_seconds_bucket{result="failure"}[1h])))` |
| Duration heatmap        | heatmap     | `sum by (le) (rate(cbsd_build_duration_seconds_bucket[5m]))`                                             |
| Timeouts & SIGKILLs     | time series | `rate(cbsd_build_timeouts_total[1h])`, `rate(cbsd_sigkill_escalations_total[1h])`                        |

"Average time to failed build" is the `result="failure"` quantile panel — a
query, not a stored metric (proposal 001/002 reframing).

### Dashboard: Per-Worker

| Panel                  | Type        | PromQL                                                                                                                                          |
| ---------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| Duration p95 by worker | time series | `histogram_quantile(0.95, sum by (le, worker) (rate(cbsd_build_duration_seconds_bucket[1h])))`                                                  |
| Throughput by worker   | time series | `sum by (worker) (rate(cbsd_build_results_total[15m]))`                                                                                         |
| Subprocess exits       | time series | `sum by (worker, result) (rate(cbsd_worker_subprocess_exits_total[15m]))` (worker-classified; cross-check vs server `cbsd_build_results_total`) |
| Reconnects by worker   | time series | `sum by (worker) (rate(cbsd_worker_reconnects_total[1h]))`                                                                                      |
| Worker uptime          | stat table  | `cbsd_worker_uptime_seconds`                                                                                                                    |

### Dashboard: Worker Host & ccache

| Panel               | Type        | PromQL                                                                                                       |
| ------------------- | ----------- | ------------------------------------------------------------------------------------------------------------ |
| CPU busy by worker  | time series | `cbsd_worker_host_cpu_busy_ratio`                                                                            |
| Load average (1m)   | time series | `cbsd_worker_host_load1`                                                                                     |
| Memory used / avail | time series | `cbsd_worker_host_mem_used_bytes / cbsd_worker_host_mem_total_bytes`, `cbsd_worker_host_mem_available_bytes` |
| Swap used / total   | time series | `cbsd_worker_host_swap_used_bytes / cbsd_worker_host_swap_total_bytes`                                       |
| Filesystem fill     | time series | `cbsd_worker_host_fs_used_bytes / cbsd_worker_host_fs_total_bytes` (legend `{{worker}} {{mount}}`)           |
| Disk IO             | time series | `rate(cbsd_worker_host_disk_read_bytes_total[5m])`, `…written…`                                              |
| ccache size vs max  | time series | `cbsd_worker_ccache_size_bytes`, `cbsd_worker_ccache_max_bytes`                                              |
| ccache hit ratio    | time series | `cbsd_worker_ccache_hit_ratio`                                                                               |
| Push drops          | time series | `rate(cbsd_worker_metrics_push_drops_total[15m])`                                                            |

### Dashboard: Build ↔ Resource Correlation

Satisfies the proposal's "correlate CPU/mem/IO with builds" requirement as a
**dashboard overlay**, a clean per-`worker` join since both the server-owned
build metrics (`cbsd_build_results_total{worker}`) and the pushed host gauges
(`cbsd_worker_host_*{worker}`) carry the same server-stamped `worker` label:

| Panel                              | Type                | PromQL                                                                                                                                     |
| ---------------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| CPU vs build completions (per wkr) | time series, 2 axes | `cbsd_worker_host_cpu_busy_ratio{worker="$worker"}` overlaid with `sum by (worker) (rate(cbsd_build_results_total{worker="$worker"}[5m]))` |
| Build events                       | annotations         | mark `increase(cbsd_build_results_total{worker="$worker"}[1m]) > 0` on the resource panels                                                 |
| Memory vs spool                    | time series         | `cbsd_worker_host_mem_used_bytes{worker="$worker"}`, `cbsd_worker_spool_bytes{worker="$worker"}`                                           |

A `worker` template variable
(`label_values(cbsd_worker_uptime_seconds, worker)`) drives the per-worker
dashboards.

### Dashboard: Fleet & Server Health

| Panel                      | Type        | PromQL                                                                                                                                                      |
| -------------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Workers connected by state | time series | `sum by (state) (cbsd_workers_connected)`                                                                                                                   |
| DB pool saturation         | time series | `cbsd_db_pool_connections{state="acquired"}` vs the pool max (see note)                                                                                     |
| Dispatch latency p95       | time series | `histogram_quantile(0.95, sum by (le) (rate(cbsd_dispatch_latency_seconds_bucket[15m])))`                                                                   |
| Ack timeouts               | time series | `rate(cbsd_dispatch_ack_timeouts_total[15m])`, `rate(cbsd_revoke_ack_timeouts_total[15m])`                                                                  |
| HTTP RED                   | time series | `sum by (status) (rate(cbsd_http_requests_total[5m]))`, `histogram_quantile(0.95, sum by (le,route) (rate(cbsd_http_request_duration_seconds_bucket[5m])))` |
| Periodic outcomes          | time series | `sum by (result) (rate(cbsd_periodic_fires_total[1d]))`                                                                                                     |
| Periodic schedule lag p95  | time series | `histogram_quantile(0.95, sum by (le) (rate(cbsd_periodic_schedule_lag_seconds_bucket[15m])))`                                                              |

The DB-pool panel highlights the 4-connection saturation invariant (CLAUDE.md
invariant #2) — its whole value is making that latent deadlock risk observable.
`cbsd_db_pool_connections{state="acquired"}` is derived server-side as
`pool.size() − pool.num_idle()` (sqlx exposes no direct acquired count). The
saturation reference line is the pool max (currently 4, CLAUDE.md invariant #2);
the dashboard should template it as a constant so it tracks the invariant rather
than hard-coding a literal that silently drifts if the pool is resized.

## Server-host node_exporter

The reachable server host runs node_exporter (CPU/mem/disk/IO/FD) scraped
directly (`node-server` job). This is the one place node_exporter still fits —
the worker constraint does not apply to the server. A standard node-exporter
dashboard (e.g. the community 1860 board) covers it; not re-specified here.

## Deferred to follow-up

- **Alerting rules** (Prometheus/Alertmanager): queue-wait SLO breach, success-
  ratio drop, DB-pool saturation, worker fleet shrinkage, sustained push drops.
  Deferred per agreed scope; the metrics above are the inputs.
- **Dashboard JSON** is generated from the spec above during implementation;
  keep the panel/query contract here authoritative if they drift.
- **Caddy/route exposure** for Grafana (behind the existing reverse proxy vs a
  published port) — an ops choice for the implementation.
