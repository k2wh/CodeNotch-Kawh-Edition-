//! Where the notch sits on screen.
//!
//! Kept separate from the Win32 code so the arithmetic — DPI scaling, edge
//! anchoring, keeping the window inside the work area — can be tested without a
//! desktop. The platform layer supplies a [`WorkArea`] measured from the OS and
//! applies the resulting [`Placement`] verbatim.

use crate::config::{Edge, HudMetrics};

/// A monitor's usable area in **physical** pixels, excluding the taskbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// DPI scale, e.g. 1.5 for 150%.
    pub scale: f64,
}

impl WorkArea {
    pub fn new(x: i32, y: i32, width: i32, height: i32, scale: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
            // A zero or negative scale would collapse the window to nothing.
            scale: if scale > 0.0 { scale } else { 1.0 },
        }
    }
}

/// A window rectangle in physical pixels, ready for `SetWindowPos`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Position a HUD of `logical_width` x `logical_height` against `edge`.
///
/// `offset` slides the window along the edge: 0.0 is left/top, 0.5 centred,
/// 1.0 right/bottom. `margin` is a logical-pixel gap from the edge.
///
/// The result is always clamped inside the work area when the window fits, so
/// growing the card downward can never push it under the taskbar or off-screen.
pub fn place(
    area: WorkArea,
    edge: Edge,
    offset: f32,
    margin: f64,
    logical_width: f64,
    logical_height: f64,
) -> Placement {
    let scale = area.scale;
    // Round rather than truncate: a half-pixel error is visible as a 1px seam
    // against the screen edge at fractional DPI scales.
    let width = (logical_width * scale).round() as i32;
    let height = (logical_height * scale).round() as i32;
    let margin = (margin * scale).round() as i32;
    let offset = offset.clamp(0.0, 1.0) as f64;

    // Slide along the free space on the cross axis.
    let slide = |extent: i32, size: i32| -> i32 {
        let free = (extent - size).max(0);
        (free as f64 * offset).round() as i32
    };

    let (x, y) = match edge {
        Edge::Top => (area.x + slide(area.width, width), area.y + margin),
        Edge::Bottom => (
            area.x + slide(area.width, width),
            area.y + area.height - height - margin,
        ),
        Edge::Left => (area.x + margin, area.y + slide(area.height, height)),
        Edge::Right => (
            area.x + area.width - width - margin,
            area.y + slide(area.height, height),
        ),
    };

    Placement {
        x: clamp_axis(x, width, area.x, area.width),
        y: clamp_axis(y, height, area.y, area.height),
        width,
        height,
    }
}

/// Like [`place`], but anchored on the strip rather than the whole window.
///
/// When a card longer than the strip opens, the window grows along the edge.
/// Sliding the grown window by `offset` (as `place` does) would move the strip
/// with it, and the ring under the pointer would slide away -- the pointer then
/// lands on a neighbouring ring, whose card opens, and the two flicker. So the
/// window starts where the resting strip starts and grows away from it, and
/// only shifts back when it would run off the work area.
///
/// Returns the placement and how far into the window the strip now starts,
/// in logical pixels (0 unless the window had to be pulled back on screen).
pub fn place_anchored(
    area: WorkArea,
    edge: Edge,
    offset: f32,
    margin: f64,
    logical_width: f64,
    logical_height: f64,
    strip_length: f64,
) -> (Placement, f64) {
    let base = place(area, edge, offset, margin, logical_width, logical_height);
    let scale = area.scale;
    let strip = (strip_length * scale).round() as i32;
    let offset = offset.clamp(0.0, 1.0) as f64;
    let slide =
        |extent: i32, size: i32| -> i32 { ((extent - size).max(0) as f64 * offset).round() as i32 };

    let (placement, shift_px) = if edge.is_horizontal() {
        let resting = area.x + slide(area.width, strip);
        let x = clamp_axis(resting, base.width, area.x, area.width);
        (Placement { x, ..base }, resting - x)
    } else {
        let resting = area.y + slide(area.height, strip);
        let y = clamp_axis(resting, base.height, area.y, area.height);
        (Placement { y, ..base }, resting - y)
    };
    (placement, shift_px.max(0) as f64 / scale)
}

/// Keep `start..start+size` inside `origin..origin+extent`.
///
/// A window larger than the work area is pinned to the origin, which keeps its
/// top-left corner reachable instead of centring the overflow on both sides.
fn clamp_axis(start: i32, size: i32, origin: i32, extent: i32) -> i32 {
    if size >= extent {
        return origin;
    }
    start.clamp(origin, origin + extent - size)
}

/// Longest the strip may grow along its edge before its list scrolls instead.
pub const MAX_STRIP_LENGTH: f64 = 1000.0;
/// Tallest the detail popover may grow.
pub const MAX_POPOVER_LENGTH: f64 = 560.0;

