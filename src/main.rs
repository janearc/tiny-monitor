// tiny-monitor is the frameless, always-on-top macOS floating widget. It polls
// obs-svc-agg's GET /state at the architecture's ~2s cadence and re-renders the
// latest snapshot: a fleet-rollup colour swatch, per-service / per-project
// health rows, and the token-runway readout. It is stateless -- each tick paints
// the latest snapshot -- and degrades visibly to a "NO DATA" view when the
// aggregator is unreachable rather than crashing or showing stale state.
//
// All the logic (fetch, parse, health -> colour, the glance view) lives in the
// tiny_monitor library and is unit-tested headlessly. This file is the NSWindow
// shell: it cannot be unit-tested without a display, so it is kept thin.

// On non-macOS the widget has no window to draw; print the resolved view to
// stdout once so the binary still builds and runs everywhere (CI, a Linux box).
#[cfg(not(target_os = "macos"))]
fn main() {
    use tiny_monitor::{fetch, render::RenderModel};
    let cfg = fetch::Config::from_env();
    eprintln!(
        "tiny-monitor: no native window on this platform; polling {} once",
        cfg.state_url
    );
    let model = match fetch::fetch_snapshot(&cfg) {
        Ok(snap) => RenderModel::from_snapshot(&snap),
        Err(e) => RenderModel::unreachable(&e),
    };
    println!("{}", model.headline);
    for row in &model.rows {
        println!("  [{}] {}  {}", row.health.label(), row.label, row.detail);
    }
    println!("{}", model.runway);
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::rc::Rc;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont,
        NSTextField, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowLevel,
        NSWindowStyleMask,
    };
    use objc2_foundation::{
        ns_string, CGFloat, MainThreadMarker, NSDefaultRunLoopMode, NSPoint, NSRect, NSRunLoop,
        NSSize, NSString, NSTimer,
    };

    use tiny_monitor::fetch::{self, Config};
    use tiny_monitor::render::{colour_for, RenderModel, Rgb};

    // window geometry: small and glanceable, like the Activity Monitor floater.
    const WIDTH: CGFloat = 300.0;
    const HEIGHT: CGFloat = 200.0;
    const PAD: CGFloat = 12.0;
    const LINE: CGFloat = 18.0;
    // MAX_ROWS bounds the fixed label set; extra services collapse into a
    // summary line so the window stays a fixed size (no per-tick reflow).
    const MAX_ROWS: usize = 6;

    // NSWindowLevelFloating == 3 (CGWindowLevelForKey(kCGFloatingWindowLevelKey)),
    // the level the architecture record names for the widget. AppKit exposes the
    // level as a typed wrapper over an isize; this is its documented value.
    const FLOATING_WINDOW_LEVEL: NSWindowLevel = 3;

    // run builds the floating window and drives the poll/render loop.
    pub fn run() {
        let mtm = MainThreadMarker::new().expect("must run on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        // accessory policy: a floating utility, no Dock icon, no menu-bar focus.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let window = build_window(mtm);
        let labels = Rc::new(build_labels(mtm, &window));

        // the widget is stateless; we keep only the config and the label handles,
        // so each tick overwrites the view from the freshly-fetched snapshot.
        let cfg = Rc::new(Config::from_env());

        // paint once immediately so the window is never blank before the first
        // tick, then install the recurring poll on the main run loop.
        tick(&cfg, &labels);
        install_timer(cfg, labels);

        // SAFETY: called once, on the main thread, after setup -- the standard
        // AppKit entry point. It blocks until the app terminates.
        unsafe { app.run() };
    }

    // build_window creates the frameless, borderless, always-on-top NSWindow.
    fn build_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
        let frame = NSRect::new(NSPoint::new(60.0, 60.0), NSSize::new(WIDTH, HEIGHT));
        // borderless == frameless: no title bar, no traffic lights.
        let style = NSWindowStyleMask::Borderless;
        // SAFETY: standard NSWindow designated initialiser on a fresh allocation.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                frame,
                style,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };

        unsafe {
            // NSWindowLevelFloating per the architecture record: above ordinary
            // windows.
            window.setLevel(FLOATING_WINDOW_LEVEL);
            window.setOpaque(false);
            window.setBackgroundColor(Some(&panel_colour()));
            // join every Space and float over fullscreen apps, like a HUD.
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            window.setMovableByWindowBackground(true);
            window.makeKeyAndOrderFront(None);
        }
        window
    }

    // Labels holds the NSTextField handles the tick loop rewrites each cycle.
    struct Labels {
        swatch: Retained<NSView>,
        headline: Retained<NSTextField>,
        rows: Vec<Retained<NSTextField>>,
        runway: Retained<NSTextField>,
    }

    // build_labels lays out the static view tree once; the loop only mutates the
    // text and colours, never the tree -- cheap, flicker-free re-renders.
    fn build_labels(mtm: MainThreadMarker, window: &NSWindow) -> Labels {
        let content = window.contentView().expect("content view");

        // the rollup colour swatch: a thin layer-backed bar across the top.
        let swatch = unsafe {
            let rect = NSRect::new(NSPoint::new(0.0, HEIGHT - 6.0), NSSize::new(WIDTH, 6.0));
            let v = NSView::initWithFrame(mtm.alloc(), rect);
            v.setWantsLayer(true);
            content.addSubview(&v);
            v
        };

        let mut y = HEIGHT - 6.0 - PAD - LINE;
        let headline = make_label(mtm, &content, y, 14.0, true);
        y -= LINE + 4.0;

        let mut rows = Vec::with_capacity(MAX_ROWS);
        for _ in 0..MAX_ROWS {
            rows.push(make_label(mtm, &content, y, 12.0, false));
            y -= LINE;
        }

        let runway = make_label(mtm, &content, PAD, 12.0, true);

        Labels {
            swatch,
            headline,
            rows,
            runway,
        }
    }

    // make_label builds one borderless, transparent, non-editable text field.
    fn make_label(
        mtm: MainThreadMarker,
        parent: &NSView,
        y: CGFloat,
        size: CGFloat,
        bold: bool,
    ) -> Retained<NSTextField> {
        unsafe {
            let rect = NSRect::new(NSPoint::new(PAD, y), NSSize::new(WIDTH - 2.0 * PAD, LINE));
            let label = NSTextField::initWithFrame(mtm.alloc(), rect);
            label.setBezeled(false);
            label.setDrawsBackground(false);
            label.setEditable(false);
            label.setSelectable(false);
            label.setStringValue(ns_string!(""));
            label.setTextColor(Some(&NSColor::whiteColor()));
            // bold for the headline/runway, monospaced for the rows so the
            // bullet + name + detail columns line up at a glance.
            let font = if bold {
                NSFont::boldSystemFontOfSize(size)
            } else {
                NSFont::monospacedSystemFontOfSize_weight(size, 0.0)
            };
            label.setFont(Some(&font));
            parent.addSubview(&label);
            label
        }
    }

    // install_timer installs a repeating main-run-loop timer that re-ticks at the
    // configured cadence. AppKit must be touched only on the main thread, so the
    // cadence runs as a run-loop timer rather than a background thread.
    fn install_timer(cfg: Rc<Config>, labels: Rc<Labels>) {
        let interval = cfg.poll.as_secs_f64();
        // the block re-renders on every fire; it owns clones of the shared state.
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            autoreleasepool(|_| tick(&cfg, &labels));
        });

        unsafe {
            let timer = NSTimer::timerWithTimeInterval_repeats_block(interval, true, &block);
            let run_loop = NSRunLoop::currentRunLoop();
            run_loop.addTimer_forMode(&timer, NSDefaultRunLoopMode);
        }
    }

    // tick performs one poll + re-render. A fetch failure renders the degraded
    // view; it never propagates an error that would tear down the window.
    fn tick(cfg: &Config, labels: &Labels) {
        let model = match fetch::fetch_snapshot(cfg) {
            Ok(snap) => RenderModel::from_snapshot(&snap),
            Err(e) => RenderModel::unreachable(&e),
        };
        apply(&model, labels);
    }

    // apply writes a RenderModel onto the existing labels and swatch.
    fn apply(model: &RenderModel, labels: &Labels) {
        unsafe {
            set_layer_colour(&labels.swatch, colour_for(model.overall));
            labels
                .headline
                .setStringValue(&NSString::from_str(&model.headline));

            for (i, label) in labels.rows.iter().enumerate() {
                if i + 1 == MAX_ROWS && model.rows.len() > MAX_ROWS {
                    // last slot summarises the overflow rather than clipping.
                    let extra = model.rows.len() - (MAX_ROWS - 1);
                    label.setStringValue(&NSString::from_str(&format!("… +{extra} more")));
                    label.setTextColor(Some(&NSColor::whiteColor()));
                    continue;
                }
                match model.rows.get(i) {
                    Some(row) => {
                        let text = format!("● {}  {}", row.label, row.detail);
                        label.setStringValue(&NSString::from_str(&text));
                        label.setTextColor(Some(&ns_colour(colour_for(row.health))));
                    }
                    None => label.setStringValue(ns_string!("")),
                }
            }

            // a reachable daemon paints the runway white; an unreachable one
            // tints it with the degraded (grey) colour to reinforce "no data".
            let runway_colour = if model.reachable {
                NSColor::whiteColor()
            } else {
                ns_colour(colour_for(model.overall))
            };
            labels
                .runway
                .setStringValue(&NSString::from_str(&model.runway));
            labels.runway.setTextColor(Some(&runway_colour));
        }
    }

    // panel_colour is the translucent dark window background.
    fn panel_colour() -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.07, 0.07, 0.09, 0.92) }
    }

    // ns_colour maps an Rgb to an opaque NSColor for text.
    fn ns_colour(c: Rgb) -> Retained<NSColor> {
        unsafe {
            NSColor::colorWithSRGBRed_green_blue_alpha(
                c.r as CGFloat / 255.0,
                c.g as CGFloat / 255.0,
                c.b as CGFloat / 255.0,
                1.0,
            )
        }
    }

    // set_layer_colour paints a layer-backed view a solid colour (the swatch).
    // NSColor has no typed CGColor binding here, and CALayer.setBackgroundColor
    // wants a CGColorRef, so both hops go through raw message sends.
    fn set_layer_colour(view: &NSView, c: Rgb) {
        unsafe {
            let colour = ns_colour(c);
            let cg: *mut objc2::runtime::AnyObject = msg_send![&*colour, CGColor];
            let layer: *mut objc2::runtime::AnyObject = msg_send![view, layer];
            if !layer.is_null() {
                let _: () = msg_send![layer, setBackgroundColor: cg];
            }
        }
    }
}
