//! Read-only current/next-disc guidance over the existing typed media swap plan.
//! Current disc is a person's selection, never observed emulator state.
use super::media_sets::SetView;
use archivefs_core::media_set::{
    MediaFamily, MediaSet, MediaSetConfidence, MediaSetState, MediaSwapPlan, MediaSwapStep,
    OrdinalUnit,
};
use eframe::egui;

pub(super) fn order(set: &MediaSet, plan: &MediaSwapPlan) -> Vec<MediaSwapStep> {
    if set.family != Some(MediaFamily::Optical)
        || set.state != MediaSetState::CompleteSet
        || !set.identity.verified
        || set.confidence != MediaSetConfidence::Proven
        || !plan.blockers.is_empty()
        || plan.ordered_media.len() != set.members.len()
        || plan.ordered_media.len() < 2
        || plan.ordered_media.iter().enumerate().any(|(index, step)| {
            step.side.is_some()
                || step.preferred_representation.is_none()
                || !step.ordinal.as_ref().is_some_and(|ordinal| {
                    ordinal.unit == OrdinalUnit::Disc && usize::from(ordinal.number) == index + 1
                })
        })
    {
        return Vec::new();
    }
    plan.ordered_media.clone()
}

fn next_disc(
    order: &[MediaSwapStep],
    current: u16,
) -> Result<Option<&MediaSwapStep>, &'static str> {
    let index = order
        .iter()
        .position(|step| step.ordinal.as_ref().is_some_and(|o| o.number == current))
        .ok_or("Choose a disc from the current checked order.")?;
    Ok(order.get(index + 1))
}

fn selection_key(set: &SetView) -> egui::Id {
    egui::Id::new((
        "next-disc-choice",
        &set.platform,
        &set.title,
        format!("{:?}", set.disc_order),
    ))
}

