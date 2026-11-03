//! The transition picked on the timeline: its program, duration, alignment
//! on the cut, parameters with keyframes, and removal.

use super::controls::{FxOwner, FxTarget};
use super::param::Key;
use super::{InspectorPanel, View, group, label, panel_header, row, select_field};
use crate::ui::kit::{
    Button, DropdownMenu as _, IconName, PopupMenuItem, Segmented, h_flex, v_flex,
};
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
    ) -> Option<View> {
        let owner = FxOwner::Transition { clip, edge };
        let target = self.fx_target(&owner, cx)?;
        let transition = target.transition.clone()?;
        let at_cut = self.at_cut(clip, edge, cx);
        let mut rows = vec![self.transition_type(&transition, clip, edge, cx)];
        rows.push(self.param_row(
            Key::TransitionDuration,
            transition.duration.as_secs_f64() as f32,
            false,
            None,
            window,
            cx,
        ));
        if at_cut {
            rows.push(self.transition_align(&transition, clip, edge, cx));
        }
        let params = self.fx_param_rows(&target, window, cx);
        let rows = rows
            .into_iter()
            .map(IntoElement::into_any_element)
            .chain(params)
            .chain([self
                .transition_actions(&target, clip, edge, cx)
                .into_any_element()]);
        let header = group(
            crate::ui::kit::SectionHeader::new("transition", target.asset.name.clone()),
            rows,
        );
        Some(View::new(
            panel_header("Transition"),
            v_flex().child(header),
        ))
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

    /// Edit its code, mention it in the chat, remove it.
    fn transition_actions(
        &self,
        target: &FxTarget,
        clip: ClipId,
        edge: Edge,
        cx: &mut Context<Self>,
    ) -> Div {
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
            .flex_none()
            .h(px(30.))
            .pl(px(16.))
            .pr(px(10.))
            .gap(px(2.))
            .child(
                Button::new("transition-code")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Code)
                    .label("Edit code")
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.edit_fx(Some(fx.clone()), cx))
                    }),
            )
            .child(
                Button::new("transition-chat")
                    .ghost()
                    .xsmall()
                    .icon(IconName::MessageSquarePlus)
                    .label("Add to chat")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mention = mention.clone();
                        this.state.update(cx, |s, cx| s.mention(vec![mention], cx));
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("transition-remove")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Trash)
                    .tooltip("Remove")
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
        let name = options
            .iter()
            .find(|(_, r)| *r == current)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| current.to_string());
        let (panel, base) = (cx.entity(), transition.clone());
        row(
            label("Type"),
            select_field("transition-type", name).dropdown_menu(move |mut menu, _, _| {
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
            None,
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
        row(
            label("Align"),
            Segmented::new("transition-align")
                .compact()
                .fill()
                .items(ALIGNS.map(|(_, l)| l))
                .selected(selected)
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
            None,
        )
    }
}
