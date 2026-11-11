//! A picture clip's masks (see `flikk_core::mask`): add a rectangle, an
//! ellipse or a depth range (paths are drawn with the viewer's pen, subjects
//! picked with its subject tool), and the list of masks, each with its mode
//! and an eye that enables it. The open mask shows its mode, invert,
//! smoothing, its edge, opacity and box as fields, a keyframe for its whole
//! shape at the playhead, and removal. A subject also tracks or not,
//! refines its edge or not, turns into a path from its outline at the
//! playhead, and shows how far its tracking got. A person mask ("Remove
//! background": the people kept, the background cut away) eases its edge
//! onto the picture's by its Edge field. A depth mask keeps a range of
//! distance (near, far, softness), the background at first.

use super::param::{Key, MaskField};
use super::{InspectorPanel, clip_time, error_line, label, row, select_field, wide};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, EmptyState, Icon, IconName, Keyframe, KeyframeNav, PopupMenuItem,
    Progress, Switch, h_flex, v_flex,
};
use flikk_core::Time;
use flikk_core::command::Command;
use flikk_core::compose;
use flikk_core::mask::{Mask, MaskKeyframe, MaskMode, MaskShape, MaskState};
use flikk_core::model::{Clip, ClipContent};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Most points of a path made from a subject's outline.
const PATH_POINTS: usize = 48;

/// Turns a clip's mask list into the one to commit.
type Change = dyn Fn(&mut Vec<Mask>);

fn kind(shape: &MaskShape) -> (&'static str, IconName) {
    match shape {
        MaskShape::Rectangle { .. } => ("Rectangle", IconName::Square),
        MaskShape::Ellipse { .. } => ("Ellipse", IconName::Circle),
        MaskShape::Path { .. } => ("Path", IconName::PenTool),
        MaskShape::Subject => ("Subject", IconName::Focus),
        MaskShape::Person { .. } => ("Person", IconName::PersonStanding),
        MaskShape::Depth { .. } => ("Depth", IconName::Layers),
    }
}