pub(super) fn show(ui: &mut egui::Ui, set: &SetView, fresh: bool) {
    ui.strong("Next disc guidance");
    if !fresh {
        ui.label("Next-disc guidance is unavailable until the current library check finishes successfully.");
        return;
    }
    if set.disc_order.is_empty() {
        ui.label("The next disc cannot be confirmed from this set. Review missing, conflicting or unverified evidence above.");
        return;
    }
    ui.label(
        "Choose the disc you are using. EmuWiz does not observe the emulator's inserted disc.",
    );
    // A changed ordered member/path selection gets fresh UI state. This is a
    // person's advisory context, never an approval token or launch request.
    let key = selection_key(set);
    let mut current = ui.ctx().data_mut(|data| data.get_temp::<u16>(key));
    egui::ComboBox::from_id_salt(key)
        .selected_text(
            current.map_or_else(|| "Choose the current disc".into(), |n| format!("Disc {n}")),
        )
        .show_ui(ui, |ui| {
            for step in &set.disc_order {
                if let Some(ordinal) = &step.ordinal {
                    ui.selectable_value(
                        &mut current,
                        Some(ordinal.number),
                        format!("Disc {}", ordinal.number),
                    );
                }
            }
        });
    if let Some(number) = current {
        ui.ctx().data_mut(|data| data.insert_temp(key, number));
        match next_disc(&set.disc_order, number) {
            Ok(Some(step)) => {
                ui.label(format!(
                    "Next in the checked order: Disc {}",
                    step.ordinal.as_ref().unwrap().number
                ));
                if let Some(source) = &step.preferred_representation {
                    ui.add(egui::Label::new(source.path.display().to_string()).wrap());
                }
            }
            Ok(None) => {
                ui.label("This is the final disc in the checked order; no next disc is listed.");
            }
            Err(reason) => {
                ui.label(reason);
            }
        }
    }
    ui.label("Guidance from the current catalogue snapshot only. Emulator disc-switching capability and save/state compatibility have not been checked. Use the emulator's own controls when the game prompts; existing launch preflight still rechecks files.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::media_set::*;

    fn fixture(numbers: &[u16], total: u16, trusted: bool) -> MediaSet {
        let records = numbers
            .iter()
            .map(|number| {
                let mut record = media_record(
                    format!("/private/Clockwork (Disc {number} of {total}).iso"),
                    Some("PlayStation"),
                );
                record.availability = MediaAvailability::Observed;
                if trusted {
                    let mut evidence =
                        MediaEvidence::new(EvidenceKind::TrustedDat, "synthetic checked catalogue");
                    evidence.release = Some(IdentityKey::new("fixture-release", "clockwork"));
                    evidence.medium = Some(IdentityKey::new("fixture-medium", number.to_string()));
                    evidence.equivalence = Equivalence::AuthorityMapping;
                    evidence.ordinal = Some(MediaOrdinal {
                        number: *number,
                        unit: OrdinalUnit::Disc,
                    });
                    evidence.expected_count = Some(ExpectedCount {
                        count: total,
                        unit: OrdinalUnit::Disc,
                    });
                    record.evidence.push(evidence);
                }
                record
            })
            .collect();
        resolve_index(index_media(records)).sets.remove(0)
    }

    #[test]
    fn two_and_four_disc_guidance_uses_the_explicit_current_disc_and_final_disc() {
        for total in [2, 4] {
            let set = fixture(&(1..=total).rev().collect::<Vec<_>>(), total, true);
            let choices = order(&set, &media_swap_plan(&set, None));
            assert_eq!(choices.len(), usize::from(total));
            assert!(next_disc(&choices, 0).is_err());
            for current in 1..total {
                let next = next_disc(&choices, current).unwrap().unwrap();
                assert_eq!(next.ordinal.as_ref().unwrap().number, current + 1);
                assert!(
                    next.preferred_representation
                        .as_ref()
                        .unwrap()
                        .path
                        .to_string_lossy()
                        .contains(&format!("Disc {}", current + 1))
                );
            }
            assert!(next_disc(&choices, total).unwrap().is_none());
        }
    }

    #[test]
    fn filenames_missing_unavailable_and_conflicting_evidence_never_offer_next_disc() {
        for set in [fixture(&[1, 2], 2, false), fixture(&[1, 3], 3, true)] {
            assert!(order(&set, &media_swap_plan(&set, None)).is_empty());
        }
        let mut set = fixture(&[1, 2], 2, true);
        set.members[1].representations[0].record.availability = MediaAvailability::Missing;
        assert!(order(&set, &media_swap_plan(&set, None)).is_empty());
        let mut set = fixture(&[1, 2], 2, true);
        set.members[1].ordinal = set.members[0].ordinal.clone();
        assert!(order(&set, &media_swap_plan(&set, None)).is_empty());
        let mut set = fixture(&[1, 2], 2, true);
        set.identity.verified = false;
        assert!(order(&set, &media_swap_plan(&set, None)).is_empty());
    }
    #[test]
    fn ui_requires_explicit_current_disc_and_never_claims_to_switch_it() {
        fn render(ctx: &egui::Context, view: &SetView, fresh: bool) -> String {
            fn text(shape: &egui::Shape, out: &mut String) {
                match shape {
                    egui::Shape::Text(t) => {
                        out.push_str(t.galley.text());
                        out.push('\n');
                    }
                    egui::Shape::Vec(shapes) => {
                        for s in shapes {
                            text(s, out);
                        }
                    }
                    _ => {}
                }
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| show(ui, view, fresh));
                },
            );
            let mut result = String::new();
            for shape in output.shapes {
                text(&shape.shape, &mut result);
            }
            result
        }
        let set = fixture(&[1, 2, 3, 4], 4, true);
        let view = super::super::media_sets::view_of(&set, &std::collections::HashMap::new());
        let ctx = egui::Context::default();
        let initial = render(&ctx, &view, true);
        assert!(initial.contains("Choose the current disc"));
        assert!(!initial.contains("Next in the checked order:"));
        ctx.data_mut(|data| data.insert_temp(selection_key(&view), 2_u16));
        let next = render(&ctx, &view, true);
        assert!(next.contains("Next in the checked order: Disc 3"));
        assert!(next.contains("does not observe"));
        assert!(next.contains("have not been checked"));
        let stale = render(&ctx, &view, false);
        assert!(stale.contains("until the current library check finishes"));
        assert!(!stale.contains("Next in the checked order:"));
        ctx.data_mut(|data| data.insert_temp(selection_key(&view), 4_u16));
        assert!(render(&ctx, &view, true).contains("final disc in the checked order"));
        let unverified = fixture(&[1, 2], 2, false);
        let view =
            super::super::media_sets::view_of(&unverified, &std::collections::HashMap::new());
        let refusal = render(&ctx, &view, true);
        assert!(refusal.contains("cannot be confirmed"));
        assert!(!refusal.contains("Next in the checked order:"));
    }
}
