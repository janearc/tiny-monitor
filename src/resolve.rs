// resolve asks delightd where a service answers, so the widget targets obs-svc-agg by NAME --
// resolved through the one well-known control plane -- instead of carrying its address as a
// hardcoded constant. The widget calls delightd's GET /resolve/{name}, which returns a
// resolve.v1.ResolvedService (scheme + address) composed from delightd's live registry. delightd
// is the single endpoint the widget has to know; everything else is resolved through it.
//
// This module is the resolution seam only: it turns a service name into the /state URL to poll.
// What the widget does when resolution FAILS (delightd down, or the service not yet registered)
// is a policy the caller owns -- this code never silently substitutes a hardcoded address.

use std::time::Duration;

use delightd_contracts::resolve::v1::ResolvedService;

// DEFAULT_DELIGHTD_URL is delightd's canonical control port on this host: compose publishes
// 127.0.0.1:8088 and the kube Deployment uses containerPort 8088 (delightd's DefaultControlPort).
// Overridable via DELIGHTD_URL for any other mapping or a real service address.
pub const DEFAULT_DELIGHTD_URL: &str = "http://127.0.0.1:8088";

// delightd_base_from_env resolves delightd's control-port base URL, defaulting to the canonical
// host port. Trailing slashes are tolerated; resolve_state_url trims them.
pub fn delightd_base_from_env() -> String {
    std::env::var("DELIGHTD_URL").unwrap_or_else(|_| DEFAULT_DELIGHTD_URL.to_string())
}

// resolve_state_url asks delightd for `service`'s address and returns the /state URL to poll.
// `delightd_base` is delightd's control-port base; `service` is the registry name to resolve
// (e.g. "obs-svc-agg"). A resolution miss (404 -- delightd holds no registration for the name)
// and an unreachable delightd are distinct Err strings so the caller can tell "not registered
// yet" from "delightd is down"; neither is silently turned into a default address here.
pub fn resolve_state_url(
    delightd_base: &str,
    service: &str,
    timeout: Duration,
) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let url = format!(
        "{}/resolve/{}",
        delightd_base.trim_end_matches('/'),
        service
    );
    let body = match agent.get(&url).call() {
        Ok(resp) => resp
            .into_string()
            .map_err(|e| format!("read resolve body: {e}"))?,
        Err(ureq::Error::Status(404, _)) => {
            return Err(format!(
                "delightd holds no registration for {service:?} (not resolvable)"
            ));
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("delightd /resolve returned HTTP {code}"));
        }
        Err(e) => return Err(format!("delightd unreachable: {e}")),
    };
    let resolved: ResolvedService = serde_json::from_str(&body)
        .map_err(|e| format!("parse resolve.v1.ResolvedService: {e}"))?;
    state_url_from(&resolved)
}

// state_url_from builds the aggregator's /state URL from a resolved service. Pulled out of the
// network path so it is unit-tested against a literal ResolvedService with no live delightd. An
// empty scheme or address is an Err -- delightd resolved something the widget cannot poll -- so
// the poll loop fails once here with a clear reason instead of silently erroring every tick on a
// malformed URL.
pub fn state_url_from(resolved: &ResolvedService) -> Result<String, String> {
    if resolved.scheme.is_empty() || resolved.address.is_empty() {
        return Err(format!(
            "delightd resolved {:?} to an incomplete endpoint (scheme={:?}, address={:?})",
            resolved.name, resolved.scheme, resolved.address
        ));
    }
    Ok(format!("{}://{}/state", resolved.scheme, resolved.address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_state_url_from_resolved() {
        let r = ResolvedService {
            name: "obs-svc-agg".into(),
            scheme: "http".into(),
            address: "obs-svc-agg.fleet:8090".into(),
        };
        assert_eq!(
            state_url_from(&r).expect("complete endpoint"),
            "http://obs-svc-agg.fleet:8090/state"
        );
    }

    #[test]
    fn deserializes_resolvedservice_from_delightd_protojson() {
        // a literal GET /resolve/{name} body exactly as delightd serves it (bare protojson,
        // proto field names). This pins the widget against the wire shape the contract emits.
        let body = r#"{"name":"obs-svc-agg","scheme":"http","address":"obs-svc-agg.fleet:8090"}"#;
        let r: ResolvedService = serde_json::from_str(body).expect("decode ResolvedService");
        assert_eq!(r.address, "obs-svc-agg.fleet:8090");
        assert_eq!(
            state_url_from(&r).expect("complete endpoint"),
            "http://obs-svc-agg.fleet:8090/state"
        );
    }

    #[test]
    fn incomplete_endpoint_is_error_not_malformed_url() {
        // delightd returning an empty endpoint must fail loudly here, not produce "://　/state".
        let r = ResolvedService {
            name: "obs-svc-agg".into(),
            scheme: String::new(),
            address: String::new(),
        };
        assert!(state_url_from(&r).is_err());
    }
}
