// fetch is the read side: it polls obs-svc-agg's GET /state over HTTP and parses
// the body into a Snapshot. The network call and the parse are separated so the
// parse (the part with logic) is unit-tested without a live aggregator.
//
// Transport note: the architecture of record specifies a 2s gRPC snapshot feed
// from obs-svc-agg to the widget. That feed is a documented follow-up and does
// not exist yet; the aggregator exposes GET /state (a JSON snapshot) today, so
// the widget polls it at the same ~2s cadence. When the gRPC feed lands this
// module is the single seam that changes -- the render/window layers consume a
// Snapshot regardless of how it arrived.

use std::time::Duration;

use crate::model::Snapshot;

// Source is how the widget obtains obs-svc-agg's /state URL each tick. The widget no longer
// carries the aggregator's address as a hardcoded default: by default it RESOLVES the address
// through delightd (the one well-known control plane), and only a deliberate human override
// bypasses that. Retiring the old :8090 constant is the point of this seam -- the widget targets
// a service by NAME and trusts delightd to say where it answers.
#[derive(Debug, Clone)]
pub enum Source {
    // Explicit is a fixed /state URL from OBS_AGG_URL: a deliberate human override (a dev
    // collector, a tunnel, a real service address) that skips resolution.
    Explicit(String),
    // Resolve asks delightd for the named service's address. delightd_base is delightd's
    // control-port base; service is the registry name to resolve (e.g. "obs-svc-agg").
    Resolve {
        delightd_base: String,
        service: String,
    },
}

// Config is the widget's runtime configuration, resolved from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    // source is how each tick obtains obs-svc-agg's /state URL: a human override, or
    // resolution through delightd.
    pub source: Source,
    // poll is the tick interval. The architecture's widget cadence is 2s.
    pub poll: Duration,
    // timeout bounds a single poll so a hung aggregator cannot stall the tick.
    pub timeout: Duration,
}

impl Config {
    // from_env resolves configuration from the environment. By default the widget RESOLVES
    // obs-svc-agg through delightd -- it does not hardcode the aggregator's address. An explicit
    // OBS_AGG_URL is a human override that bypasses resolution; OBS_AGG_NAME overrides the
    // registry name to resolve, and DELIGHTD_URL overrides delightd's control-port base.
    pub fn from_env() -> Self {
        let source = match std::env::var("OBS_AGG_URL") {
            Ok(url) => Source::Explicit(url),
            Err(_) => Source::Resolve {
                delightd_base: crate::resolve::delightd_base_from_env(),
                service: std::env::var("OBS_AGG_NAME")
                    .unwrap_or_else(|_| DEFAULT_OBS_AGG_NAME.to_string()),
            },
        };
        let poll = env_secs("OBS_POLL_SECS", DEFAULT_POLL_SECS);
        let timeout = env_secs("OBS_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS);
        Config {
            source,
            poll,
            timeout,
        }
    }

    // state_url returns the /state URL to poll this tick. An Explicit source returns its URL;
    // a Resolve source asks delightd and FAILS LOUDLY (Err) when delightd cannot answer -- the
    // widget never substitutes a hardcoded address, so a degraded resolution stays visible (the
    // caller renders the loud "NO DATA" view). Resolving each tick is deliberate: it is cheap (a
    // localhost registry lookup) and self-healing -- the widget recovers the moment delightd and
    // the registration return, and only ever shows data delightd can currently vouch for.
    pub fn state_url(&self) -> Result<String, String> {
        match &self.source {
            Source::Explicit(url) => Ok(url.clone()),
            Source::Resolve {
                delightd_base,
                service,
            } => crate::resolve::resolve_state_url(delightd_base, service, self.timeout),
        }
    }
}

// DEFAULT_OBS_AGG_NAME is the registry name the widget resolves through delightd to find the
// observability aggregator. Overridable via OBS_AGG_NAME.
pub const DEFAULT_OBS_AGG_NAME: &str = "obs-svc-agg";
const DEFAULT_POLL_SECS: u64 = 2;
const DEFAULT_TIMEOUT_SECS: u64 = 3;

// env_secs reads a u64 seconds value from the environment, falling back to the
// default on absence or a non-numeric value (a typo must not zero the timeout).
fn env_secs(key: &str, default: u64) -> Duration {
    let secs = std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default);
    Duration::from_secs(secs)
}

// fetch_snapshot performs one poll of /state and returns the parsed Snapshot.
// A network error, a non-2xx status, or an unparseable body all surface as Err
// so the caller renders the degraded "no data" view rather than stale state.
pub fn fetch_snapshot(cfg: &Config) -> Result<Snapshot, String> {
    // Resolve the target first: a Resolve source asks delightd, and a resolution miss surfaces
    // here as Err (the loud degraded view) rather than the widget guessing an address.
    let state_url = cfg.state_url()?;
    let agent = ureq::AgentBuilder::new().timeout(cfg.timeout).build();
    let body = match agent.get(&state_url).call() {
        Ok(resp) => resp.into_string().map_err(|e| format!("read body: {e}"))?,
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("aggregator returned HTTP {code}"));
        }
        Err(e) => return Err(format!("unreachable: {e}")),
    };
    parse_snapshot(&body)
}

