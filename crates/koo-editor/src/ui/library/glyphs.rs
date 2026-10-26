//! Icons for FX kinds and the inspector's FX controls, painted with GPUI
//! paths on a 16-unit grid like the timeline's tool glyphs (the app bundles
//! only the default component icons).

use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxGlyph {
    /// Video effect: a four-point sparkle.
    Effect,
    /// Two overlapping frames.
    Transition,
    /// Shader generator: a circle and a square.
    Generator,
    /// HTML title or motion graphic: a "T".
    Title,
    /// Audio effect: waveform bars.
    Audio,
    /// A project's own program: `</>`.
    Code,
    /// Ripple: an arrow pushing a bar.
    Ripple,
    FlipH,
    FlipV,
    /// Drag handle: two columns of dots.
    Grip,
}

/// A `size`-square element drawing `glyph` in `color`.
pub fn fx_glyph(glyph: FxGlyph, size: Pixels, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| paint(glyph, bounds, color, window),
    )
    .size(size)
    .flex_none()
}

fn paint(glyph: FxGlyph, bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    let unit = bounds.size.width.min(bounds.size.height) / 16.;
    let o = bounds.origin;
    let p = |x: f32, y: f32| point(o.x + unit * x, o.y + unit * y);
    let stroke = (unit * 1.4).max(px(1.));

    let build = |mut b: PathBuilder, pts: &[(f32, f32)], close: bool, window: &mut Window| {
        for (i, (x, y)) in pts.iter().enumerate() {
            if i == 0 {
                b.move_to(p(*x, *y));
            } else {
                b.line_to(p(*x, *y));
            }
        }
        if close {
            b.close();
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };
    let lines = |pts: &[(f32, f32)], close: bool, window: &mut Window| {
        build(PathBuilder::stroke(stroke), pts, close, window)
    };
    let fill =
        |pts: &[(f32, f32)], window: &mut Window| build(PathBuilder::fill(), pts, true, window);
    let dot = |cx: f32, cy: f32, r: f32, filled: bool, window: &mut Window| {
        let mut b = if filled {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(stroke)
        };
        let radii = point(unit * r, unit * r);
        b.move_to(p(cx + r, cy));
        b.arc_to(radii, px(0.), false, true, p(cx - r, cy));
        b.arc_to(radii, px(0.), false, true, p(cx + r, cy));
        b.close();
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };

    match glyph {
        FxGlyph::Effect => {
            fill(
                &[
                    (7., 1.5),
                    (8.6, 6.4),
                    (13.5, 8.),
                    (8.6, 9.6),
                    (7., 14.5),
                    (5.4, 9.6),
                    (0.5, 8.),
                    (5.4, 6.4),
                ],
                window,
            );
            fill(
                &[
                    (13., 1.),
                    (13.7, 2.8),
                    (15.5, 3.5),
                    (13.7, 4.2),
                    (13., 6.),
                    (12.3, 4.2),
                    (10.5, 3.5),
                    (12.3, 2.8),
                ],
                window,
            );
        }
        FxGlyph::Transition => {
            lines(
                &[(1.5, 3.5), (9.5, 3.5), (9.5, 10.), (1.5, 10.)],
                true,
                window,
            );
            fill(&[(6.5, 6.), (14.5, 6.), (14.5, 12.5), (6.5, 12.5)], window);
        }
        FxGlyph::Generator => {
            dot(5.5, 5.5, 3.6, false, window);
            fill(
                &[(7.5, 7.5), (14.5, 7.5), (14.5, 14.5), (7.5, 14.5)],
                window,
            );
        }
        FxGlyph::Title => {
            fill(&[(2., 2.), (14., 2.), (14., 4.5), (2., 4.5)], window);
            fill(&[(6.7, 4.5), (9.3, 4.5), (9.3, 14.), (6.7, 14.)], window);
        }
        FxGlyph::Audio => {
            for (x, h) in [(2., 3.), (5., 7.), (8., 11.), (11., 6.), (14., 3.5)] {
                lines(&[(x, 8. - h / 2.), (x, 8. + h / 2.)], false, window);
            }
        }
        FxGlyph::Code => {
            lines(&[(5., 4.), (1.5, 8.), (5., 12.)], false, window);
            lines(&[(11., 4.), (14.5, 8.), (11., 12.)], false, window);
            lines(&[(9.5, 2.5), (6.5, 13.5)], false, window);
        }
        FxGlyph::Ripple => {
            fill(&[(12., 2.), (14., 2.), (14., 14.), (12., 14.)], window);
            lines(&[(1.5, 8.), (9.5, 8.)], false, window);
            lines(&[(6.5, 4.5), (10., 8.), (6.5, 11.5)], false, window);
        }
        FxGlyph::FlipH => {
            lines(&[(8., 1.), (8., 15.)], false, window);
            fill(&[(6., 3.5), (6., 12.5), (1., 12.5)], window);
            lines(&[(10., 3.5), (10., 12.5), (15., 12.5)], true, window);
        }
        FxGlyph::FlipV => {
            lines(&[(1., 8.), (15., 8.)], false, window);
            fill(&[(3.5, 6.), (12.5, 6.), (12.5, 1.)], window);
            lines(&[(3.5, 10.), (12.5, 10.), (12.5, 15.)], true, window);
        }
        FxGlyph::Grip => {
            for y in [3.5, 8., 12.5] {
                dot(6., y, 1.1, true, window);
                dot(10., y, 1.1, true, window);
            }
        }
    }
}
