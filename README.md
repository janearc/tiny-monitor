# tiny-monitor

The frameless, always-on-top macOS floating widget for `obs-svc`. It renders the
fleet's per-service / per-project health and the LLM token-runway readout at a
glance, fed by the `obs-svc-agg` aggregator. Think the macOS Activity Monitor
floating CPU window, for fleet health.

This crate is the presentation layer (`tiny-monitor`) named in the architecture
record (`observability_architecture_v1.md` §3.3). It is its own repository -- a
standalone Rust crate that consumes the `big-little-mesh-contracts` contract
crate; it does not touch the Go `obs-svc-agg` module.

## Data path

The architecture of record specifies a 2-second gRPC `WidgetStatePayload` feed
from `obs-svc-agg` to the widget. That feed is a documented follow-up and does
not exist yet. The aggregator exposes `GET /state` (a JSON snapshot) today, so
the widget polls `/state` over HTTP at the same ~2s cadence and renders the
result. When the gRPC feed lands, `src/fetch.rs` is the single seam that
changes — the render and window layers consume a `Snapshot` regardless of how it
arrived.

```
obs-svc-agg  GET /state (JSON)
      │  HTTP poll @ ~2s  (eventual: 2s gRPC WidgetStatePayload feed)
      ▼
tiny-monitor
  · fetch + parse snapshot          (src/fetch.rs, src/model.rs)
  · health -> colour, glance view   (src/render.rs)
  · NSWindow (borderless, floating)  (src/main.rs)
```

The widget is **stateless**: each tick re-renders the latest snapshot. It holds
no history. When `obs-svc-agg` is unreachable it draws a degraded `NO DATA` view
(neutral grey, the reason on screen) instead of crashing or showing stale state.

## Health → colour

| State       | Colour       | Meaning                                  |
| ----------- | ------------ | ---------------------------------------- |
| GREEN       | green        | healthy                                  |
| YELLOW      | amber        | degraded                                 |
| RED         | red          | failing                                  |
| EXHAUSTED   | dark red     | quota fully consumed (terminal)          |
| UNSPECIFIED | neutral grey | unknown / no data — deliberately not green |

The fleet headline prefers the aggregator's own `fleet.overall` rollup. Against
a build that does not emit it (the current backups-only aggregator), the rollup
is recomputed worst-wins over per-service states, falling back to the daemon
`healthy` flag, and the per-project backup outcomes are shown so the widget is
never blank against today's aggregator.

## Run it

Requires the Rust toolchain (`cargo`) and macOS (for the native window).

```sh
cd tiny-monitor
cargo run --release
```

A small dark panel appears near the lower-left of the screen, floating above
other windows. Drag it anywhere by its background. There is no title bar and no
Dock icon (it runs as a macOS accessory). Quit it from the terminal that
launched it (`Ctrl-C`), or `pkill tiny-monitor`.

### Configuration

| Variable          | Default                          | Meaning                              |
| ----------------- | -------------------------------- | ------------------------------------ |
| `OBS_AGG_URL`     | `http://127.0.0.1:8090/state`    | aggregator `/state` endpoint         |
| `OBS_POLL_SECS`   | `2`                              | poll cadence, seconds                |
| `OBS_TIMEOUT_SECS`| `3`                              | per-poll HTTP timeout, seconds       |

Endpoints are not hardcoded into the widget. The default targets a local
`obs-svc-agg`; point `OBS_AGG_URL` at whatever address (Traefik, a published
container port, a remote host) serves `/state`. To confirm the source first:

```sh
curl -s "$OBS_AGG_URL" | python3 -m json.tool
```

> Note: the published host port of the `obs-svc-agg` container can vary (Docker
> may map the internal `8090` to an ephemeral host port). Set `OBS_AGG_URL` to
> the mapped port, or run against Traefik. `docker ps` shows the mapping.

### See it with no aggregator

With nothing on `OBS_AGG_URL`, the widget still launches and shows the degraded
`NO DATA` view — this is the graceful-degradation path, and the quickest way to
confirm the window renders.

## Tests

The load-bearing logic — the `/state` fetch/parse, the health→colour mapping,
and the glance-view derivation — is library code with no AppKit dependency and
is unit-tested headlessly:

```sh
cargo test
cargo clippy --all-targets
cargo fmt --check
```

The NSWindow itself cannot be unit-tested without a display; it is kept thin in
`src/main.rs` and verified by running the binary (a human step).

## Layout

| Path           | Contents                                                        |
| -------------- | -------------------------------------------------------------- |
| `src/model.rs` | `/state` snapshot model (every field optional — partial-snapshot safe) |
| `src/fetch.rs` | HTTP poll of `/state`, parse, runtime `Config` from env        |
| `src/render.rs`| `Snapshot` → `RenderModel` (rollup, rows, runway), health→colour |
| `src/main.rs`  | the borderless floating `NSWindow` shell + poll loop           |

## author

max toegang <max.toegang@ftml.net>
🤖 claude · claude-opus-4-8
🤖 bespoke locally trained models
