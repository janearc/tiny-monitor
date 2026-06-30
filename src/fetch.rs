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

// Config is the widget's runtime configuration, resolved from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    // state_url is the full URL of the aggregator's /state endpoint.
    pub state_url: String,
    // poll is the tick interval. The architecture's widget cadence is 2s.
    pub poll: Duration,
    // timeout bounds a single poll so a hung aggregator cannot stall the tick.
    pub timeout: Duration,
}

impl Config {
    // from_env resolves configuration, defaulting to the obs-svc-agg address
    // reachable on this host. Endpoints are not hardcoded into the widget: the
    // URL and cadence are overridable, per the no-hardcoded-endpoints rule.
    //
    // The default targets the running obs-svc-agg container's published host
    // port. The compose-internal port is 8090; the container currently maps it
    // to a host port, so the default is overridable via OBS_AGG_URL for any
    // mapping (and points at Traefik / a service address in a real deploy).
    pub fn from_env() -> Self {
        let state_url =
            std::env::var("OBS_AGG_URL").unwrap_or_else(|_| DEFAULT_STATE_URL.to_string());
        let poll = env_secs("OBS_POLL_SECS", DEFAULT_POLL_SECS);
        let timeout = env_secs("OBS_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS);
        Config {
            state_url,
            poll,
            timeout,
        }
    }
}

// DEFAULT_STATE_URL is the obs-svc-agg /state endpoint. Overridable via
// OBS_AGG_URL; this default is the host-published port of the running container.
pub const DEFAULT_STATE_URL: &str = "http://127.0.0.1:8090/state";
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
    let agent = ureq::AgentBuilder::new().timeout(cfg.timeout).build();
    let body = match agent.get(&cfg.state_url).call() {
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
    fn config_defaults_when_env_absent() {
        // guard the defaults without mutating process env (parallel-test safe).
        let cfg = Config {
            state_url: DEFAULT_STATE_URL.to_string(),
            poll: Duration::from_secs(DEFAULT_POLL_SECS),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };
        assert_eq!(cfg.state_url, "http://127.0.0.1:8090/state");
        assert_eq!(cfg.poll, Duration::from_secs(2));
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
