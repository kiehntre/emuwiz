//! The controller's virtual mouse (Controller Everywhere follow-up).
//!
//! R3 toggles a visible pointer. The right stick moves it, A is a left click,
//! X a right click and B leaves the mode. Movement is shaped here (dead zone,
//! acceleration curve, frame-rate independent); `pad_shell` owns the toggle,
//! button consumption and drawing. With the mode off none of this runs and the
//! right stick sends nothing at all.

use eframe::egui;
use std::sync::{Arc, Mutex};

/// Radial dead zone, as a share of full deflection.
pub(super) const DEAD_ZONE: f32 = 0.18;
/// Pointer speed at full deflection, points per second.
pub(super) const MAX_SPEED: f32 = 1400.0;
/// Longest step simulated for one frame, so a stall never throws the pointer.
const MAX_STEP: f64 = 0.05;

/// Points per second for a stick deflection: nothing inside the dead zone,
/// then a squared curve so small pushes are precise and a full push is fast.
pub(super) fn speed(x: f32, y: f32) -> (f32, f32) {
    let magnitude = x.hypot(y);
    if magnitude <= DEAD_ZONE {
        return (0.0, 0.0);
    }
    let scaled = ((magnitude - DEAD_ZONE) / (1.0 - DEAD_ZONE)).min(1.0);
    let rate = MAX_SPEED * scaled * scaled;
    (x / magnitude * rate, y / magnitude * rate)
}

#[derive(Default)]
pub(super) struct VirtualMouse {
    pub(super) on: bool,
    pub(super) pos: egui::Pos2,
    stick: (f32, f32),
    last_time: Option<f64>,
    /// Buttons whose press this mode swallowed, so their release is swallowed
    /// too (and a release whose press the navigator saw still reaches it).
    pub(super) swallowed: Vec<super::big_picture::nav::Button>,
    pub(super) primary_down: bool,
    pub(super) secondary_down: bool,
}

impl VirtualMouse {
    pub(super) fn set_stick(&mut self, x: f32, y: f32) {
        self.stick = (x, y);
    }

    pub(super) fn moving(&self) -> bool {
        self.on && speed(self.stick.0, self.stick.1) != (0.0, 0.0)
    }

    /// Advance to `now`; the new position when it moved.
    pub(super) fn step(&mut self, now: f64, screen: egui::Rect) -> Option<egui::Pos2> {
        let dt = self
            .last_time
            .map_or(0.0, |t| (now - t).clamp(0.0, MAX_STEP));
        self.last_time = Some(now);
        if !self.on {
            return None;
        }
        let (vx, vy) = speed(self.stick.0, self.stick.1);
        if vx == 0.0 && vy == 0.0 {
            return None;
        }
        let moved = self.pos + egui::vec2(vx, vy) * dt as f32;
        let clamped = egui::pos2(
            moved.x.clamp(screen.left(), screen.right() - 1.0),
            moved.y.clamp(screen.top(), screen.bottom() - 1.0),
        );
        (clamped != self.pos).then(|| {
            self.pos = clamped;
            clamped
        })
    }

    pub(super) fn reset_motion(&mut self) {
        self.stick = (0.0, 0.0);
        self.last_time = None;
    }
}

/// Feeds the pointer's events into egui's raw input at the start of the next
/// frame: egui reads pointer state only from raw input, so events pushed
/// mid-frame would never move it or click anything.
pub(super) struct Injector(pub(super) Arc<Mutex<Vec<egui::Event>>>);

impl egui::Plugin for Injector {
    fn debug_name(&self) -> &'static str {
        "emuwiz_virtual_pointer"
    }

    fn input_hook(&mut self, input: &mut egui::RawInput) {
        let mut queued = self.0.lock().unwrap_or_else(|e| e.into_inner());
        input.events.append(&mut queued);
    }
}

type Queue = Arc<Mutex<Vec<egui::Event>>>;

fn shared(context: &egui::Context) -> Queue {
    context.data_mut(|d| {
        d.get_temp_mut_or_insert_with::<Queue>(egui::Id::new("pad_mouse_queue"), Default::default)
            .clone()
    })
}

pub(super) fn queue(context: &egui::Context, events: impl IntoIterator<Item = egui::Event>) {
    shared(context)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(events);
}

/// Once per context.
pub(super) fn register_injector(context: &egui::Context) {
    context.add_plugin(Injector(shared(context)));
}

/// An arrow with a dark outline so it shows on any background.
pub(super) fn paint(context: &egui::Context, pos: egui::Pos2) {
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("pad_mouse_pointer"),
    ));
    let points = vec![
        pos,
        pos + egui::vec2(0.0, 22.0),
        pos + egui::vec2(5.5, 17.0),
        pos + egui::vec2(9.5, 26.0),
        pos + egui::vec2(13.0, 24.5),
        pos + egui::vec2(9.0, 16.0),
        pos + egui::vec2(16.0, 16.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        points.clone(),
        egui::Color32::WHITE,
        egui::Stroke::new(2.0_f32, egui::Color32::BLACK),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_moves_inside_the_dead_zone() {
        assert_eq!(speed(0.1, 0.05), (0.0, 0.0));
        assert_eq!(speed(0.0, DEAD_ZONE), (0.0, 0.0));
    }

    #[test]
    fn speed_accelerates_and_keeps_direction() {
        let (half, _) = speed(0.5, 0.0);
        let (full, _) = speed(1.0, 0.0);
        assert!(
            half > 0.0 && full > half * 3.0,
            "squared curve: {half} {full}"
        );
        assert!((full - MAX_SPEED).abs() < 1.0);
        let (x, y) = speed(-0.6, 0.6);
        assert!(x < 0.0 && y > 0.0 && (x.abs() - y.abs()).abs() < 0.001);
    }

    #[test]
    fn movement_is_frame_rate_independent_and_stays_on_screen() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0));
        let mut slow = VirtualMouse {
            on: true,
            pos: egui::pos2(100.0, 100.0),
            ..Default::default()
        };
        let mut fast = VirtualMouse {
            on: true,
            pos: egui::pos2(100.0, 100.0),
            ..Default::default()
        };
        slow.set_stick(1.0, 0.0);
        fast.set_stick(1.0, 0.0);
        slow.step(0.0, screen);
        fast.step(0.0, screen);
        for i in 1..=10 {
            slow.step(f64::from(i) * 0.02, screen); // 50 fps
        }
        for i in 1..=20 {
            fast.step(f64::from(i) * 0.01, screen); // 100 fps
        }
        assert!(
            (slow.pos.x - fast.pos.x).abs() < 1.0,
            "{:?} {:?}",
            slow.pos,
            fast.pos
        );
        for i in 21..200 {
            fast.step(f64::from(i) * 0.01, screen);
        }
        assert!(fast.pos.x <= 999.0, "clamped to the screen: {:?}", fast.pos);
    }

    #[test]
    fn a_stalled_frame_cannot_throw_the_pointer() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(5000.0, 5000.0));
        let mut m = VirtualMouse {
            on: true,
            pos: egui::pos2(0.0, 0.0),
            ..Default::default()
        };
        m.set_stick(1.0, 0.0);
        m.step(0.0, screen);
        m.step(30.0, screen); // the app was stuck for 30 s
        assert!(m.pos.x <= MAX_SPEED * 0.05 + 1.0, "{:?}", m.pos);
    }

    #[test]
    fn off_means_no_movement() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 500.0));
        let mut m = VirtualMouse::default();
        m.set_stick(1.0, 1.0);
        assert!(m.step(0.0, screen).is_none() && m.step(0.1, screen).is_none());
    }
}
