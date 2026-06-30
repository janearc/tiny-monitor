// tiny_monitor is the logic core of the floating widget: the /state read, the
// snapshot model, and the health -> colour / glance-view derivation. It carries
// no AppKit dependency so it builds and unit-tests on any host. The binary
// (src/main.rs) is the macOS NSWindow shell that consumes this crate.

pub mod fetch;
pub mod model;
pub mod render;

pub use fetch::{fetch_snapshot, parse_snapshot, Config};
pub use model::{Health, Snapshot};
pub use render::{colour_for, RenderModel, Rgb, Row};
