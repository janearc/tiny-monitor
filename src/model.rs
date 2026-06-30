// model mirrors obs-svc-agg's GET /state JSON snapshot (pkg/agg.Snapshot).
//
// The running aggregator does not always emit every field: a build that has
// only folded delight.v1 BackupEvents emits `healthy`/`total_events`/`backups`
// and omits `services`/`fleet` entirely. Every field here is therefore
// optional, so the widget degrades to "what the daemon actually sent" rather
// than failing to parse a valid-but-partial snapshot. This is the forward/
// backward-compatible read the architecture's idempotent-telemetry rule wants.

use serde::Deserialize;

// Health mirrors observability.v1.HealthState. The aggregator serialises the
// state-machine output as the bare vocabulary ("GREEN", not
// "HEALTH_STATE_GREEN"); we keep an Unspecified fallback for any unknown or
// missing value so a contract addition never panics the widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Unspecified,
    Green,
    Yellow,
    Red,
    Exhausted,
}

impl Health {
    // parse maps the aggregator's string state to a Health. Unknown or empty
    // input is Unspecified, never an error -- a missing rollup is data, not a
    // parse failure.
    pub fn parse(s: &str) -> Self {
        match s {
            "GREEN" => Health::Green,
            "YELLOW" => Health::Yellow,
            "RED" => Health::Red,
            "EXHAUSTED" => Health::Exhausted,
            _ => Health::Unspecified,
        }
    }

    // severity orders states worst-wins for a rollup, matching the aggregator's
    // own ordering: EXHAUSTED (terminal) outranks RED; UNSPECIFIED is the floor.
    pub fn severity(self) -> u8 {
        match self {
            Health::Green => 1,
            Health::Yellow => 2,
            Health::Red => 3,
            Health::Exhausted => 4,
            Health::Unspecified => 0,
        }
    }

    // label is the short glance text for the state.
    pub fn label(self) -> &'static str {
        match self {
            Health::Unspecified => "UNSPECIFIED",
            Health::Green => "GREEN",
            Health::Yellow => "YELLOW",
            Health::Red => "RED",
            Health::Exhausted => "EXHAUSTED",
        }
    }
}

// Snapshot is the deserialised /state body.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Snapshot {
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub total_events: u64,
    #[serde(default)]
    pub backups: std::collections::BTreeMap<String, BackupStat>,
    // services/fleet are absent in a backups-only aggregator build.
    #[serde(default)]
    pub services: std::collections::BTreeMap<String, ServiceHealth>,
    #[serde(default)]
    pub fleet: Option<FleetHealth>,
    // quota is the architecture's QuotaMetrics -> token runway. Not yet emitted
    // by the aggregator; modelled so the widget renders it the moment it lands
    // rather than needing a contract change first.
    #[serde(default)]
    pub quota: Option<QuotaMetrics>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BackupStat {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub successes: u64,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub last_success: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServiceHealth {
    #[serde(default)]
    pub service: String,
    // state is the debounced state-machine output (post-hysteresis).
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub heartbeat_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FleetHealth {
    #[serde(default)]
    pub overall: String,
    #[serde(default)]
    pub active_nodes: u64,
    #[serde(default)]
    pub degraded_nodes: u64,
    #[serde(default)]
    pub exhausted_nodes: u64,
}

// QuotaMetrics mirrors observability.v1.QuotaMetrics (the token-runway readout).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct QuotaMetrics {
    #[serde(default)]
    pub runway_state: String,
    #[serde(default)]
    pub runway_minutes_remaining: u64,
    #[serde(default)]
    pub burn_rate_tokens_per_minute: u64,
    #[serde(default)]
    pub absolute_quota_remaining_cents: u64,
}
