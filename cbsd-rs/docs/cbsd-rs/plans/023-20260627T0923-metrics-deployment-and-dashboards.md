# Phase: Metrics — Deployment & Dashboards

**Design document:**
`cbsd-rs/docs/cbsd-rs/design/023-20260626T0944-metrics-deployment-and-dashboards.md`

Companion plans: `021-…-metrics-protocol-and-worker-collector.md`,
`022-…-metrics-server-exporter-and-aggregation.md`. The global build order and
dependency graph live in the 022 plan.

## Progress

| #   | Commit                                                            | ~LOC                | Status  |
| --- | ----------------------------------------------------------------- | ------------------- | ------- |
| 1   | `cbsd-rs: add Prometheus/Grafana monitoring stack and dashboards` | ~mostly config/JSON | Pending |

**Total:** 1 commit (config + provisioned dashboard JSON).

This plan owns global build step **G7**. It depends on all prior steps: the
server `/metrics` endpoint (G1–G5) and the worker push path (G6) must exist for
the scrape target and dashboards to have data.

---

## Why one commit

Deployment is a single coherent operational unit: the compose services, the
scrape config, and the provisioned dashboards only make sense together. None of
it is Rust; the dashboard JSON is generated from the panel/PromQL contract fixed
in design 023. Splitting would leave half-wired intermediate states with no
testable value.

---

## Commit 1 (G7): monitoring stack and dashboards

**Delivers:** `podman-compose -f podman-compose.cbsd-rs.yaml up` brings up
Prometheus scraping the server `/metrics` (plus a server-host node-exporter) and
Grafana with the provisioned dashboards.

**Files:**

- `cbsd-rs/config/server.example.yaml` (add `metrics:` section)
- `cbsd-rs/config/worker.example.yaml` (add `metrics:` section)
- `podman-compose.cbsd-rs.yaml` (add `prometheus`, `grafana`, `node-exporter`
  services; expose server `metrics.bind` on the compose network only)
- `container/monitoring/prometheus.yml` (new — scrape config)
- `container/monitoring/grafana/provisioning/datasources/prometheus.yml` (new)
- `container/monitoring/grafana/provisioning/dashboards/cbsd.yml` (new)
- `container/monitoring/grafana/dashboards/*.json` (new — generated from the 023
  panel/PromQL spec)

**Steps:**

1. Config examples: server gets `enabled`, `bind: "0.0.0.0:9090"`,
   `stale-after-secs: 45`, `gauge-refresh-secs: 5`; worker gets `enabled`,
   `push-interval-secs: 15`, `ccache-interval-secs: 60`. Add the coupled-cadence
   tuning note as a comment pointer (design §"Tuning the cadences as a unit").
2. Compose: three services (no worker-side services — the defining property of
   proposal 002). The server `metrics.bind` port is reachable only on the
   compose network (not in `ports:`); node-exporter runs beside the server.
3. `prometheus.yml`: `scrape_interval: 15s`; jobs `cbsd-server`
   (`server-dev:9090`) and `node-server` (`node-exporter:9100`).
4. Grafana provisioning: default Prometheus datasource + a dashboard provider
   pointing at `/dashboards`.
5. Dashboards generated from the 023 contract: Build Queue & Throughput, Build
   Duration & SLOs, Per-Worker, Worker Host & ccache, Build ↔ Resource
   Correlation, Fleet & Server Health. Keep the panel/PromQL contract in design
   023 authoritative if the JSON drifts.

**Verification (manual, ops):**

- `podman-compose … up` starts all services; Prometheus targets page shows both
  jobs UP; Grafana lists the provisioned dashboards and they render against a
  running server + worker.

---

## Implementation notes

- **No exposed-but-un-graphed metrics.** The design-set review's lineage gap was
  closed in design 023: the panel contract now graphs the formerly-orphaned
  families — `cbsd_worker_subprocess_exits_total` (Per-Worker),
  `cbsd_worker_host_load1` / swap / `mem_available` (Worker Host & ccache), and
  `cbsd_dispatch_latency_seconds` / `cbsd_periodic_schedule_lag_seconds` (Fleet
  & Server Health). Generating the dashboards from the 023 contract therefore
  leaves nothing exposed-but-unused.
- Alerting rules are **out of scope** (deferred follow-up per agreed scope); the
  metrics above are the inputs.
- A `worker` template variable
  (`label_values(cbsd_worker_uptime_seconds, worker)`) drives the per-worker
  dashboards.
- Caddy/route exposure for Grafana (reverse proxy vs published port) is an ops
  choice left to implementation.
- Server-host node-exporter is covered by a standard community dashboard (e.g.
  board 1860), not re-specified here. </content>
