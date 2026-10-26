//! Small vector icons painted with GPUI paths. The app registers no icon
//! assets beyond the component defaults, so the editor's own tool icons are
//! drawn here instead of loaded as SVGs.

use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Pointer,
    Blade,
    Magnet,
    Marker,
    Silence,
    Play,
    Pause,
    Lock,
    Unlock,
    Eye,
    EyeOff,
    Speaker,
    SpeakerOff,
    Headphones,
    Plus,
    Minus,
    Fit,
}

/// A `size`-square element drawing `glyph` in `color`.
pub fn glyph(glyph: Glyph, size: Pixels, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| paint_glyph(glyph, bounds, color, window),
    )
    .size(size)
    .flex_none()
}

/// Paint `glyph` inside `bounds`, designed on a 16-unit grid.
pub fn paint_glyph(glyph: Glyph, bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    let unit = bounds.size.width.min(bounds.size.height) / 16.;
    let o = bounds.origin;
    let p = |x: f32, y: f32| point(o.x + unit * x, o.y + unit * y);
    let stroke = (unit * 1.4).max(px(1.));

    let lines = |pts: &[(f32, f32)], close: bool, window: &mut Window| {
        let mut b = PathBuilder::stroke(stroke);
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
    let fill = |pts: &[(f32, f32)], window: &mut Window| {
        let mut b = PathBuilder::fill();
        for (i, (x, y)) in pts.iter().enumerate() {
            if i == 0 {
                b.move_to(p(*x, *y));
            } else {
                b.line_to(p(*x, *y));
            }
        }
        b.close();
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };
    let circle = |cx: f32, cy: f32, r: f32, filled: bool, window: &mut Window| {
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
    let arc = |from: (f32, f32), to: (f32, f32), r: f32, sweep: bool, window: &mut Window| {
        let mut b = PathBuilder::stroke(stroke);
        b.move_to(p(from.0, from.1));
        b.arc_to(
            point(unit * r, unit * r),
            px(0.),
            false,
            sweep,
            p(to.0, to.1),
        );
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    };

    match glyph {
        Glyph::Pointer => fill(
            &[
                (4., 2.),
                (12.5, 9.5),
                (8.6, 10.),
                (10.8, 14.),
                (9.2, 14.8),
                (7., 10.8),
                (4., 13.5),
            ],
            window,
        ),
        Glyph::Blade => {
            circle(4.5, 12., 2.2, false, window);
            circle(11.5, 12., 2.2, false, window);
            lines(&[(5.8, 10.2), (11.5, 2.)], false, window);
            lines(&[(10.2, 10.2), (4.5, 2.)], false, window);
        }
        Glyph::Magnet => {
            lines(&[(3., 2.5), (3., 8.)], false, window);
            lines(&[(13., 2.5), (13., 8.)], false, window);
            arc((3., 8.), (13., 8.), 5., false, window);
            lines(&[(3., 5.), (6., 5.)], false, window);
            lines(&[(10., 5.), (13., 5.)], false, window);
            lines(&[(6., 2.5), (6., 8.)], false, window);
            lines(&[(10., 2.5), (10., 8.)], false, window);
            arc((6., 8.), (10., 8.), 2., false, window);
        }
        Glyph::Marker => fill(
            &[(4., 2.), (12., 2.), (12., 14.), (8., 10.5), (4., 14.)],
            window,
        ),
        Glyph::Silence => {
            for (x, h) in [(2., 3.), (4.5, 6.), (11.5, 5.), (14., 2.5)] {
                lines(&[(x, 8. - h), (x, 8. + h)], false, window);
            }
            lines(&[(6.5, 8.), (9.5, 8.)], false, window);
        }
        Glyph::Play => fill(&[(4.5, 2.5), (13.5, 8.), (4.5, 13.5)], window),
        Glyph::Pause => {
            fill(&[(4., 3.), (6.8, 3.), (6.8, 13.), (4., 13.)], window);
            fill(&[(9.2, 3.), (12., 3.), (12., 13.), (9.2, 13.)], window);
        }
        Glyph::Lock | Glyph::Unlock => {
            fill(&[(3., 7.5), (13., 7.5), (13., 14.), (3., 14.)], window);
            if glyph == Glyph::Lock {
                lines(&[(5., 7.5), (5., 5.)], false, window);
                lines(&[(11., 7.5), (11., 5.)], false, window);
                arc((5., 5.), (11., 5.), 3., true, window);
            } else {
                lines(&[(5., 7.5), (5., 5.)], false, window);
                arc((5., 5.), (11., 5.), 3., true, window);
            }
        }
        Glyph::Eye | Glyph::EyeOff => {
            arc((1.5, 8.), (14.5, 8.), 9., true, window);
            arc((1.5, 8.), (14.5, 8.), 9., false, window);
            circle(8., 8., 2.2, true, window);
            if glyph == Glyph::EyeOff {
                lines(&[(2.5, 2.5), (13.5, 13.5)], false, window);
            }
        }
        Glyph::Speaker | Glyph::SpeakerOff => {
            fill(
                &[
                    (2., 6.),
                    (4.5, 6.),
                    (8., 3.),
                    (8., 13.),
                    (4.5, 10.),
                    (2., 10.),
                ],
                window,
            );
            if glyph == Glyph::SpeakerOff {
                lines(&[(10.5, 5.5), (15., 10.5)], false, window);
                lines(&[(15., 5.5), (10.5, 10.5)], false, window);
            } else {
                arc((10.5, 5.5), (10.5, 10.5), 3.5, true, window);
                arc((12.5, 3.5), (12.5, 12.5), 6., true, window);
            }
        }
        Glyph::Headphones => {
            arc((2.5, 10.), (13.5, 10.), 5.5, true, window);
            fill(&[(1.8, 9.), (5., 9.), (5., 14.), (1.8, 14.)], window);
            fill(&[(11., 9.), (14.2, 9.), (14.2, 14.), (11., 14.)], window);
        }
        Glyph::Plus => {
            lines(&[(3., 8.), (13., 8.)], false, window);
            lines(&[(8., 3.), (8., 13.)], false, window);
        }
        Glyph::Minus => lines(&[(3., 8.), (13., 8.)], false, window),
        Glyph::Fit => {
            lines(&[(2., 5.), (2., 11.)], false, window);
            lines(&[(14., 5.), (14., 11.)], false, window);
            lines(&[(4.5, 8.), (11.5, 8.)], false, window);
            lines(&[(6.5, 6.), (4.5, 8.), (6.5, 10.)], false, window);
            lines(&[(9.5, 6.), (11.5, 8.), (9.5, 10.)], false, window);
        }
    }
}
