use crate::{
    DisplayPoint, DisplayRow, EditorSettings, RowExt, ScrollAnchor, WorkspaceId,
    display_map::{DisplaySnapshot, ToDisplayPoint},
};
use gpui::{App, Point, point};
use language::Bias;
use settings::Settings;
use std::time::{Duration, Instant};

pub(crate) enum UpdateResponse {
    Finished {
        destination_anchor: ScrollAnchor,
        destination_top_row: u32,
        state: PersistentState,
    },
    Nothing,
    RequiresAnimationFrame {
        intermediate_anchor: ScrollAnchor,
        intermediate_top_row: u32,
    },
}

#[derive(Clone)]
pub(crate) struct PersistentState {
    pub(crate) map: DisplaySnapshot,
    pub(crate) workspace_id: Option<WorkspaceId>,
    pub(crate) local: bool,
    pub(crate) autoscroll: bool,
}

pub(crate) struct Anim {
    start: f32,
    length: f32,
    destination_top_row: u32,
    destination_anchor: ScrollAnchor,
    start_moment: Instant,
    state: PersistentState,
    animation_time:Duration,
    spring:CriticallyDampedSpringAnimation,
}

impl Anim {
    pub(crate) fn new(
        from: Point<f32>,
        destination_top_row: u32,
        destination_anchor: ScrollAnchor,
        map: DisplaySnapshot,
        workspace_id: Option<WorkspaceId>,
        local: bool,
        autoscroll: bool,
    ) -> Anim {
        let mut spring=CriticallyDampedSpringAnimation::new();
        let start = from.y;
        let end = destination_anchor.offset.y
            + destination_anchor
                .anchor
                .to_display_point(&map)
                .row()
                .as_f32();
        let delta = end - start;
        spring.position=delta;

        Anim {
            start,
            length: delta,
            destination_top_row,
            destination_anchor,
            start_moment: Instant::now(),
            state: PersistentState {
                map,
                workspace_id,
                local,
                autoscroll,
            },
            spring:spring,
            animation_time:Duration::ZERO,
        }
    }
}

pub(crate) struct ScrollAnimationManager {
    anim: Option<Anim>,
    scroll_duration: f32,
}

impl ScrollAnimationManager {
    pub(crate) fn new(cx: &App) -> Self {
        ScrollAnimationManager {
            anim: None,
            scroll_duration: EditorSettings::get_global(cx)
                .smooth_scroll_duration
                .max(0.),
        }
    }

    pub(crate) fn start(&mut self, anim: Anim) {
        self.anim = Some(anim);
    }

    pub(crate) fn set_duration(&mut self, new_dur: f32) {
        self.scroll_duration = new_dur.max(0.);
    }

    pub(crate) fn has_anim(&self) -> bool {
        self.anim.is_some()
    }

    pub(crate) fn get_state(&self) -> Option<PersistentState> {
        self.anim.as_ref().map(|v| v.state.clone())
    }

    fn make_final_results(
        &self,
        intermediate_scroll_top: f32,
    ) -> (ScrollAnchor, u32) {
        // the logic here is roughly the same as what you'd find in
        // [ScrollManager::set_scroll_position()]
        // the idea is to build objects that [ScrollManager::set_anchor()] can exploit
        // using our calculated intermediate_scroll_top
        let map= &self.anim.as_ref().unwrap().state.map;
        let scroll_top_buffer_point =
            DisplayPoint::new(DisplayRow(intermediate_scroll_top as u32), 0).to_point(map);
        let new_top_anchor = map
            .buffer_snapshot
            .anchor_at(scroll_top_buffer_point, Bias::Right);

        (
            ScrollAnchor {
                anchor: new_top_anchor,
                offset: point(
                    // no horizontal scrolling yet ...
                    self.anim.as_ref().unwrap().destination_anchor.offset.x,
                    intermediate_scroll_top - new_top_anchor.to_display_point(map).row().as_f32(),
                ),
            },
            scroll_top_buffer_point.row,
        )
    }
//1/120
//1 2
//anim.delta 是最终要挪动的距离
//
    pub(crate) fn update(&mut self) -> UpdateResponse {
        if let Some(anim) = &mut self.anim {
            let now= Instant::now();
            let target_animation_time = now-anim.start_moment;
            let mut delta = target_animation_time.saturating_sub(anim.animation_time);
            if target_animation_time.as_secs_f32() >= self.scroll_duration {
                let anim = self.anim.take().unwrap();
                UpdateResponse::Finished {
                    destination_top_row: anim.destination_top_row,
                    destination_anchor: anim.destination_anchor,
                    state: anim.state,
                }
            } else {
                let mut dt =Duration::from_secs_f32(1.0/120.0);
                if delta > Duration::from_millis(1000) {
                    anim.start_moment = now;
                    anim.animation_time = Duration::ZERO;
                    delta = dt;
                }
                let mut catchup = if delta >= dt {
                    delta
                } else {
                    delta.div_f64(10.0)
                };
                dt+=catchup;
                anim.animation_time+=dt;
                anim.spring.update(dt.as_secs_f32(), 0.1);
                let new_scroll_top =
                    anim.start + anim.length -anim.spring.position;
                let (intermediate_anchor, intermediate_top_row) =
                    self.make_final_results(new_scroll_top);

                UpdateResponse::RequiresAnimationFrame {
                    intermediate_anchor,
                    intermediate_top_row,
                }
            }
        } else {
            UpdateResponse::Nothing
        }
    }
}

//copy from neovide
#[derive(Clone)]
pub struct CriticallyDampedSpringAnimation {
    pub position: f32,
    velocity: f32,
}

impl CriticallyDampedSpringAnimation {
    pub fn new() -> Self {
        Self {
            position: 0.0,
            velocity: 0.0,
        }
    }

    pub fn update(&mut self, dt: f32, animation_length: f32) -> bool {
        if animation_length <= dt {
            self.reset();
            return false;
        }
        if self.position == 0.0 {
            return false;
        }

        // Simulate a critically damped spring, also known as a PD controller.
        // For more details of why this was chosen, see this:
        // https://gdcvault.com/play/1027059/Math-In-Game-Development-Summit
        // < 1 underdamped,  1 critically damped, > 1 overdamped
        let zeta = 1.0;
        // The omega is calculated so that the destination is reached with a 2% tolerance in
        // animation_length time.
        let omega = 4.0 / (zeta * animation_length);

        // Use the analytica formula for critically damped harmonic oscillation
        // a and b are the intial conditions by setting dt to zero and solving the position and
        // velocity respectively
        let a = self.position;
        let b = self.position * omega + self.velocity;

        let c = (-omega * dt).exp();

        self.position = (a + b * dt) * c;
        self.velocity = c * (-a * omega - b * dt * omega + b);

        if self.position.abs() < 0.01 {
            self.reset();
            false
        } else {
            true
        }
    }

    pub fn reset(&mut self) {
        self.position = 0.0;
        self.velocity = 0.0;
    }
}

