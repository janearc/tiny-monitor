// render turns a Snapshot (or the absence of one) into the glanceable view the
// window paints: a fleet-rollup colour, a short status line, per-service rows,
// and the token-runway readout. It holds no window state -- the widget is
// stateless per the architecture, so every tick rebuilds this view from the
// latest snapshot. The window layer reads RenderModel and does nothing else.

use crate::model::{Health, Snapshot};

// Rgb is a plain 8-bit colour; the window maps it to an NSColor. Kept here so
// the health -> colour decision is testable without AppKit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

// colour_for maps a health state to the swatch the widget paints. UNSPECIFIED
// and the unreachable "no data" state share a neutral grey -- "we do not know"
// is deliberately not green.
pub fn colour_for(h: Health) -> Rgb {
    match h {
        Health::Green => Rgb {
            r: 0x2e,
            g: 0xcc,
            b: 0x71,
        },
        Health::Yellow => Rgb {
            r: 0xf1,
            g: 0xc4,
            b: 0x0f,
        },
        Health::Red => Rgb {
            r: 0xe7,
            g: 0x4c,
            b: 0x3c,
        },
        Health::Exhausted => Rgb {
            r: 0x99,
            g: 0x2d,
            b: 0x22,
        },
        Health::Unspecified => Rgb {
            r: 0x7f,
            g: 0x8c,
            b: 0x8d,
        },
    }
}

// Row is one rendered line in the body (a service or a backup project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub health: Health,
    pub detail: String,
}

// RenderModel is the entire paintable state for one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderModel {
    // overall is the fleet rollup health driving the headline swatch.
    pub overall: Health,
    // headline is the one-line status ("FLEET GREEN", "NO DATA", ...).
    pub headline: String,
    // rows are the per-service / per-project lines under the headline.
    pub rows: Vec<Row>,
    // runway is the token-runway readout, or a "no quota feed" placeholder.
    pub runway: String,
    // reachable is false when the aggregator could not be polled this tick;
    // the window uses it to draw the degraded "no data" treatment.
    pub reachable: bool,
}

impl RenderModel {
    // unreachable is the degraded view drawn when GET /state failed this tick.
    // The widget shows it instead of crashing -- graceful degradation, per the
    // architecture. The error string is surfaced so a glance tells you why.
    pub fn unreachable(reason: &str) -> Self {
        RenderModel {
            overall: Health::Unspecified,
            headline: "NO DATA".to_string(),
            rows: vec![Row {
                label: "obs-svc-agg".to_string(),
                health: Health::Unspecified,
                detail: truncate(reason, 48),
            }],
            runway: "runway: --".to_string(),
            reachable: false,
        }
    }

    // from_snapshot builds the view from a successfully-fetched snapshot.
    //
    // The fleet headline prefers the aggregator's own `fleet.overall` rollup
    // when present; if the build does not emit it (backups-only snapshot), the
    // rollup is recomputed worst-wins over whatever per-service states exist,
    // and falls back to the daemon `healthy` flag when there are no services at
    // all. This way the widget shows something honest against both the current
    // and the future aggregator.
    pub fn from_snapshot(snap: &Snapshot) -> Self {
        let overall = fleet_overall(snap);

        let mut rows: Vec<Row> = Vec::new();
        for sh in snap.services.values() {
            rows.push(Row {
                label: sh.service.clone(),
                health: Health::parse(&sh.state),
                detail: format!("{} hb", sh.heartbeat_count),
            });
        }
        // With no per-service heartbeats, show backup projects so the widget is
        // not blank against today's aggregator. A failing last backup reads as
        // RED, otherwise GREEN -- a coarse but honest per-project signal.
        if rows.is_empty() {
            for bs in snap.backups.values() {
                let health = if bs.total > 0 && !bs.last_success {
                    Health::Red
                } else if bs.total > 0 {
                    Health::Green
                } else {
                    Health::Unspecified
                };
                rows.push(Row {
                    label: bs.project.clone(),
                    health,
                    detail: format!("{}ok/{}fail", bs.successes, bs.failures),
                });
            }
        }

        let headline = if rows.is_empty() {
            format!("FLEET {} (no events)", overall.label())
        } else {
            format!("FLEET {}", overall.label())
        };

        RenderModel {
            overall,
            headline,
            rows,
            runway: runway_line(snap),
            reachable: true,
        }
    }
}

// fleet_overall resolves the headline health, preferring the aggregator rollup.
fn fleet_overall(snap: &Snapshot) -> Health {
    if let Some(fleet) = &snap.fleet {
        if !fleet.overall.is_empty() {
            return Health::parse(&fleet.overall);
        }
    }
    if !snap.services.is_empty() {
        let mut worst = Health::Unspecified;
        for sh in snap.services.values() {
            let h = Health::parse(&sh.state);
            if h.severity() > worst.severity() {
                worst = h;
            }
        }
        return worst;
    }
    // No service health at all: lean on the daemon readiness flag so the widget
    // distinguishes "daemon up, no fleet heartbeats yet" from genuine trouble.
    if snap.healthy {
        Health::Green
    } else {
        Health::Unspecified
    }
}

// runway_line renders QuotaMetrics, or a placeholder until the feed exists.
fn runway_line(snap: &Snapshot) -> String {
    match &snap.quota {
        Some(q) => format!(
            "runway: {}m  burn: {}/m",
            q.runway_minutes_remaining, q.burn_rate_tokens_per_minute
        ),
        None => "runway: -- (no quota feed)".to_string(),
    }
}

