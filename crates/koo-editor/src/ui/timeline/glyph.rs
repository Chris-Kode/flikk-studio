//! Small vector icons painted with GPUI paths. The app registers no icon
//! assets beyond the component defaults, so the editor's own tool icons are
//! drawn here instead of loaded as SVGs.

use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Pointer,
    Blade,
    Trim,
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
    PrevCut,
    NextCut,
    Trash,
    SafeArea,
    CornerPin,
    Mask,
    MaskRect,
    MaskEllipse,
    Pen,
    /// A figure with a spark: a mask the model finds.
    Subject,
    /// A target with motion lines: follow through the clip.
    Track,
    /// A disc with hairs at its edge: edges eased onto the picture's.
    Refine,
    /// A frame half hatched: show what masks cut away.
    MaskOverlay,
    /// Two beamed notes: music.
    Note,
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
        Glyph::Trim => {
            lines(
                &[(6.5, 2.5), (3.5, 2.5), (3.5, 13.5), (6.5, 13.5)],
                false,
                window,
            );
            lines(
                &[(9.5, 2.5), (12.5, 2.5), (12.5, 13.5), (9.5, 13.5)],
                false,
                window,
            );
            lines(&[(8., 5.), (8., 11.)], false, window);
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
        Glyph::Note => {
            circle(4.6, 12.2, 2.1, true, window);
            circle(11.6, 10.7, 2.1, true, window);
            lines(&[(6.5, 12.2), (6.5, 3.6)], false, window);
            lines(&[(13.5, 10.7), (13.5, 2.1)], false, window);
            fill(&[(6.5, 3.), (13.5, 1.5), (13.5, 4.), (6.5, 5.5)], window);
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
        Glyph::SafeArea => {
            lines(
                &[(1.5, 3.), (14.5, 3.), (14.5, 13.), (1.5, 13.)],
                true,
                window,
            );
            lines(
                &[(4.5, 5.5), (11.5, 5.5), (11.5, 10.5), (4.5, 10.5)],
                true,
                window,
            );
        }
        Glyph::CornerPin => {
            let quad = [(2.5, 4.5), (13., 2.5), (12., 13.5), (4., 11.5)];
            lines(&quad, true, window);
            for (x, y) in quad {
                fill(
                    &[
                        (x - 1.5, y - 1.5),
                        (x + 1.5, y - 1.5),
                        (x + 1.5, y + 1.5),
                        (x - 1.5, y + 1.5),
                    ],
                    window,
                );
            }
        }
        Glyph::Mask => {
            lines(
                &[(1.5, 3.), (14.5, 3.), (14.5, 13.), (1.5, 13.)],
                true,
                window,
            );
            circle(8., 8., 3.2, true, window);
        }
        Glyph::MaskRect => {
            lines(
                &[(2.5, 3.5), (13.5, 3.5), (13.5, 12.5), (2.5, 12.5)],
                true,
                window,
            );
        }
        Glyph::MaskEllipse => circle(8., 8., 5.5, false, window),
        Glyph::Subject => {
            circle(6.5, 5.5, 2.6, true, window);
            fill(
                &[
                    (1.5, 14.5),
                    (2.3, 11.),
                    (4.3, 9.4),
                    (8.7, 9.4),
                    (10.7, 11.),
                    (11.5, 14.5),
                ],
                window,
            );
            fill(
                &[
                    (13.2, 0.8),
                    (13.9, 2.6),
                    (15.5, 3.3),
                    (13.9, 4.0),
                    (13.2, 5.8),
                    (12.5, 4.0),
                    (10.9, 3.3),
                    (12.5, 2.6),
                ],
                window,
            );
        }
        Glyph::Track => {
            circle(9.5, 8., 4.5, false, window);
            circle(9.5, 8., 1.4, true, window);
            lines(&[(0.8, 5.5), (3.6, 5.5)], false, window);
            lines(&[(0.8, 10.5), (3.6, 10.5)], false, window);
        }
        Glyph::Refine => {
            circle(6.5, 8., 4.5, true, window);
            for (a, b) in [
                ((11.5, 4.5), (14., 2.5)),
                ((12.2, 7.), (15., 6.)),
                ((12.2, 9.5), (15., 10.5)),
                ((11.5, 12.), (14., 14.)),
            ] {
                lines(&[a, b], false, window);
            }
        }
        Glyph::MaskOverlay => {
            lines(
                &[(1.5, 3.), (14.5, 3.), (14.5, 13.), (1.5, 13.)],
                true,
                window,
            );
            for (a, b) in [
                ((8., 13.), (14.5, 6.5)),
                ((8., 8.5), (13.5, 3.)),
                ((11.5, 13.), (14.5, 10.)),
                ((8., 4.5), (9.5, 3.)),
            ] {
                lines(&[a, b], false, window);
            }
            lines(&[(8., 3.), (8., 13.)], false, window);
        }
        Glyph::Pen => {
            lines(
                &[
                    (3., 13.),
                    (4., 9.),
                    (11., 2.),
                    (14., 5.),
                    (7., 12.),
                    (3., 13.),
                ],
                false,
                window,
            );
            lines(&[(9.5, 3.5), (12.5, 6.5)], false, window);
        }
        Glyph::PrevCut => {
            lines(&[(3.5, 3.5), (3.5, 12.5)], false, window);
            fill(&[(12.5, 3.5), (5.5, 8.), (12.5, 12.5)], window);
        }
        Glyph::NextCut => {
            lines(&[(12.5, 3.5), (12.5, 12.5)], false, window);
            fill(&[(3.5, 3.5), (10.5, 8.), (3.5, 12.5)], window);
        }
        Glyph::Trash => {
            lines(&[(2.5, 4.5), (13.5, 4.5)], false, window);
            lines(
                &[(6., 4.5), (6., 2.5), (10., 2.5), (10., 4.5)],
                false,
                window,
            );
            lines(
                &[(3.8, 4.5), (4.8, 13.5), (11.2, 13.5), (12.2, 4.5)],
                false,
                window,
            );
            lines(&[(6.8, 7.), (6.8, 11.)], false, window);
            lines(&[(9.2, 7.), (9.2, 11.)], false, window);
        }
    }
}
