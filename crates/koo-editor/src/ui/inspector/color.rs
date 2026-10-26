//! Primary grade (PRD K1): the Color workspace grades the whole timeline,
//! a media clip's Color tab the selected clips.

use super::param::Key;
use super::{InspectorPanel, empty, is_media, section, section_with};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::model::{Clip, Grade};

const PRIMARY: [Key; 6] = [
    Key::Exposure,
    Key::Contrast,
    Key::Saturation,
    Key::Gamma,
    Key::Temperature,
    Key::Tint,
];

impl InspectorPanel {
    /// The timeline grade, applied over every media clip's own.
    pub(super) fn timeline_color_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let g = self.project(cx).timeline.grade.clone();
        let reset = Button::new("timeline-grade-reset")
            .ghost()
            .xsmall()
            .icon(IconName::Undo2)
            .tooltip("Reset")
            .disabled(g == Grade::default())
            .on_click(cx.listener(|this, _, _, cx| {
                let commands = vec![Command::SetTimelineGrade {
                    grade: Grade::default(),
                }];
                this.run(commands, "reset_timeline_grade", cx);
            }));
        self.grade_controls(&g, reset, window, cx)
    }

    pub(super) fn color_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let clips: Vec<Clip> = self
            .selected_clips(cx)
            .into_iter()
            .filter(is_media)
            .collect();
        let Some(clip) = clips.first().cloned() else {
            return empty("No clip selected", cx);
        };
        let g = clip.grade.clone();
        let ids: Vec<_> = clips.iter().map(|c| c.id).collect();
        let source = self
            .copied_grade
            .filter(|from| ids.iter().any(|id| id != from));
        let reset_ids = ids.clone();
        let actions = h_flex()
            .gap_1()
            .child(
                Button::new("grade-reset")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Undo2)
                    .tooltip("Reset")
                    .disabled(g == Grade::default())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let commands = vec![Command::SetGrade {
                            clips: reset_ids.clone(),
                            grade: Grade::default(),
                        }];
                        this.run(commands, "reset_grade", cx);
                    })),
            )
            .child(
                Button::new("grade-copy")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip("Copy grade")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copied_grade = Some(clip.id);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("grade-paste")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Replace)
                    .tooltip("Paste grade to selection")
                    .disabled(source.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(from) = source else { return };
                        let to: Vec<_> = ids.iter().copied().filter(|id| *id != from).collect();
                        this.run(vec![Command::CopyGrade { from, to }], "copy_grade", cx);
                    })),
            );
        self.grade_controls(&g, actions, window, cx)
    }

    /// Primary sliders and lift/gamma/gain for `g`, `actions` by the title.
    fn grade_controls(
        &mut self,
        g: &Grade,
        actions: impl IntoElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut primary = Vec::new();
        for key in PRIMARY {
            primary.push(self.param(key, grade_value(g, key), window, cx).row(cx));
        }
        let theme = cx.theme();
        let channels = [theme.red, theme.green, theme.blue];
        let mut wheels = Vec::new();
        for (label, make) in [
            ("Lift", Key::Lift as fn(usize) -> Key),
            ("Gamma", Key::GammaRgb),
            ("Gain", Key::Gain),
        ] {
            let mut row = h_flex().h(px(28.)).gap_2().child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            );
            for (i, color) in channels.iter().enumerate() {
                let key = make(i);
                row = row.child(
                    self.param(key, grade_value(g, key), window, cx)
                        .slider(Some(*color)),
                );
            }
            wheels.push(row);
        }
        v_flex()
            .gap_4()
            .child(section_with("Primary", actions, cx).children(primary))
            .child(section("Lift · Gamma · Gain", cx).children(wheels))
            .into_any_element()
    }
}

/// Set the grade field `key` names to `v`.
pub(super) fn set_grade_value(g: &mut Grade, key: Key, v: f32) {
    match key {
        Key::Exposure => g.exposure = v,
        Key::Contrast => g.contrast = v,
        Key::Saturation => g.saturation = v,
        Key::Gamma => g.gamma = v,
        Key::Temperature => g.temperature = v,
        Key::Tint => g.tint = v,
        Key::Lift(i) => g.lift[i] = v,
        Key::GammaRgb(i) => g.gamma_rgb[i] = v,
        Key::Gain(i) => g.gain[i] = v,
        _ => {}
    }
}

fn grade_value(g: &Grade, key: Key) -> f32 {
    match key {
        Key::Exposure => g.exposure,
        Key::Contrast => g.contrast,
        Key::Saturation => g.saturation,
        Key::Gamma => g.gamma,
        Key::Temperature => g.temperature,
        Key::Tint => g.tint,
        Key::Lift(i) => g.lift[i],
        Key::GammaRgb(i) => g.gamma_rgb[i],
        Key::Gain(i) => g.gain[i],
        _ => 0.0,
    }
}
