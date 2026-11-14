//! Answers handed back by background readiness workers.
//!
//! Several emulator checks run on their own thread for the selected game
//! (Mesen, shadPS4, Hatari, VICE, the MAME machine probe, the per-game
//! content inspections). Each leaves its answer in a channel that is only read the
//! next time the launch-readiness input is built, then wakes the interface.
//!
//! A worker calls [`arrived`] after sending its answer and before that
//! wake-up. Code that remembers a built input (see
//! `gui_v2/native_workflows/launch_input_memo.rs`) keys on [`count`], so the
//! frame the worker asked for builds again and collects the answer instead
//! of drawing the older one until a timer runs out.
//!
//! The count lives in the egui context the worker already holds, so two
//! windows, or two tests, never see each other's answers.

use eframe::egui;

fn slot() -> egui::Id {
    egui::Id::new("emuwiz-background-readiness-answers")
}

/// A background readiness worker has sent an answer that is waiting to be
/// collected. Call it before waking the interface.
pub(crate) fn arrived(context: &egui::Context) {
    context.data_mut(|data| {
        let count = data.get_temp_mut_or_default::<u64>(slot());
        *count = count.wrapping_add(1);
    });
}

/// How many answers have arrived in this context.
pub(crate) fn count(context: &egui::Context) -> u64 {
    context.data(|data| data.get_temp::<u64>(slot()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_counted_per_context() {
        let ours = egui::Context::default();
        let theirs = egui::Context::default();
        assert_eq!(count(&ours), 0);
        arrived(&ours);
        arrived(&ours.clone());
        assert_eq!(count(&ours), 2, "a clone is the same context");
        assert_eq!(count(&theirs), 0);
    }

    #[test]
    fn a_worker_thread_can_report_an_answer() {
        let context = egui::Context::default();
        let worker = context.clone();
        std::thread::spawn(move || arrived(&worker)).join().unwrap();
        assert_eq!(count(&context), 1);
    }
}