impl InspectorPanel {
    /// The Masks page: the add tools, then every mask.
    pub(super) fn masks_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let add = |shape: MaskShape,
                   icon: IconName,
                   tip: &'static str,
                   clip: &Clip,
                   cx: &mut Context<Self>| {
            let (id, masks) = (clip.id, clip.masks.clone());
            Button::new(SharedString::from(format!("add-mask-{tip}")))
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut masks = masks.clone();
                    let mask = Mask::new(shape.clone());
                    this.open_mask = Some(mask.id);
                    masks.push(mask);
                    this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
                }))
        };
        let tools = h_flex()
            .flex_none()
            .h(px(36.))
            .px(px(8.))
            .gap(px(2.))
            .border_b_1()
            .border_color(t::LN)
            .child(add(
                MaskShape::Rectangle {
                    center: [0.5, 0.5],
                    size: [0.5, 0.5],
                    rotation_deg: 0.0,
                    roundness: 0.0,
                },
                IconName::Square,
                "Rectangle",
                clip,
                cx,
            ))
            .child(add(
                MaskShape::Ellipse {
                    center: [0.5, 0.5],
                    size: [0.5, 0.5],
                    rotation_deg: 0.0,
                },
                IconName::Circle,
                "Ellipse",
                clip,
                cx,
            ))
            .child(add(
                MaskShape::Depth {
                    near: 0.5,
                    far: 1.0,
                    softness: 0.05,
                },
                IconName::Layers,
                "Depth",
                clip,
                cx,
            ))
            .child(div().flex_1())
            .when(matches!(clip.content, ClipContent::Media { .. }), |row| {
                let (id, masks) = (clip.id, clip.masks.clone());
                row.child(
                    Button::new("add-mask-person")
                        .ghost()
                        .small()
                        .icon(IconName::PersonStanding)
                        .label("Remove background")
                        .disabled(masks.iter().any(Mask::is_person))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if masks.iter().any(Mask::is_person) {
                                return;
                            }
                            let mut masks = masks.clone();
                            let mask = Mask::person();
                            this.open_mask = Some(mask.id);
                            masks.push(mask);
                            this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
                        })),
                )
            });
        if !clip.masks.iter().any(|m| Some(m.id) == self.open_mask) {
            self.open_mask = clip.masks.first().map(|m| m.id);
        }
        let blocks: Vec<Div> = (0..clip.masks.len())
            .map(|i| self.mask_block(clip, i, window, cx))
            .collect();
        v_flex()
            .child(tools)
            .when(blocks.is_empty(), |v| {
                v.child(EmptyState::new(IconName::Circle, "No masks").compact())
            })
            .children(blocks)
    }

    /// One mask: its row, then its settings while it is the open one.
    fn mask_block(
        &mut self,
        clip: &Clip,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mask = clip.masks[i].clone();
        let open = self.open_mask == Some(mask.id);
        let (kind_name, icon) = kind(&mask.state.shape);
        let nth = clip.masks[..=i]
            .iter()
            .filter(|m| kind(&m.state.shape).0 == kind_name)
            .count();
        let (id, masks) = (clip.id, clip.masks.clone());
        let mask_id = mask.id;
        let enabled = mask.enabled;
        let header = h_flex()
            .id(("mask", i))
            .flex_none()
            .relative()
            .h(px(30.))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(8.))
            .cursor_pointer()
            .when(open, |d| d.bg(t::HV))
            .when(!open, |d| d.hover(|s| s.bg(t::alpha(t::HV, 0.6))))
            .when(open, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(t::AC),
                )
            })
            .child(
                Icon::new(icon)
                    .size(px(12.))
                    .color(if open { t::T1 } else { t::T2 }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if enabled { t::T1 } else { t::T3 })
                    .child(format!("{kind_name} {nth}")),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t::T3)
                    .child(if mask.invert {
                        format!("{} · Inverted", mask.mode.label())
                    } else {
                        mask.mode.label().to_owned()
                    }),
            )
            .child(
                Button::new(("mask-enabled", i))
                    .ghost()
                    .xsmall()
                    .icon(if enabled {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .tooltip(if enabled { "Disable" } else { "Enable" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let mut masks = masks.clone();
                        masks[i].enabled = !masks[i].enabled;
                        this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_mask = Some(mask_id);
                cx.notify();
            }));
        let block = v_flex().border_b_1().border_color(t::LN).child(header);
        if !open {
            return block;
        }
        block.child(self.mask_settings(clip, i, window, cx))
    }

    /// The open mask's settings.
    fn mask_settings(
        &mut self,
        clip: &Clip,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let frame = state.bus.project().timeline.settings.rate.frame_duration();
        let at = clip_time(clip, playhead);
        let inside = clip.range().contains(playhead);
        let mask = clip.masks[i].clone();
        let now = mask.state_at(at);
        let status = mask
            .picks()
            .and_then(|_| {
                let r = compose::subject_ref(state.bus.project(), clip, &mask)?;
                Some(state.services.subjects.as_ref()?.status(r))
            })
            .or_else(|| {
                let r = compose::person_ref(state.bus.project(), clip, &mask)?;
                Some(state.services.subjects.as_ref()?.person_status(r))
            });
        let (id, masks) = (clip.id, clip.masks.clone());
        let set = move |change: Box<Change>| {
            let masks = masks.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                let mut masks = masks.clone();
                change(&mut masks);
                this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
            }
        };
        // A switch row editing the mask list.
        let switch = |name: &'static str, on: bool, change: Box<Change>, cx: &mut Context<Self>| {
            let apply = set(change);
            row(
                label(name),
                Switch::new((name, i))
                    .checked(on)
                    .on_click(cx.listener(move |this, _: &bool, _, cx| apply(this, cx))),
                None,
            )
        };
        let panel = cx.entity();
        let current = mask.mode;
        let mode_masks = clip.masks.clone();
        let mut rows = vec![row(
            label("Mode"),
            select_field(("mask-mode", i), current.label()).dropdown_menu(move |mut menu, _, _| {
                for mode in MaskMode::ALL {
                    let (panel, masks) = (panel.clone(), mode_masks.clone());
                    menu = menu.item(
                        PopupMenuItem::new(mode.label())
                            .checked(mode == current)
                            .on_click(move |_, _, cx| {
                                let mut masks = masks.clone();
                                masks[i].mode = mode;
                                panel.update(cx, |this, cx| {
                                    this.run(
                                        vec![Command::SetMasks { clip: id, masks }],
                                        "set_masks",
                                        cx,
                                    )
                                })
                            }),
                    );
                }
                menu
            }),
            None,
        )];
        rows.push(switch(
            "Invert",
            mask.invert,
            Box::new(move |m| m[i].invert = !m[i].invert),
            cx,
        ));
        if let MaskShape::Path { smooth, .. } = &mask.state.shape {
            rows.push(switch(
                "Smooth",
                *smooth,
                Box::new(move |m| {
                    let toggle = |shape: &mut MaskShape| {
                        if let MaskShape::Path { smooth, .. } = shape {
                            *smooth = !*smooth;
                        }
                    };
                    toggle(&mut m[i].state.shape);
                    for k in &mut m[i].keyframes {
                        toggle(&mut k.state.shape);
                    }
                }),
                cx,
            ));
        }
        if let Some(subject) = mask.picks().cloned() {
            rows.push(switch(
                "Track",
                subject.track,
                Box::new(move |m| {
                    if let Some(s) = m[i].subject.as_mut() {
                        s.track = !s.track;
                    }
                }),
                cx,
            ));
            rows.push(switch(
                "Refine edge",
                subject.refine,
                Box::new(move |m| {
                    if let Some(s) = m[i].subject.as_mut() {
                        s.refine = !s.refine;
                    }
                }),
                cx,
            ));
        }
        let tracking = status.as_ref().and_then(|s| {
            let track = mask.picks().is_some_and(|m| m.track) || (mask.is_person() && s.tracking);
            let busy = s.tracking || (track && s.tracked < s.frames);
            (track && busy && s.error.is_none()).then(|| {
                let fraction = s.tracked as f32 / s.frames.max(1) as f32;
                wide(Progress::new().value(fraction))
            })
        });
        let error = status.and_then(|s| s.error).map(error_line);

        let mut fields = match now.shape {
            MaskShape::Depth { .. } => vec![
                MaskField::Near,
                MaskField::Far,
                MaskField::Softness,
                MaskField::Opacity,
            ],
            _ => vec![MaskField::Feather, MaskField::Expansion, MaskField::Opacity],
        };
        match now.shape {
            MaskShape::Rectangle { .. } => {
                fields.extend([MaskField::Rotation, MaskField::Roundness])
            }
            MaskShape::Ellipse { .. } => fields.push(MaskField::Rotation),
            MaskShape::Person { .. } => fields.push(MaskField::Edge),
            MaskShape::Path { .. } | MaskShape::Subject | MaskShape::Depth { .. } => {}
        }
        let keyed = mask
            .keyframes
            .iter()
            .any(|k| (k.at - at).0.abs() < frame.0 / 2 + 1);
        for field in fields {
            let key = Key::Mask(i, field);
            rows.push(self.param_row(key, field.get(&now), keyed, None, window, cx));
        }

        let keyframe = if keyed {
            Keyframe::On
        } else if mask.keyframes.is_empty() {
            Keyframe::None
        } else {
            Keyframe::Off
        };
        let key_state = now.clone();
        let toggle_key = set(Box::new(move |m| {
            let keys = &mut m[i].keyframes;
            if keyed {
                keys.retain(|k| (k.at - at).0.abs() > frame.0 / 2);
            } else {
                keys.push(MaskKeyframe {
                    at,
                    state: key_state.clone(),
                    ease: Default::default(),
                });
                keys.sort_by_key(|k| k.at);
            }
        }));
        let half = Time(frame.0 / 2);
        let start = clip.start;
        let prev = mask
            .keyframes
            .iter()
            .map(|k| k.at)
            .filter(|t| *t < at - half)
            .max();
        let next = mask
            .keyframes
            .iter()
            .map(|k| k.at)
            .filter(|t| *t > at + half)
            .min();
        let panel = cx.entity();
        let seek = |to: Option<Time>| {
            let panel = panel.clone();
            to.map(|to| {
                move |_: &mut Window, cx: &mut App| {
                    panel.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| s.seek(start + to, cx))
                    })
                }
            })
        };
        let (prev, next) = (seek(prev), seek(next));
        let toggle_panel = panel.clone();
        let nav = KeyframeNav::new(("mask-key", i), keyframe)
            .disabled(!inside)
            .on_toggle(move |_, cx| toggle_panel.update(cx, |this, cx| toggle_key(this, cx)))
            .when_some(prev, |nav, f| nav.on_prev(f))
            .when_some(next, |nav, f| nav.on_next(f));
        let mask_id = mask.id;
        let to_path = mask.picks().is_some().then(|| {
            Button::new(("mask-to-path", i))
                .ghost()
                .xsmall()
                .icon(IconName::PenTool)
                .label("Convert to path")
                .on_click(cx.listener(move |this, _, _, cx| {
                    let state = this.state.read(cx);
                    let project = state.bus.project();
                    let Some((_, clip)) = project.timeline.clip(id) else {
                        return;
                    };
                    let mattes =
                        compose::subject_mattes(project, &state.services, clip, state.playhead);
                    let points = mattes
                        .iter()
                        .find(|(m, _)| *m == mask_id)
                        .and_then(|(_, matte)| matte.outline_path(PATH_POINTS));
                    let (Some(points), Some(index)) =
                        (points, clip.masks.iter().position(|m| m.id == mask_id))
                    else {
                        return;
                    };
                    let mut masks = clip.masks.clone();
                    let old = &masks[index];
                    masks[index] = Mask {
                        state: MaskState {
                            shape: MaskShape::Path {
                                points,
                                smooth: false,
                            },
                            ..old.state.clone()
                        },
                        keyframes: Vec::new(),
                        subject: None,
                        ..old.clone()
                    };
                    this.run(vec![Command::SetMasks { clip: id, masks }], "set_masks", cx);
                }))
        });
        let footer = h_flex()
            .flex_none()
            .h(px(30.))
            .pl(px(16.))
            .pr(px(10.))
            .gap(px(6.))
            .child(nav)
            .child(label("Keyframe shape"))
            .child(div().flex_1())
            .children(to_path)
            .child(
                Button::new(("mask-remove", i))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Trash)
                    .label("Remove")
                    .on_click({
                        let remove = set(Box::new(move |m| {
                            m.remove(i);
                        }));
                        cx.listener(move |this, _, _, cx| remove(this, cx))
                    }),
            );
        v_flex()
            .pt(px(2.))
            .children(tracking)
            .children(error)
            .children(rows)
            .child(footer)
    }
}