// truncate clips a string to n chars with an ellipsis, so a long error does not
// blow out the fixed-size window.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BackupStat, FleetHealth, QuotaMetrics, ServiceHealth, Snapshot};

    // disingenerosity: a snapshot that lies by omission (partial fields) must
    // still render an honest, non-green default rather than a flattering blank.

    fn svc(name: &str, state: &str) -> (String, ServiceHealth) {
        (
            name.to_string(),
            ServiceHealth {
                service: name.to_string(),
                state: state.to_string(),
                heartbeat_count: 5,
            },
        )
    }

    #[test]
    fn health_parse_round_trips_known_states() {
        for s in ["GREEN", "YELLOW", "RED", "EXHAUSTED", "UNSPECIFIED"] {
            assert_eq!(Health::parse(s).label(), s);
        }
    }

    #[test]
    fn health_parse_unknown_is_unspecified() {
        assert_eq!(Health::parse("HEALTH_STATE_GREEN"), Health::Unspecified);
        assert_eq!(Health::parse(""), Health::Unspecified);
        assert_eq!(Health::parse("garbage"), Health::Unspecified);
    }

    #[test]
    fn colour_unknown_is_not_green() {
        // "we do not know" must never paint as healthy.
        assert_ne!(colour_for(Health::Unspecified), colour_for(Health::Green));
    }

    #[test]
    fn each_state_has_a_distinct_colour() {
        let states = [
            Health::Green,
            Health::Yellow,
            Health::Red,
            Health::Exhausted,
            Health::Unspecified,
        ];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(colour_for(*a), colour_for(*b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn severity_orders_exhausted_above_red() {
        assert!(Health::Exhausted.severity() > Health::Red.severity());
        assert!(Health::Red.severity() > Health::Yellow.severity());
        assert!(Health::Yellow.severity() > Health::Green.severity());
        assert!(Health::Green.severity() > Health::Unspecified.severity());
    }

    #[test]
    fn fleet_rollup_prefers_aggregator_value() {
        let mut snap = Snapshot::default();
        snap.services.extend([svc("a", "GREEN")]);
        snap.fleet = Some(FleetHealth {
            overall: "RED".to_string(),
            ..Default::default()
        });
        // explicit rollup wins over the recomputed per-service worst-wins.
        assert_eq!(fleet_overall(&snap), Health::Red);
    }

    #[test]
    fn fleet_rollup_recomputes_worst_wins_when_absent() {
        let mut snap = Snapshot::default();
        snap.services
            .extend([svc("a", "GREEN"), svc("b", "YELLOW"), svc("c", "GREEN")]);
        assert_eq!(fleet_overall(&snap), Health::Yellow);
    }

    #[test]
    fn fleet_rollup_falls_back_to_healthy_flag() {
        // backups-only snapshot (today's aggregator): no services, no fleet.
        let snap = Snapshot {
            healthy: true,
            ..Default::default()
        };
        assert_eq!(fleet_overall(&snap), Health::Green);
        let snap = Snapshot {
            healthy: false,
            ..Default::default()
        };
        assert_eq!(fleet_overall(&snap), Health::Unspecified);
    }

    #[test]
    fn render_from_services_lists_each_service() {
        let mut snap = Snapshot::default();
        snap.services
            .extend([svc("paling", "GREEN"), svc("delightd", "RED")]);
        let m = RenderModel::from_snapshot(&snap);
        assert!(m.reachable);
        assert_eq!(m.rows.len(), 2);
        // BTreeMap iteration is sorted: delightd before paling.
        assert_eq!(m.rows[0].label, "delightd");
        assert_eq!(m.rows[0].health, Health::Red);
    }

    #[test]
    fn render_falls_back_to_backups_when_no_services() {
        // mirrors the live aggregator snapshot shape.
        let mut snap = Snapshot {
            healthy: true,
            total_events: 83,
            ..Default::default()
        };
        snap.backups.insert(
            "comfyui".to_string(),
            BackupStat {
                project: "comfyui".to_string(),
                total: 37,
                successes: 37,
                failures: 0,
                last_success: true,
            },
        );
        snap.backups.insert(
            "broken".to_string(),
            BackupStat {
                project: "broken".to_string(),
                total: 2,
                successes: 1,
                failures: 1,
                last_success: false,
            },
        );
        let m = RenderModel::from_snapshot(&snap);
        assert!(m.reachable);
        assert_eq!(m.rows.len(), 2);
        let broken = m.rows.iter().find(|r| r.label == "broken").unwrap();
        assert_eq!(broken.health, Health::Red);
        let ok = m.rows.iter().find(|r| r.label == "comfyui").unwrap();
        assert_eq!(ok.health, Health::Green);
    }

    #[test]
    fn render_runway_placeholder_without_quota() {
        let snap = Snapshot::default();
        let m = RenderModel::from_snapshot(&snap);
        assert!(m.runway.contains("no quota feed"));
    }

    #[test]
    fn render_runway_from_quota() {
        let snap = Snapshot {
            quota: Some(QuotaMetrics {
                runway_minutes_remaining: 120,
                burn_rate_tokens_per_minute: 4500,
                ..Default::default()
            }),
            ..Default::default()
        };
        let m = RenderModel::from_snapshot(&snap);
        assert!(m.runway.contains("120m"));
        assert!(m.runway.contains("4500/m"));
    }

    #[test]
    fn unreachable_view_is_degraded_not_green() {
        let m = RenderModel::unreachable("connection refused");
        assert!(!m.reachable);
        assert_eq!(m.overall, Health::Unspecified);
        assert_eq!(m.headline, "NO DATA");
        assert!(m.rows[0].detail.contains("connection refused"));
    }

    #[test]
    fn truncate_clips_long_strings() {
        let long = "x".repeat(100);
        let out = truncate(&long, 48);
        assert_eq!(out.chars().count(), 48);
        assert!(out.ends_with('…'));
    }
}
