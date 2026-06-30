// smoke is a headless end-to-end check of the data path: poll a live /state and
// print the derived glance view. Used to prove fetch+parse+render against the
// running obs-svc-agg without opening the window. Run:
//   OBS_AGG_URL=http://127.0.0.1:<port>/state cargo run --example smoke
use tiny_monitor::{fetch, render::RenderModel};
fn main() {
    let cfg = fetch::Config::from_env();
    println!("polling {}", cfg.state_url);
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
