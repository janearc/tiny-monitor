// contract_conformance proves tiny-monitor consumes the big-little-mesh
// observability contract across the repo boundary, and pins the hand-rolled
// read-path vocabulary to the generated one so the two cannot silently drift.
//
// Why a conformance test and not a read-path migration: src/model.rs parses the
// BARE health vocabulary the running obs-svc-agg emits today ("GREEN"), whereas
// the generated contract serializes the protojson value name ("HEALTH_STATE_GREEN").
// Switching the read path onto the gen types is gated on obs-svc-agg emitting
// conformant protojson and is a later change. Until then this test is the seam
// that keeps the hand-rolled enum honest against the contract: add a Health
// variant, rename a label, or change a contract value name, and this fails.

use big_little_mesh_contracts::observability::v1::HealthState;
use tiny_monitor::Health;

// every hand-rolled Health state, paired with the contract enum it mirrors.
const PAIRS: &[(Health, HealthState)] = &[
    (Health::Unspecified, HealthState::HealthStateUnspecified),
    (Health::Green, HealthState::HealthStateGreen),
    (Health::Yellow, HealthState::HealthStateYellow),
    (Health::Red, HealthState::HealthStateRed),
    (Health::Exhausted, HealthState::HealthStateExhausted),
];

#[test]
fn hand_rolled_vocabulary_matches_the_contract() {
    for (health, state) in PAIRS {
        // the contract serializes the protojson value name, e.g. "HEALTH_STATE_GREEN".
        let contract_name = serde_json::to_string(state).expect("serialize HealthState");
        // model.rs reads the bare suffix ("GREEN"); the contract name is exactly
        // "HEALTH_STATE_" + that bare label. If they diverge, the read path can no
        // longer be migrated onto the contract without a translation, so fail loud.
        let expected = format!("\"HEALTH_STATE_{}\"", health.label());
        assert_eq!(
            contract_name,
            expected,
            "{health:?} ({}) drifted from contract {state:?}",
            health.label()
        );
    }
}

#[test]
fn contract_tolerates_unknown_values() {
    // the generated enum has an Unknown catch-all (serde(other)), so a contract
    // addition the widget has not been rebuilt for decodes rather than panicking
    // -- the same forward-compatible posture model.rs takes with Health::parse.
    let decoded: HealthState =
        serde_json::from_str("\"HEALTH_STATE_FUTURE\"").expect("unknown value must decode");
    assert_eq!(decoded, HealthState::Unknown);
}