// parse_snapshot deserialises a /state body. Pulled out of fetch_snapshot so the
// parse is testable against captured aggregator output with no network.
pub fn parse_snapshot(body: &str) -> Result<Snapshot, String> {
    serde_json::from_str::<Snapshot>(body).map_err(|e| format!("parse /state: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // a captured live snapshot from the running obs-svc-agg (backups-only build:
    // services/fleet omitted). The widget must parse this exactly.
    const LIVE_BACKUPS_ONLY: &str = r#"{
        "healthy": true,
        "total_events": 83,
        "backups": {
            "comfyui": {"project":"comfyui","total":37,"successes":37,"failures":0,"last_success":true,"last_bytes_after":16289014,"last_duration_ms":7453,"last_seen":"2026-06-19T07:42:16.945743864Z"},
            "paling": {"project":"paling","total":9,"successes":9,"failures":0,"last_success":true,"last_bytes_after":377307,"last_duration_ms":184,"last_seen":"2026-06-19T07:26:40.739408531Z"}
        }
    }"#;

    // a full snapshot from the post-health-machine aggregator build.
    const FULL_SNAPSHOT: &str = r#"{
        "healthy": true,
        "total_events": 200,
        "backups": {},
        "services": {
            "paling": {"service":"paling","state":"GREEN","last_reported":"GREEN","uptime_seconds":1000,"load_metric":3,"heartbeat_count":42,"last_seen":"2026-06-19T07:42:16Z"},
            "delightd": {"service":"delightd","state":"YELLOW","last_reported":"RED","uptime_seconds":50,"load_metric":9,"heartbeat_count":7,"last_seen":"2026-06-19T07:42:16Z"}
        },
        "fleet": {"overall":"YELLOW","active_nodes":2,"degraded_nodes":1,"exhausted_nodes":0}
    }"#;

    #[test]
    fn parses_live_backups_only_snapshot() {
        let snap = parse_snapshot(LIVE_BACKUPS_ONLY).expect("must parse partial snapshot");
        assert!(snap.healthy);
        assert_eq!(snap.total_events, 83);
        assert_eq!(snap.backups.len(), 2);
        // absent fields degrade to empty/None, not a parse error.
        assert!(snap.services.is_empty());
        assert!(snap.fleet.is_none());
        assert!(snap.quota.is_none());
    }

    #[test]
    fn parses_full_snapshot_with_services_and_fleet() {
        let snap = parse_snapshot(FULL_SNAPSHOT).expect("must parse full snapshot");
        assert_eq!(snap.services.len(), 2);
        let fleet = snap.fleet.expect("fleet present");
        assert_eq!(fleet.overall, "YELLOW");
        assert_eq!(fleet.degraded_nodes, 1);
    }

    #[test]
    fn rejects_garbage_body() {
        assert!(parse_snapshot("not json at all").is_err());
        assert!(parse_snapshot("").is_err());
    }

    #[test]
    fn parses_empty_object_to_defaults() {
        // an empty-but-valid body is data (a daemon with nothing yet), not an
        // error: every field is optional.
        let snap = parse_snapshot("{}").expect("empty object is valid");
        assert!(!snap.healthy);
        assert_eq!(snap.total_events, 0);
    }

    #[test]
    fn config_default_source_resolves_through_delightd() {
        // The widget no longer defaults to a hardcoded aggregator URL: absent an explicit
        // override it resolves obs-svc-agg through delightd at the canonical control port.
        let cfg = Config {
            source: Source::Resolve {
                delightd_base: crate::resolve::DEFAULT_DELIGHTD_URL.to_string(),
                service: DEFAULT_OBS_AGG_NAME.to_string(),
            },
            poll: Duration::from_secs(DEFAULT_POLL_SECS),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };
        match &cfg.source {
            Source::Resolve {
                delightd_base,
                service,
            } => {
                assert_eq!(delightd_base, "http://127.0.0.1:8088");
                assert_eq!(service, "obs-svc-agg");
            }
            other => panic!("default source must resolve through delightd, got {other:?}"),
        }
        assert_eq!(cfg.poll, Duration::from_secs(2));
    }

    #[test]
    fn explicit_source_state_url_is_passthrough() {
        // An explicit OBS_AGG_URL override bypasses resolution entirely (no network).
        let cfg = Config {
            source: Source::Explicit("http://127.0.0.1:9999/state".to_string()),
            poll: Duration::from_secs(DEFAULT_POLL_SECS),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };
        assert_eq!(
            cfg.state_url().expect("explicit passthrough"),
            "http://127.0.0.1:9999/state"
        );
    }

    #[test]
    fn env_secs_rejects_zero_and_nonnumeric() {
        // a zero or bad override must not disable the timeout / spin the loop.
        std::env::set_var("OBS_TEST_SECS_BAD", "0");
        assert_eq!(env_secs("OBS_TEST_SECS_BAD", 2), Duration::from_secs(2));
        std::env::set_var("OBS_TEST_SECS_BAD", "abc");
        assert_eq!(env_secs("OBS_TEST_SECS_BAD", 2), Duration::from_secs(2));
        std::env::set_var("OBS_TEST_SECS_BAD", "5");
        assert_eq!(env_secs("OBS_TEST_SECS_BAD", 2), Duration::from_secs(5));
        std::env::remove_var("OBS_TEST_SECS_BAD");
    }
}