/// Logical size of the whole HUD window.
///
/// The window has to contain the strip and, while open, the popover beside it.
/// `measured` is what the webview actually laid out and wins outright when
/// present: only the webview knows whether a popover is really on screen and
/// how tall it came out. Without that, hovering the strip between two rings
/// would leave the window standing wide open around nothing.
///
/// The fallback is used until the first measurement arrives. Either way the
/// result is clamped so a not-yet-measured 0 or a runaway list can't produce a
/// silly window.
pub fn hud_extent(
    metrics: HudMetrics,
    edge: Edge,
    providers: usize,
    open: bool,
    measured: Option<(f64, f64)>,
) -> (f64, f64) {
    let (along, thickness) = metrics.strip_extent(providers);
    let open_depth = thickness + metrics.popover_gap + metrics.popover_size;

    let (fallback_w, fallback_h) = if edge.is_horizontal() {
        (along, if open { open_depth } else { thickness })
    } else {
        (if open { open_depth } else { thickness }, along)
    };

    let usable = |value: f64| value.is_finite() && value > 0.0;
    let (w, h) = match measured {
        Some((w, h)) if usable(w) && usable(h) => (w, h),
        _ => (fallback_w, fallback_h),
    };

    let max_along = MAX_STRIP_LENGTH.max(MAX_POPOVER_LENGTH);
    if edge.is_horizontal() {
        // On a top/bottom edge the popover's *height* sets the depth, and
        // `popover_size` is its width, so cap by the tallest card instead.
        (
            w.clamp(metrics.slot, max_along),
            h.clamp(
                thickness,
                thickness + metrics.popover_gap + MAX_POPOVER_LENGTH,
            ),
        )
    } else {
        (
            w.clamp(thickness, open_depth),
            h.clamp(metrics.slot, max_along),
        )
    }
}

