// smoke is a headless end-to-end check of the data path: resolve obs-svc-agg (through delightd
// by default, or an explicit override), poll its /state, and print the derived glance view. It
// proves resolve+fetch+parse+render without opening the window. Run against a live delightd:
//   cargo run --example smoke
// or bypass resolution with an explicit aggregator:
//   OBS_AGG_URL=http://127.0.0.1:<port>/state cargo run --example smoke
use tiny_monitor::{fetch, render::RenderModel};
fn main() {
    let cfg = fetch::Config::from_env();
    println!("source: {:?}", cfg.source);
    let model = match fetch::fetch_snapshot(&cfg) {
        Ok(s) => RenderModel::from_snapshot(&s),
        Err(e) => RenderModel::unreachable(&e),
    };
    println!("[{}] {}", model.overall.label(), model.headline);
    for r in &model.rows {
        println!("  ({}) {}  {}", r.health.label(), r.label, r.detail);
    }
    println!("{}", model.runway);
    println!("reachable={}", model.reachable);
}
