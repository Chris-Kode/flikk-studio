//! The transition picked on the timeline: its program, duration, alignment
//! on the cut, parameters with keyframes, and removal.

use super::controls::{FxOwner, FxTarget};
use super::param::Key;
use super::{InspectorPanel, section};
use crate::ui::library::{FxGlyph, fx_glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::ClipId;
use koo_core::command::{Command, Edge};
use koo_core::fx::{self, FxInstance, FxKind, FxRef, Transition, TransitionAlign};

const ALIGNS: [(TransitionAlign, &str); 3] = [
    (TransitionAlign::StartOnCut, "Start"),
    (TransitionAlign::Center, "Center"),
    (TransitionAlign::EndOnCut, "End"),
];

impl InspectorPanel {
    /// `None` when the selected transition no longer exists.
    pub(super) fn transition_view(
        &mut self,
        clip: ClipId,
        edge: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let owner = FxOwner::Transition { clip, edge };
        let target = self.fx_target(&owner, cx)?;
        let transition = target.transition.clone()?;
        let at_cut = self.at_cut(clip, edge, cx);
        let header = self.transition_header(&target, clip, edge, cx);
        let duration = self
            .param(
                Key::TransitionDuration,
                transition.duration.as_secs_f64() as f32,
                window,
                cx,
            )
            .row(cx);
        let mut timing = v_flex()
            .gap_0p5()
            .child(self.transition_type(&transition, clip, edge, cx))
            .child(duration);
        if at_cut {
            timing = timing.child(self.transition_align(&transition, clip, edge, cx));
        }
        let rows = self.fx_param_rows(&target, window, cx);
        let params = (!rows.is_empty()).then(|| section("Parameters", cx).children(rows));
        Some(
            v_flex()
                .gap_4()
                .child(header)
                .child(timing)
                .children(params)
                .into_any_element(),
        )
    }

    /// Whether a clip meets this edge, so the transition sits on a cut.
    fn at_cut(&self, clip: ClipId, edge: Edge, cx: &App) -> bool {
        let tl = &self.project(cx).timeline;
        let Some((track, c)) = tl.clip(clip) else {
            return false;
        };
        track.clips.iter().any(|o| {
            o.enabled
                && match edge {
                    Edge::Start => o.end() == c.start,
                    Edge::End => o.start == c.end(),
                }
        })
    }

    fn transition_header(
        &self,
        target: &FxTarget,
        clip: ClipId,
        edge: Edge,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (muted, secondary) = (theme.muted_foreground, theme.secondary);
        let fx = target.asset.id.clone();
        let side = match edge {
            Edge::Start => "in",
            Edge::End => "out",
        };
        let mention = (
            format!("@transition {clip}/{side}"),
            SharedString::from(target.asset.name.clone()),
        );
        let state = self.state.clone();
        h_flex()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_sm()
                    .bg(secondary)
                    .text_xs()
                    .child("Transition"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(target.asset.name.clone()),
            )
            .child(
                Button::new("transition-chat")
                    .ghost()
                    .xsmall()
                    .tooltip("Add to chat")
                    .child(div().text_xs().text_color(muted).child("@"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mention = mention.clone();
                        this.state.update(cx, |s, cx| s.mention(vec![mention], cx));
                    })),
            )
            .child(
                Button::new("transition-code")
                    .ghost()
                    .xsmall()
                    .tooltip("Edit code")
                    .child(fx_glyph(FxGlyph::Code, px(13.), muted))
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.edit_fx(Some(fx.clone()), cx))
                    }),
            )
            .child(
                Button::new("transition-remove")
                    .ghost()
                    .xsmall()
                    .tooltip("Remove")
                    .child(crate::ui::timeline::glyph::glyph(
                        crate::ui::timeline::glyph::Glyph::Trash,
                        px(13.),
                        muted,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run(
                            vec![Command::SetTransition {
                                clip,
                                edge,
                                transition: None,
                            }],
                            "set_transition",
                            cx,
                        );
                        this.state.update(cx, |s, cx| s.select_transition(None, cx));
                    })),
            )
    }

    /// Program dropdown; switching keeps duration and alignment.
    fn transition_type(
        &self,
        transition: &Transition,
        clip: ClipId,
        edge: Edge,
        cx: &mut Context<Self>,
    ) -> Div {
        let options: Vec<(String, FxRef)> = fx::library(self.project(cx))
            .filter(|a| a.kind == FxKind::Transition)
            .map(|a| (a.name.clone(), a.id.clone()))
            .collect();
        let current = transition.fx.fx.clone();
        let label = options
            .iter()
            .find(|(_, r)| *r == current)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| current.to_string());
        let (panel, base) = (cx.entity(), transition.clone());
        labeled(
            "Type",
            Button::new("transition-type")
                .outline()
                .xsmall()
                .w_full()
                .label(label)
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for (name, r) in &options {
                        let (panel, r, base) = (panel.clone(), r.clone(), base.clone());
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(r == current)
                                .on_click(move |_, _, cx| {
                                    let transition = Transition {
                                        fx: FxInstance::new(r.clone()),
                                        ..base.clone()
                                    };
                                    panel.update(cx, |this, cx| {
                                        this.run(
                                            vec![Command::SetTransition {
                                                clip,
                                                edge,
                                                transition: Some(transition),
                                            }],
                                            "set_transition",
                                            cx,
                                        )
                                    })
                                }),
                        );
                    }
                    menu
                }),
            cx,
        )
    }

    fn transition_align(
        &self,
        transition: &Transition,
        clip: ClipId,
        edge: Edge,
        cx: &mut Context<Self>,
    ) -> Div {
        let base = transition.clone();
        let selected = ALIGNS
            .iter()
            .position(|(a, _)| *a == transition.align)
            .unwrap_or(1);
        labeled(
            "Align",
            TabBar::new("transition-align")
                .segmented()
                .xsmall()
                .selected_index(selected)
                .children(ALIGNS.iter().map(|(_, l)| Tab::new().label(*l)))
                .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                    let transition = Transition {
                        align: ALIGNS[*ix].0,
                        ..base.clone()
                    };
                    this.run(
                        vec![Command::SetTransition {
                            clip,
                            edge,
                            transition: Some(transition),
                        }],
                        "set_transition",
                        cx,
                    );
                })),
            cx,
        )
    }
}

/// A 28 px row with a label column, like the slider rows.
pub(super) fn labeled(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .h(px(28.))
        .gap_2()
        .child(
            div()
                .w(px(64.))
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}
