//! Deterministic randomized tests for the input state machine and the
//! coordinate mapping. These act as a lightweight fuzzing harness: they are
//! reproducible, run in normal `cargo test`, and assert the safety invariants
//! that must hold for every pointer sequence.

use snipunix_core::{
    CaptureMapping, ImageRect, ImageSize, LogicalPoint, LogicalRect, MIN_SELECTION_SIZE,
    MonitorImage, Selection, SelectionPhase,
};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn coordinate(&mut self, max: f64) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0 * max
    }

    #[allow(dead_code)]
    fn unit(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
}

/// Tolerance for edge-escaping checks: a few ULPs at the scale of typical
/// desktop coordinates (≤ 8K). `f64` has ~15 significant digits; 4K×4K
/// coordinates fit well within that, but `x + width` may drift from `right`
/// by a couple of ULPs (≈ coordinate × 2^-52). 1 × 10^-9 is 1 000× more
/// tolerant than the worst-case ULP at 32 768-pixel scale, so it admits
/// harmless rounding while still catching real out-of-bounds bugs.
const EDGE_TOL: f64 = 1e-9;

#[test]
fn randomized_pointer_sequences_keep_the_selection_inside_the_desktop() {
    let desktop = LogicalRect::new(-1280.0, -200.0, 3200.0, 1440.0);
    let mut random = Lcg(0x5eed_1234_abcd_0001);
    let mut selection = Selection::new(desktop);

    for _ in 0..25_000 {
        let point = LogicalPoint::new(
            random.coordinate(desktop.width + 2000.0) - 1500.0,
            random.coordinate(desktop.height + 2000.0) - 1200.0,
        );
        match random.next() % 6 {
            0 => {
                selection.press(point);
            }
            1 => {
                selection.drag(point);
            }
            2 => {
                selection.release(point);
            }
            3 => {
                selection.nudge(
                    random.coordinate(40.0) - 20.0,
                    random.coordinate(40.0) - 20.0,
                );
            }
            4 => {
                selection.clear();
            }
            _ => {
                let other = LogicalPoint::new(
                    point.x + random.coordinate(500.0) - 250.0,
                    point.y + random.coordinate(500.0) - 250.0,
                );
                selection.select_rect(LogicalRect::from_corners(point, other));
            }
        }

        if let Some(rect) = selection.rect() {
            assert!(
                rect.x >= desktop.x - EDGE_TOL
                    && rect.y >= desktop.y - EDGE_TOL
                    && rect.right() <= desktop.right() + EDGE_TOL
                    && rect.bottom() <= desktop.bottom() + EDGE_TOL,
                "selection escaped the desktop: {rect:?}"
            );
            assert!(rect.width.is_finite() && rect.height.is_finite());
            assert!(rect.width >= 0.0 && rect.height >= 0.0);
            if matches!(selection.phase(), SelectionPhase::Selected) {
                assert!(
                    rect.width >= MIN_SELECTION_SIZE - EDGE_TOL
                        && rect.height >= MIN_SELECTION_SIZE - EDGE_TOL,
                    "accepted selection is below the minimum: {rect:?}"
                );
            }
        }
    }
}

#[test]
fn randomized_mappings_never_escape_the_captured_image() {
    let desktop = LogicalRect::new(-500.0, -300.0, 4000.0, 2200.0);
    let image = ImageSize::new(6000, 3300);
    let mapping = CaptureMapping::new(desktop, image).expect("valid mapping");
    let mut random = Lcg(0xfeed_9876_5432_1111);

    for _ in 0..25_000 {
        let x = random.coordinate(desktop.width + 2000.0) - 1500.0;
        let y = random.coordinate(desktop.height + 2000.0) - 1300.0;
        let rect = LogicalRect::new(x, y, random.coordinate(2500.0), random.coordinate(2500.0));
        if let Some(mapped) = mapping.virtual_to_image(rect) {
            assert!(mapped.x <= image.width);
            assert!(mapped.y <= image.height);
            assert!(mapped.x.saturating_add(mapped.width) <= image.width);
            assert!(mapped.y.saturating_add(mapped.height) <= image.height);
            assert!(mapped.width > 0 && mapped.height > 0);
        }
    }
}

#[test]
fn randomized_per_monitor_mappings_never_escape_the_captured_image() {
    // Left monitor at 2x and right monitor at 1x, mirroring the mixed-DPI
    // layout used by the GUI's per-monitor mapping path.
    let desktop = LogicalRect::new(0.0, 0.0, 2240.0, 1080.0);
    let image = ImageSize::new(3200, 1080);
    let mapping = CaptureMapping::with_monitors(
        desktop,
        image,
        vec![
            MonitorImage {
                id: 0,
                logical: LogicalRect::new(0.0, 0.0, 960.0, 540.0),
                image: ImageRect::new(0, 0, 1920, 1080),
            },
            MonitorImage {
                id: 1,
                logical: LogicalRect::new(960.0, 0.0, 1280.0, 720.0),
                image: ImageRect::new(1920, 0, 1280, 720),
            },
        ],
    )
    .expect("valid per-monitor mapping");
    let mut random = Lcg(0x0ddc_0ffe_e123_4567);

    for _ in 0..25_000 {
        let x = random.coordinate(desktop.width + 2000.0) - 1500.0;
        let y = random.coordinate(desktop.height + 2000.0) - 1300.0;
        let rect = LogicalRect::new(x, y, random.coordinate(2500.0), random.coordinate(2500.0));
        if let Some(mapped) = mapping.virtual_to_image(rect) {
            assert!(mapped.x.saturating_add(mapped.width) <= image.width);
            assert!(mapped.y.saturating_add(mapped.height) <= image.height);
            assert!(mapped.width > 0 && mapped.height > 0);
        }
    }
}