/// Where the strip sits inside the window, as an offset from the window's
/// origin, in logical pixels.
///
/// The strip always hugs the screen edge; the popover occupies the rest of the
/// window on the inward side, so on a right or bottom edge the strip is pushed
/// to the far end of the window.
pub fn strip_offset(metrics: HudMetrics, edge: Edge, window: (f64, f64)) -> (f64, f64) {
    let (w, h) = window;
    match edge {
        Edge::Right => (w - metrics.strip_thickness, 0.0),
        Edge::Left => (0.0, 0.0),
        Edge::Bottom => (0.0, h - metrics.strip_thickness),
        Edge::Top => (0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, HudSize};

    /// 1920x1080 with a 40px taskbar at the bottom, at 100% scale.
    fn screen() -> WorkArea {
        WorkArea::new(0, 0, 1920, 1040, 1.0)
    }

    #[test]
    fn top_centre_is_the_default_resting_position() {
        let p = place(screen(), Edge::Top, 0.5, 0.0, 168.0, 32.0);
        assert_eq!(p.width, 168);
        assert_eq!(p.height, 32);
        assert_eq!(p.x, (1920 - 168) / 2);
        assert_eq!(p.y, 0);
    }

    #[test]
    fn margin_offsets_from_the_work_area_edge() {
        let p = place(screen(), Edge::Top, 0.5, 8.0, 168.0, 32.0);
        assert_eq!(p.y, 8);

        let p = place(screen(), Edge::Bottom, 0.5, 8.0, 168.0, 32.0);
        assert_eq!(p.y, 1040 - 32 - 8, "bottom edge measures from the taskbar");
    }

    #[test]
    fn expanding_downward_keeps_the_top_edge_pinned() {
        // The pill and the expanded card must share a top edge, so the card
        // appears to grow out of the pill rather than jumping.
        let collapsed = place(screen(), Edge::Top, 0.5, 0.0, 168.0, 32.0);
        let expanded = place(screen(), Edge::Top, 0.5, 0.0, 344.0, 420.0);
        assert_eq!(collapsed.y, expanded.y);
    }

    #[test]
    fn expanding_upward_keeps_the_bottom_edge_pinned() {
        let collapsed = place(screen(), Edge::Bottom, 0.5, 0.0, 168.0, 32.0);
        let expanded = place(screen(), Edge::Bottom, 0.5, 0.0, 344.0, 420.0);
        assert_eq!(
            collapsed.y + collapsed.height,
            expanded.y + expanded.height,
            "the card should grow up from the taskbar, not off the screen"
        );
    }

    #[test]
    fn a_top_edge_strip_is_deep_enough_for_a_ring_column() {
        // The ring, its percentage and the weekly bar stack across the strip
        // on a horizontal edge; a strip sized for a vertical one clips them.
        for size in [HudSize::Small, HudSize::Medium, HudSize::Large] {
            let vertical = Config {
                size,
                edge: Edge::Right,
                ..Config::default()
            };
            let horizontal = Config {
                size,
                edge: Edge::Top,
                ..Config::default()
            };
            let across = horizontal.metrics().strip_thickness;
            assert!(
                across >= horizontal.metrics().stack_depth(),
                "{size:?}: strip ({across}) must hold the column"
            );
            assert!(across > vertical.metrics().strip_thickness);
        }
    }

    #[test]
    fn a_growing_window_keeps_the_strip_still() {
        // Right edge, centred, a 300px strip. Opening a 560px card must not
        // move the strip on screen.
        let (rest, rest_shift) =
            place_anchored(screen(), Edge::Right, 0.5, 0.0, 92.0, 300.0, 300.0);
        let (open, open_shift) =
            place_anchored(screen(), Edge::Right, 0.5, 0.0, 462.0, 560.0, 300.0);
        assert_eq!(rest_shift, 0.0);
        assert_eq!(rest.y, (1040 - 300) / 2);
        assert_eq!(
            open.y as f64 + open_shift,
            rest.y as f64,
            "strip start on screen is unchanged"
        );
        assert_eq!(open.x + open.width, 1920, "still flush with the edge");
    }

    #[test]
    fn a_window_that_would_run_off_screen_is_pulled_back_and_reports_the_shift() {
        // Strip at the very bottom: the card has to grow upward instead.
        let (open, shift) = place_anchored(screen(), Edge::Right, 1.0, 0.0, 462.0, 560.0, 300.0);
        assert_eq!(open.y + open.height, 1040);
        assert_eq!(shift, (560 - 300) as f64);
    }

    #[test]
    fn anchoring_respects_the_display_scale() {
        let hidpi = WorkArea::new(0, 0, 2400, 1300, 1.25);
        let (rest, _) = place_anchored(hidpi, Edge::Left, 1.0, 0.0, 92.0, 300.0, 300.0);
        let (open, shift) = place_anchored(hidpi, Edge::Left, 1.0, 0.0, 462.0, 560.0, 300.0);
        // Logical shift times the scale lands the strip back where it rested.
        assert_eq!((open.y as f64 + shift * 1.25).round() as i32, rest.y);
    }

    #[test]
    fn offset_slides_the_window_along_the_edge() {
        let left = place(screen(), Edge::Top, 0.0, 0.0, 168.0, 32.0);
        assert_eq!(left.x, 0);

        let right = place(screen(), Edge::Top, 1.0, 0.0, 168.0, 32.0);
        assert_eq!(right.x, 1920 - 168);

        // Out-of-range offsets clamp instead of flying off-screen.
        let silly = place(screen(), Edge::Top, 9.0, 0.0, 168.0, 32.0);
        assert_eq!(silly.x, 1920 - 168);
    }

    #[test]
    fn side_edges_slide_vertically() {
        let p = place(screen(), Edge::Left, 0.0, 12.0, 48.0, 200.0);
        assert_eq!(p.x, 12);
        assert_eq!(p.y, 0);

        let p = place(screen(), Edge::Right, 1.0, 12.0, 48.0, 200.0);
        assert_eq!(p.x, 1920 - 48 - 12);
        assert_eq!(p.y, 1040 - 200);
    }

    #[test]
    fn logical_sizes_scale_with_dpi() {
        // 150% scaling: a 168pt pill occupies 252 physical pixels.
        let area = WorkArea::new(0, 0, 2880, 1560, 1.5);
        let p = place(area, Edge::Top, 0.5, 8.0, 168.0, 32.0);
        assert_eq!(p.width, 252);
        assert_eq!(p.height, 48);
        assert_eq!(p.y, 12, "the margin scales too");
        assert_eq!(p.x, (2880 - 252) / 2);
    }

    #[test]
    fn fractional_scales_round_rather_than_truncate() {
        // 125% of 168 is 210; 125% of 33 is 41.25 -> 41.
        let area = WorkArea::new(0, 0, 2400, 1300, 1.25);
        let p = place(area, Edge::Top, 0.5, 0.0, 168.0, 33.0);
        assert_eq!(p.width, 210);
        assert_eq!(p.height, 41);
    }

    #[test]
    fn secondary_monitors_with_negative_origins_work() {
        // A monitor to the left of the primary has a negative x origin.
        let area = WorkArea::new(-1920, -200, 1920, 1080, 1.0);
        let p = place(area, Edge::Top, 0.5, 0.0, 200.0, 40.0);
        assert_eq!(p.x, -1920 + (1920 - 200) / 2);
        assert_eq!(p.y, -200);
    }

    #[test]
    fn a_window_is_never_pushed_outside_the_work_area() {
        // A tall card on a short screen plus a large margin would otherwise
        // start above the top of the work area.
        let area = WorkArea::new(0, 0, 1920, 500, 1.0);
        let p = place(area, Edge::Bottom, 0.5, 200.0, 344.0, 420.0);
        assert!(p.y >= 0, "clamped back inside, got y={}", p.y);
        assert!(p.y + p.height <= 500 || p.height >= 500);
    }

    #[test]
    fn an_oversized_window_pins_to_the_origin() {
        let area = WorkArea::new(100, 50, 300, 200, 1.0);
        let p = place(area, Edge::Top, 0.5, 0.0, 800.0, 600.0);
        assert_eq!(p.x, 100);
        assert_eq!(p.y, 50);
    }

    #[test]
    fn a_nonsense_scale_falls_back_to_one_to_one() {
        let area = WorkArea::new(0, 0, 1920, 1040, 0.0);
        assert_eq!(area.scale, 1.0);
        let p = place(area, Edge::Top, 0.5, 0.0, 168.0, 32.0);
        assert_eq!(p.width, 168);
    }
}

#[cfg(test)]
mod extent_tests {
    use super::*;
    use crate::config::HudSize;

    fn m() -> HudMetrics {
        HudSize::Medium.metrics()
    }

    #[test]
    fn a_resting_strip_is_only_as_deep_as_its_thickness() {
        let (w, h) = hud_extent(m(), Edge::Right, 3, false, None);
        assert_eq!(w, m().strip_thickness);
        assert_eq!(h, m().strip_extent(3).0);
    }

    #[test]
    fn opening_the_popover_grows_the_window_inward_only() {
        let resting = hud_extent(m(), Edge::Right, 3, false, None);
        let open = hud_extent(m(), Edge::Right, 3, true, None);
        assert_eq!(
            open.0,
            m().strip_thickness + m().popover_gap + m().popover_size
        );
        assert_eq!(open.1, resting.1, "length along the edge is unchanged");
    }

    #[test]
    fn a_horizontal_edge_swaps_the_axes() {
        let vertical = hud_extent(m(), Edge::Right, 3, true, None);
        let horizontal = hud_extent(m(), Edge::Top, 3, true, None);
        assert_eq!((horizontal.1, horizontal.0), vertical);
    }

    #[test]
    fn the_measured_size_wins_when_the_webview_reports_one() {
        let open = m().strip_thickness + m().popover_gap + m().popover_size;
        let (w, h) = hud_extent(m(), Edge::Right, 3, true, Some((open, 430.0)));
        assert_eq!((w, h), (open, 430.0));
    }

    #[test]
    fn hovering_the_strip_with_no_popover_keeps_the_window_narrow() {
        // The cursor can sit on the strip between two rings: the backend thinks
        // the notch is open, but the webview has nothing to show. Its measured
        // depth is the strip alone, and that must win -- otherwise the window
        // stands wide open around an empty space.
        let (w, _) = hud_extent(
            m(),
            Edge::Right,
            3,
            true,
            Some((m().strip_thickness, 430.0)),
        );
        assert_eq!(w, m().strip_thickness);
    }

    #[test]
    fn an_unmeasured_or_absurd_size_is_clamped() {
        let natural = m().strip_extent(3).0;
        assert_eq!(hud_extent(m(), Edge::Right, 3, true, None).1, natural);
        // A webview that has not laid out yet reports zero.
        assert_eq!(
            hud_extent(m(), Edge::Right, 3, true, Some((0.0, 0.0))).1,
            natural
        );
        assert_eq!(
            hud_extent(m(), Edge::Right, 3, true, Some((10.0, f64::NAN))).1,
            natural
        );
        // A runaway list cannot cover the screen, and neither can a bogus depth.
        let (w, h) = hud_extent(m(), Edge::Right, 3, true, Some((9000.0, 9000.0)));
        assert!(h <= MAX_STRIP_LENGTH.max(MAX_POPOVER_LENGTH));
        assert!(w <= m().strip_thickness + m().popover_gap + m().popover_size);
    }

    #[test]
    fn the_strip_hugs_its_edge_within_the_window() {
        let window = hud_extent(m(), Edge::Right, 3, true, None);
        // Docked right: the strip is pushed to the window's right end, leaving
        // the popover the inward side.
        let (x, y) = strip_offset(m(), Edge::Right, window);
        assert_eq!(x, window.0 - m().strip_thickness);
        assert_eq!(y, 0.0);

        // Docked left: the strip is already at the origin.
        assert_eq!(strip_offset(m(), Edge::Left, window), (0.0, 0.0));

        let window = hud_extent(m(), Edge::Bottom, 3, true, None);
        let (x, y) = strip_offset(m(), Edge::Bottom, window);
        assert_eq!(x, 0.0);
        assert_eq!(y, window.1 - m().strip_thickness);
    }
}
