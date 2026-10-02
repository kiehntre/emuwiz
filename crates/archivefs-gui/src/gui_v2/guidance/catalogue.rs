//! The authored script catalogue.
//!
//! Typed production data: the 42 designed scripts of
//! `docs/design/MR_WIZ_GUIDANCE_AND_AI_ASSISTANT_V1.md` section 11 (with their four
//! documented variants), and the 35 messages the pre-engine implementation
//! shipped. Wording is the approved wording; the stable IDs are the identity, so
//! copy can be revised without changing what an event *is*.
//!
//! Priorities follow the design's bands (section 6):
//!
//! | band | situation |
//! | ---- | --------- |
//! | 100  | current apply/launch refused by an authoritative safety check |
//! | 90   | a concrete missing prerequisite for the action being attempted |
//! | 80   | current operation failed/incomplete; conflicting identity; recheck needed |
//! | 70   | a current scoped warning with an available decision |
//! | 60   | completion of the user's current operation |
//! | 50   | a completed check found an empty state, setup gap or something to review |
//! | 40   | an optional explanation of a provider/source state |
//! | 10   | first-use or orientation help for an eligible task |
//!
//! Every design script requires at least one fact the page must supply, so none
//! can appear on a page merely because the page opened. The legacy messages
//! require only the *legacy* fact kinds projected from the pre-engine evidence
//! fields, so existing page output is unchanged until a page-owned adapter
//! supplies the design facts that replace them.

#![allow(dead_code)]

use super::model::FactKind as K;
use super::model::GuidanceAction as A;
use super::model::GuidanceCategory as C;
use super::model::GuidancePage as P;
use super::model::GuidanceScope as S;
use super::model::GuidanceTopic as T;
use super::model::MascotState as M;
use super::script::{BASE, GuidanceScript, RepeatPolicy as R, ScriptOrigin};

pub(crate) static CATALOGUE: &[GuidanceScript] = &[
    // --- 01 First run ---------------------------------------------------------------
    GuidanceScript {
        id: "first_run.choose_sources",
        design_number: Some(1),
        category: C::EmptyState,
        topics: &[T::FirstRun],
        pages: &[P::Home, P::Setup],
        requires: &[K::FreshEnvironment],
        priority: 50,
        exclusive_group: Some("first-run"),
        mascot: M::Helpful,
        quick: "Welcome. Choose where EmuWiz should look for your games; you’ll review the folders before scanning them.",
        minimal: Some("Choose game folders, then review and scan them."),
        explain: Some(
            "Start with folders you already use. EmuWiz can list what it finds without renaming or repairing your games. Emulator setup and optional artwork sources can come afterwards.",
        ),
        technical: Some(
            "Technical details show the active configuration root, the loaded source summary and the environment snapshot’s freshness.",
        ),
        action: Some(A::ChooseGameFolders),
        ..BASE
    },
    // --- 02 Home ---------------------------------------------------------------------
    GuidanceScript {
        id: "home.browse_ready",
        design_number: Some(2),
        category: C::Tip,
        topics: &[T::Home],
        pages: &[P::Home],
        requires: &[K::HomeFirstUseTipEligible],
        priority: 10,
        repeat: R::FirstUse,
        mascot: M::Neutral,
        quick: "Your game list is ready to browse. Select a title to see what EmuWiz knows about it and whether it is ready to play.",
        minimal: Some("Browse games to review identity and launch readiness."),
        explain: Some(
            "Being listed is not the same as being verified or ready to launch. Game Details keeps those checks separate, along with artwork and associated documents.",
        ),
        technical: Some(
            "Technical details show the current catalogue counts and load revision, not a library dump.",
        ),
        action: Some(A::BrowseGames),
        ..BASE
    },
    // --- 03/04 Sources ---------------------------------------------------------------
    GuidanceScript {
        id: "sources.none_configured",
        design_number: Some(3),
        category: C::EmptyState,
        topics: &[T::Sources],
        pages: &[P::Sources, P::Home],
        requires: &[K::SourcesNoneConfigured],
        priority: 50,
        exclusive_group: Some("source-state"),
        mascot: M::Helpful,
        quick: "No game folders are configured yet. Add a folder so EmuWiz knows where to look.",
        minimal: Some("No game folders configured. Add a source."),
        explain: Some(
            "Choose a folder containing your own collection. Adding its location is separate from scanning it, and does not move the files.",
        ),
        technical: Some(
            "Technical details show the source-configuration status and the active configuration location.",
        ),
        action: Some(A::AddGameFolder),
        ..BASE
    },
    GuidanceScript {
        id: "sources.unavailable",
        design_number: Some(4),
        category: C::Warning,
        topics: &[T::Sources],
        pages: &[P::Sources],
        scope: S::Source,
        requires: &[K::SourceUnavailable],
        priority: 70,
        exclusive_group: Some("source-state"),
        repeat: R::CollapsibleWarning,
        mascot: M::Warning,
        quick: "This game folder is unavailable. Check that its drive is connected, then review the folder location.",
        minimal: Some("Source unavailable; catalogue retained. Review its location."),
        explain: Some(
            "The catalogue entry has been kept. An unavailable folder does not prove the games were deleted; the drive may be disconnected or its mount location may have changed.",
        ),
        technical: Some(
            "Technical details show the exact path, the availability reason and the last successful scan, if known.",
        ),
        action: Some(A::ReviewGameFolder),
        ..BASE
    },
    // --- 05/06 Scanning --------------------------------------------------------------
    GuidanceScript {
        id: "scan.running",
        design_number: Some(5),
        category: C::Explain,
        topics: &[T::Scanning],
        scope: S::Operation,
        requires: &[K::ScanRunning],
        priority: 50,
        mascot: M::Thinking,
        quick: "EmuWiz is reading your game folders and updating the list. You can keep browsing while the scan runs.",
        minimal: Some("Scan running. View Activity for progress."),
        explain: Some(
            "This scan does not rename or repair your original games. Progress reflects what the scanner has reported; there is no completion estimate unless one is available.",
        ),
        technical: Some(
            "Technical details show the job ID, the current item, the reported counts and whether cancellation is supported.",
        ),
        action: Some(A::ViewScanProgress),
        ..BASE
    },
    GuidanceScript {
        id: "scan.partial_failure",
        design_number: Some(6),
        category: C::Warning,
        topics: &[T::Scanning, T::Warnings],
        pages: &[P::Sources, P::Activity],
        scope: S::Operation,
        requires: &[K::ScanPartialFailure],
        excludes: &[K::ScanRunning],
        priority: 70,
        repeat: R::CollapsibleWarning,
        mascot: M::Warning,
        quick: "The scan finished, but {folder_count} {folder_count?folder|folders} could not be read. Review {folder_count?that folder|those folders} before relying on the new list as complete.",
        minimal: Some(
            "Scan incomplete for {folder_count} {folder_count?folder|folders}; previous entries retained.",
        ),
        explain: Some(
            "EmuWiz kept the previous entries for {folder_count?that folder|those folders}. Reconnect the drive or correct the location, then run the existing scan again when you are ready.",
        ),
        technical: Some(
            "Technical details show each folder’s error, the scan receipt and time, and the evidence of retained entries.",
        ),
        action: Some(A::ReviewUnavailableFolders),
        ..BASE
    },
    // --- 07-10 Identity and DATs -----------------------------------------------------
    GuidanceScript {
        id: "dat.none_available",
        design_number: Some(7),
        category: C::WhyBlocked,
        topics: &[T::IdentityDat],
        pages: &[P::DatManagement, P::CheckGames],
        scope: S::Operation,
        requires: &[K::DatNoneAvailable],
        priority: 90,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "There is no usable DAT for this check yet. Add or review a DAT source so EmuWiz can compare these games with a reference catalogue.",
        minimal: Some("No usable DAT for this check. Manage DAT sources."),
        explain: Some(
            "A DAT lists known releases and file checksums. Without a suitable one, this DAT-based check cannot confirm a match. Other evidence and ordinary browsing remain available.",
        ),
        technical: Some(
            "Technical details show the requested platform, the DAT inventory status and any parser or import errors.",
        ),
        action: Some(A::ManageDats),
        ..BASE
    },
    GuidanceScript {
        id: "identity.unknown",
        design_number: Some(8),
        category: C::WhyBlocked,
        topics: &[T::IdentityDat, T::UnknownGame],
        pages: &[P::Games, P::CheckGames, P::DatManagement, P::Organisation],
        scope: S::Game,
        requires: &[K::IdentityUnknown, K::OperationNeedsStrongerIdentity],
        priority: 80,
        exclusive_group: Some("identity"),
        repeat: R::Blocker,
        mascot: M::Thinking,
        quick: "EmuWiz cannot safely identify this game yet. Review its evidence before choosing a rename or repair.",
        minimal: Some("Identity not confirmed. Review evidence before rename or repair."),
        explain: Some(
            "A filename can suggest a title, but it is not enough to justify a destructive change. A matching checksum or other supported identification evidence gives EmuWiz a firmer basis.",
        ),
        technical: Some(
            "Technical details show the report status, the inspected format, the available checksums, the candidate evidence and the DAT provenance.",
        ),
        action: Some(A::ReviewGameEvidence),
        ..BASE
    },
    // Variant of 08: with no blocked operation the same facts are an explanation,
    // never a launch blocker inferred from identity alone.
    GuidanceScript {
        id: "identity.unknown.explain",
        variant_of: Some("identity.unknown"),
        category: C::Explain,
        topics: &[T::IdentityDat, T::UnknownGame],
        pages: &[P::Games, P::CheckGames, P::DatManagement, P::Organisation],
        scope: S::Game,
        requires: &[K::IdentityUnknown],
        excludes: &[K::OperationNeedsStrongerIdentity],
        priority: 50,
        exclusive_group: Some("identity"),
        mascot: M::Thinking,
        quick: "EmuWiz cannot safely identify this game yet. Review its evidence before choosing a rename or repair.",
        minimal: Some("Identity not confirmed. Review evidence before rename or repair."),
        explain: Some(
            "A filename can suggest a title, but it is not enough to justify a destructive change. A matching checksum or other supported identification evidence gives EmuWiz a firmer basis.",
        ),
        technical: Some(
            "Technical details show the report status, the inspected format, the available checksums, the candidate evidence and the DAT provenance.",
        ),
        action: Some(A::ReviewGameEvidence),
        ..BASE
    },
    GuidanceScript {
        id: "identity.candidate_only",
        design_number: Some(9),
        category: C::Explain,
        topics: &[T::IdentityDat],
        pages: &[P::Games, P::CheckGames, P::DatManagement, P::Organisation],
        scope: S::Game,
        requires: &[K::IdentityCandidateOnly],
        priority: 50,
        exclusive_group: Some("identity"),
        mascot: M::Thinking,
        quick: "This may be {title}, but the match has not been verified. Review the evidence before using that name for a change.",
        minimal: Some("Possible match: {title}. Not verified; review before changing files."),
        explain: Some(
            "A possible match is a lead, perhaps from a name or provider record. A verified match has passed the checks required by the relevant EmuWiz workflow. The two are not interchangeable.",
        ),
        technical: Some(
            "Technical details show the candidate’s source, the comparison method, the missing verification requirement and the report revision.",
        ),
        action: Some(A::ReviewPossibleMatch),
        ..BASE
    },
    GuidanceScript {
        id: "identity.conflicting_evidence",
        design_number: Some(10),
        category: C::Warning,
        topics: &[T::IdentityDat, T::ConflictingEvidence],
        pages: &[P::Games, P::CheckGames, P::DatManagement, P::Organisation],
        scope: S::Game,
        requires: &[K::IdentityConflicting],
        priority: 80,
        exclusive_group: Some("identity"),
        repeat: R::CollapsibleWarning,
        mascot: M::Warning,
        quick: "The available evidence points to different game releases. Review the conflicting results before choosing an identity.",
        minimal: Some("Identity evidence conflicts. Compare the candidates and local checks."),
        explain: Some(
            "Different DAT versions, revisions or provider records can disagree. EmuWiz will show their sources and any stronger local checks; neither a tidy filename nor a confident explanation settles the conflict.",
        ),
        technical: Some(
            "Technical details show every relevant candidate with its checksums, DAT versions and provenance, and the resolver outcome.",
        ),
        action: Some(A::CompareIdentityEvidence),
        ..BASE
    },
    // --- 11 Problems -----------------------------------------------------------------
    GuidanceScript {
        id: "problems.review_findings",
        design_number: Some(11),
        category: C::Explain,
        topics: &[T::Problems],
        pages: &[P::ProblemsRepair],
        requires: &[K::ProblemsActionable],
        priority: 50,
        mascot: M::Helpful,
        quick: "{finding_count?There is|There are} {finding_count} {finding_count?finding|findings} to review. Open {finding_count?it|one} to see the cause and any supported next step.",
        minimal: Some(
            "{finding_count} {finding_count?finding|findings}. Review the highest-priority item.",
        ),
        explain: Some(
            "A finding may mean a missing file, uncertain identity or a setup requirement. It does not mean every item is damaged or repairable. EmuWiz will explain the available action before a change.",
        ),
        technical: Some(
            "Technical details show the selected finding’s severity, category, evidence source and scope.",
        ),
        action: Some(A::ReviewFirstFinding),
        action_alternates: &[(K::NoFirstFindingTarget, Some(A::ReviewProblems))],
        ..BASE
    },
    // --- 12-14, 41 MAME --------------------------------------------------------------
    GuidanceScript {
        id: "mame.missing_members",
        design_number: Some(12),
        category: C::WhyBlocked,
        topics: &[T::Mame],
        pages: &[P::Organisation, P::Games],
        scope: S::Game,
        requires: &[K::MameMissingMembers],
        priority: 90,
        exclusive_group: Some("mame-set"),
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "This MAME set is missing {missing_count} required {missing_count?file|files}. Review the complete set in MAME before changing individual files.",
        minimal: Some(
            "MAME: {missing_count} required {missing_count?file|files} missing. Review the complete set.",
        ),
        explain: Some(
            "MAME games are often sets of related files, sometimes shared with another set. Renaming one ROM cannot supply a missing member and may break the set's relationships. MAME review keeps those relationships in view.",
        ),
        technical: Some(
            "Technical details show the target set, the parent and clone relationships, the missing members’ checksums, and any supported repair or reconstruction evidence.",
        ),
        action: Some(A::ReviewInMame),
        ..BASE
    },
    GuidanceScript {
        id: "mame.parent_dependency",
        design_number: Some(13),
        category: C::WhyBlocked,
        topics: &[T::Mame],
        pages: &[P::Organisation, P::Games],
        scope: S::Game,
        requires: &[K::MameParentDependencyMissing],
        priority: 90,
        exclusive_group: Some("mame-set"),
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "This clone needs files from parent set {parent}. Review that dependency in MAME.",
        minimal: Some("Missing parent dependency: {parent}. Review in MAME."),
        explain: Some(
            "Some related arcade games share files. A clone can be correctly named and still need its parent set. EmuWiz checks the available member evidence before suggesting a reconstruction.",
        ),
        technical: Some(
            "Technical details show the parent and clone IDs, the ownership and member evidence, availability and any reconstruction blockers.",
        ),
        action: Some(A::ReviewParentDependency),
        ..BASE
    },
    GuidanceScript {
        id: "mame.bad_dump_reference",
        design_number: Some(14),
        category: C::Explain,
        topics: &[T::Mame],
        pages: &[P::Organisation, P::Games],
        scope: S::Game,
        requires: &[K::MameBadDumpReference],
        priority: 50,
        exclusive_group: Some("mame-reference"),
        mascot: M::Helpful,
        quick: "The reference marks this as a known imperfect dump. That alone does not mean your copy has become damaged.",
        minimal: Some("BAD_DUMP in the reference. Review separately from local hash failures."),
        explain: Some(
            "BAD_DUMP describes the reference data's known limitation. It is different from your file failing a checksum comparison. This flag alone is not a reason to offer an ordinary repair.",
        ),
        technical: Some(
            "Technical details show the exact flag, the member, the DAT version, the local comparison result and any independently supported action.",
        ),
        action: Some(A::ReviewReferenceEvidence),
        ..BASE
    },
    GuidanceScript {
        id: "mame.no_dump_reference",
        design_number: Some(41),
        category: C::Explain,
        topics: &[T::Mame],
        pages: &[P::Organisation, P::Games],
        scope: S::Game,
        requires: &[K::MameNoDumpReference],
        priority: 50,
        exclusive_group: Some("mame-reference"),
        mascot: M::Helpful,
        quick: "The reference has no known dump for this member. EmuWiz cannot treat that as an ordinary missing file it can repair.",
        minimal: Some("NO_DUMP reference limitation. No ordinary repair is implied."),
        explain: Some(
            "NO_DUMP records a gap in the reference material. It is not a checksum failure in a file you already have, and it does not establish that a usable replacement exists.",
        ),
        technical: Some(
            "Technical details show the member, the exact flag, the DAT provenance and the separate local availability evidence.",
        ),
        action: Some(A::ReviewReferenceLimitation),
        ..BASE
    },
    // --- 15, 16, 40 Artwork ----------------------------------------------------------
    GuidanceScript {
        id: "artwork.cover_missing",
        design_number: Some(15),
        category: C::EmptyState,
        topics: &[T::ArtworkMetadata],
        pages: &[P::Artwork, P::Games],
        scope: S::Game,
        requires: &[K::ArtworkCoverMissing],
        excludes: &[K::ArtworkCachedUsable],
        priority: 50,
        exclusive_group: Some("artwork-cover"),
        mascot: M::Helpful,
        quick: "No usable cover has been matched to this game yet. Review the available artwork sources.",
        minimal: Some("No usable cover found. Review artwork sources."),
        explain: Some(
            "Missing cover art does not mean the game is unidentified or unplayable. The configured sources may simply have no usable image for this title.",
        ),
        technical: Some(
            "Technical details show the resolver’s candidates, provider availability, matching provenance and delivery status.",
        ),
        action: Some(A::ReviewArtworkSources),
        action_alternates: &[
            (K::IdentityPreventsArtworkMatch, Some(A::ReviewGameEvidence)),
            (K::ArtworkRefreshAvailable, Some(A::RefreshArtwork)),
        ],
        ..BASE
    },
    GuidanceScript {
        id: "artwork.stale_usable_cache",
        design_number: Some(16),
        category: C::Explain,
        topics: &[T::ArtworkMetadata],
        pages: &[P::Artwork, P::Games],
        scope: S::Game,
        requires: &[
            K::ArtworkCachedUsable,
            K::ArtworkCacheStale,
            K::ArtworkSourceUnavailable,
        ],
        priority: 50,
        exclusive_group: Some("artwork-cover"),
        mascot: M::Neutral,
        quick: "Using cached cover art. The source is currently unavailable, but this saved copy can still be displayed.",
        minimal: Some("Using a stale cached cover; source unavailable."),
        explain: Some(
            "A cached image is a local retained copy. It may be older than the source's current image; EmuWiz has not confirmed a newer one. There is no need to treat the usable copy as a failure.",
        ),
        technical: Some(
            "Technical details show the cache timestamp and key, the provider, the last error and the current winner’s evidence.",
        ),
        action: Some(A::ViewArtworkDetails),
        ..BASE
    },
    // Variant of 16: a stale copy whose source is available must not claim the
    // source is unavailable.
    GuidanceScript {
        id: "artwork.stale_usable_cache.source_available",
        variant_of: Some("artwork.stale_usable_cache"),
        category: C::Explain,
        topics: &[T::ArtworkMetadata],
        pages: &[P::Artwork, P::Games],
        scope: S::Game,
        requires: &[K::ArtworkCachedUsable, K::ArtworkCacheStale],
        excludes: &[K::ArtworkSourceUnavailable],
        priority: 50,
        exclusive_group: Some("artwork-cover"),
        mascot: M::Neutral,
        quick: "Using an older cached cover. You can review the artwork details and refresh it when ready.",
        minimal: Some("Using a stale cached cover."),
        explain: Some(
            "A cached image is a local retained copy. It may be older than the source's current image; EmuWiz has not confirmed a newer one. There is no need to treat the usable copy as a failure.",
        ),
        technical: Some(
            "Technical details show the cache timestamp and key, the provider, the last error and the current winner’s evidence.",
        ),
        action: Some(A::ViewArtworkDetails),
        ..BASE
    },
    GuidanceScript {
        id: "artwork.alternatives_available",
        design_number: Some(40),
        category: C::Explain,
        topics: &[T::ArtworkMetadata],
        pages: &[P::Artwork, P::Games],
        scope: S::Game,
        requires: &[K::ArtworkAlternatives],
        priority: 10,
        repeat: R::FirstUse,
        mascot: M::Neutral,
        quick: "Using {provider} for this image. {alternative_count} {alternative_count?alternative source is|alternative sources are} available to inspect.",
        minimal: Some(
            "Using {provider}; {alternative_count} {alternative_count?alternative|alternatives}. View provenance.",
        ),
        explain: Some(
            "Several sources can offer an image without anything being wrong. EmuWiz's existing precedence rules determine the current choice. Viewing alternatives here does not change those rules.",
        ),
        technical: Some(
            "Technical details show the winner, the candidate list, the precedence decision, the provenance and the delivery and cache evidence.",
        ),
        action: Some(A::ViewArtworkSources),
        ..BASE
    },
    // --- 17, 18 Emulator and firmware ------------------------------------------------
    GuidanceScript {
        id: "emulator.multiple_installations",
        design_number: Some(17),
        category: C::WhyBlocked,
        topics: &[T::EmulatorSetup],
        pages: &[P::EmulatorSetup, P::Launch],
        scope: S::Game,
        requires: &[K::EmulatorMultipleInstallations],
        priority: 90,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "{count_word} {emulator} installations were found. Review them and choose which one EmuWiz should use.",
        minimal: Some(
            "{count_word} {emulator} installations; no applicable choice. Review installations.",
        ),
        explain: Some(
            "Each installation can have its own version and configuration. Choosing one makes the launch target clear; finding {count_lower} does not mean any of them is broken.",
        ),
        technical: Some(
            "Technical details show each installation’s path, version and discovery source, and any existing binding or preference, without changing it.",
        ),
        action: Some(A::ReviewInstallations),
        ..BASE
    },
    GuidanceScript {
        id: "firmware.missing",
        design_number: Some(18),
        category: C::WhyBlocked,
        topics: &[T::BiosFirmware],
        pages: &[P::BiosFirmware, P::Launch, P::EmulatorSetup],
        scope: S::Game,
        requires: &[K::FirmwareMissing],
        priority: 90,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "This emulator needs system firmware, and EmuWiz has not found it in the checked locations. Review the BIOS folder.",
        minimal: Some("Required firmware not found in checked locations. Review BIOS / Firmware."),
        explain: Some(
            "BIOS or firmware is the small system software that the original console uses to start and operate. Some emulators need a copy. EmuWiz only needs to read the files to detect them; it does not need to modify them for this check.",
        ),
        technical: Some(
            "Technical details show the emulator, the required firmware kind, the checked paths, the accepted checksum evidence and the exact detection result.",
        ),
        action: Some(A::ReviewBiosFolder),
        action_alternates: &[(K::BiosFolderChooserAvailable, Some(A::ChooseBiosFolder))],
        ..BASE
    },
    // --- 19-21 Launch ----------------------------------------------------------------
    GuidanceScript {
        id: "launch.no_compatible_emulator",
        design_number: Some(19),
        category: C::WhyBlocked,
        topics: &[T::LaunchReadiness],
        pages: &[P::Launch, P::Games],
        scope: S::Game,
        requires: &[K::LaunchNoCompatibleEmulator],
        priority: 90,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "I can see the game, but I can’t safely launch it yet because no compatible emulator has been selected.",
        minimal: Some("Launch blocked: no compatible emulator selected. Choose one."),
        explain: Some(
            "The emulator needs to support this platform and the game's current representation. Review the available installations; EmuWiz will check readiness again after the selection.",
        ),
        technical: Some(
            "Technical details show the current launch plan, each candidate’s compatibility, the selection reason and any additional blockers.",
        ),
        action: Some(A::ChooseEmulator),
        ..BASE
    },
    GuidanceScript {
        id: "launch.ready",
        design_number: Some(20),
        category: C::Success,
        topics: &[T::LaunchReadiness, T::SuccessStates],
        pages: &[P::Launch, P::Games],
        scope: S::Game,
        requires: &[K::LaunchReady],
        excludes: &[K::LaunchWarningsUnacknowledged],
        priority: 60,
        mascot: M::Success,
        quick: "The current launch checks are clear. You can use Play when you’re ready.",
        minimal: Some("Ready for the current launch plan."),
        explain: Some(
            "These checks cover the evidence EmuWiz has for this launch. They do not guarantee that every part of the game will run correctly in the emulator.",
        ),
        technical: Some(
            "Technical details show the selected emulator, the media target, the firmware result, the evidence revision and the plan’s warnings (none for this message).",
        ),
        action: Some(A::ReviewLaunch),
        ..BASE
    },
    GuidanceScript {
        id: "launch.failed",
        design_number: Some(21),
        category: C::WhyBlocked,
        topics: &[T::FailedLaunch, T::RecoveryFromFailure],
        pages: &[P::Launch, P::Games],
        scope: S::Operation,
        requires: &[K::LaunchFailed],
        priority: 80,
        repeat: R::Blocker,
        mascot: M::Concerned,
        quick: "The game did not start: {plain_failure_reason}. Review the launch details before trying again.",
        minimal: Some("Launch failed: {plain_failure_reason}. Review details."),
        explain: Some(
            "This is the result of the launch attempt, not proof that the game files are damaged. The details show the emulator and failure reported. If the cause is unknown, say so rather than guessing.",
        ),
        technical: Some(
            "Technical details show the exact error, the exit status when available, the executable, the safe arguments, the relevant bounded log and the request ID.",
        ),
        action: Some(A::ReviewLaunchDetails),
        ..BASE
    },
    // --- 22, 23 Cheats ---------------------------------------------------------------
    GuidanceScript {
        id: "cheats.no_game_selected",
        design_number: Some(22),
        category: C::EmptyState,
        topics: &[T::Cheats],
        pages: &[P::CheatsMods],
        requires: &[K::NoGameSelected],
        priority: 50,
        mascot: M::Helpful,
        quick: "Select a game first. Then I can show you any cheats EmuWiz knows about for it.",
        minimal: Some("Select a game to view known cheats."),
        explain: Some(
            "Cheats can depend on the platform and game revision. Selecting the game gives EmuWiz the context needed to show relevant entries.",
        ),
        technical: Some(
            "Technical details: the current context has no selected game. That is not an error.",
        ),
        action: Some(A::ChooseGame),
        ..BASE
    },
    GuidanceScript {
        id: "cheats.none_known",
        design_number: Some(23),
        category: C::EmptyState,
        topics: &[T::Cheats],
        pages: &[P::CheatsMods],
        scope: S::Game,
        requires: &[K::CheatsNoneKnown],
        priority: 50,
        mascot: M::Neutral,
        quick: "No cheats are currently known for this game. You can keep browsing its details.",
        minimal: Some("No known cheats for this game."),
        explain: Some(
            "This is an empty result, not an error. It does not mean the game is broken or that no cheat exists anywhere.",
        ),
        technical: Some(
            "Technical details show the sources checked and the applicable revision evidence, where available.",
        ),
        action: Some(A::BackToGameDetails),
        ..BASE
    },
    // --- 24 Patches ------------------------------------------------------------------
    GuidanceScript {
        id: "patch.wrong_base",
        design_number: Some(24),
        category: C::WhyBlocked,
        topics: &[T::ModsPatches],
        pages: &[P::CheatsMods],
        scope: S::Operation,
        requires: &[K::PatchBaseMismatch, K::PatchRevisionMismatchKnown],
        priority: 100,
        exclusive_group: Some("patch-base"),
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "This patch expects a different game revision or region. EmuWiz has not modified your original file.",
        minimal: Some("Base ROM does not match the patch. Original unchanged."),
        explain: Some(
            "A patch describes changes to one particular starting file. Applying it to a different release can produce unusable output. Compare the patch's required checksum with the selected game; a similar filename is not enough.",
        ),
        technical: Some(
            "Technical details show the expected and actual base hashes, the required revision and region only if known, the patch format and the exact rejection.",
        ),
        action: Some(A::ReviewExpectedBase),
        ..BASE
    },
    // Variant of 24: if only a hash mismatch is known, do not guess the revision.
    GuidanceScript {
        id: "patch.wrong_base.hash_only",
        variant_of: Some("patch.wrong_base"),
        category: C::WhyBlocked,
        topics: &[T::ModsPatches],
        pages: &[P::CheatsMods],
        scope: S::Operation,
        requires: &[K::PatchBaseMismatch],
        excludes: &[K::PatchRevisionMismatchKnown],
        priority: 100,
        exclusive_group: Some("patch-base"),
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "This patch expects a different starting file. EmuWiz has not modified your original file.",
        minimal: Some("Base ROM does not match the patch. Original unchanged."),
        explain: Some(
            "A patch describes changes to one particular starting file. Applying it to a different release can produce unusable output. Compare the patch's required checksum with the selected game; a similar filename is not enough.",
        ),
        technical: Some(
            "Technical details show the expected and actual base hashes, the required revision and region only if known, the patch format and the exact rejection.",
        ),
        action: Some(A::ReviewExpectedBase),
        ..BASE
    },
    // --- 25, 26 Conversion -----------------------------------------------------------
    GuidanceScript {
        id: "conversion.preview_available",
        design_number: Some(25),
        category: C::Explain,
        topics: &[T::Conversion],
        pages: &[P::Converter],
        scope: S::Operation,
        requires: &[K::ConversionPreviewAvailable],
        excludes: &[K::ConversionPreservationUnknown],
        priority: 50,
        mascot: M::Helpful,
        quick: "This conversion can create a separate {target_format} file. Preview the destination and checks before converting; your original stays unchanged.",
        minimal: Some(
            "Preview {source_format} → {target_format}; separate output, original retained.",
        ),
        explain: Some(
            "The preview shows what EmuWiz can actually perform, the output location and any warnings. A recognised format alone does not mean conversion is supported.",
        ),
        technical: Some(
            "Technical details show the source and target representation, the backend capability, the operation classification, the collision rule and the verification plan.",
        ),
        action: Some(A::PreviewConversion),
        ..BASE
    },
    GuidanceScript {
        id: "conversion.preservation_unknown",
        design_number: Some(26),
        category: C::Warning,
        topics: &[T::Conversion, T::Warnings],
        pages: &[P::Converter],
        scope: S::Operation,
        requires: &[K::ConversionPreservationUnknown],
        priority: 70,
        repeat: R::CollapsibleWarning,
        mascot: M::Warning,
        quick: "EmuWiz cannot prove that this conversion preserves all disc layout information. Review the warning before deciding what to do.",
        minimal: Some("Preservation not proven. Review the planner's warning or blocker."),
        explain: Some(
            "A disc may contain several tracks, audio or timing information beyond a simple data image. Only mention those features when the source evidence confirms them. Unknown preservation is not the same as lossless.",
        ),
        technical: Some(
            "Technical details show the existing classification, the track and audio evidence if present, the unsupported properties and the planner’s allowed or blocked state.",
        ),
        action: Some(A::ReviewConversionWarning),
        ..BASE
    },
    // --- 27 Multi-disc ---------------------------------------------------------------
    GuidanceScript {
        id: "multidisc.required_disc_missing",
        design_number: Some(27),
        category: C::WhyBlocked,
        topics: &[T::MultiDisc],
        pages: &[P::Games, P::Launch],
        scope: S::Game,
        requires: &[K::MultiDiscRequiredMissing],
        priority: 90,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "This game is missing a required disc from its known set. Review the disc list before launching or rebuilding it.",
        minimal: Some("Required disc missing from the known set. Review media evidence."),
        explain: Some(
            "Some games use several discs as one release. EmuWiz needs evidence that those discs belong together; similar names alone do not establish a complete set.",
        ),
        technical: Some(
            "Technical details show the known disc identities and order, the missing member, the manifest or DAT provenance and the readiness consequence.",
        ),
        action: Some(A::ReviewDiscSet),
        ..BASE
    },
    // --- 28, 29 Playing Library and duplicates ---------------------------------------
    GuidanceScript {
        id: "playing_library.plan_ready",
        design_number: Some(28),
        category: C::Explain,
        topics: &[T::PlayingLibrary],
        pages: &[P::Organisation],
        scope: S::Operation,
        requires: &[K::PlayingLibraryPlan],
        priority: 50,
        mascot: M::Helpful,
        quick: "The plan selects {set_count} {set_count?set|sets} and creates {link_count} {link_count?link|links} for a play-focused library. Review the choices; your original collection stays in place.",
        minimal: Some(
            "{set_count} {set_count?set|sets}, {link_count} {link_count?link|links} planned. Originals remain in place.",
        ),
        explain: Some(
            "A Playing Library can make a collection easier to browse without reorganising the preserved originals. Where 1G1R and regional preferences are supported, the plan shows which release was selected and why.",
        ),
        technical: Some(
            "Technical details show the selected entries, the preference rules applied, the skipped candidates, the output root, any collisions and the source-preservation evidence.",
        ),
        action: Some(A::ReviewPlayingLibraryPlan),
        ..BASE
    },
    GuidanceScript {
        id: "duplicates.exact_matches",
        design_number: Some(29),
        category: C::Explain,
        topics: &[T::Duplicates],
        pages: &[P::Duplicates, P::Organisation],
        scope: S::Operation,
        requires: &[K::DuplicatesExact],
        priority: 50,
        mascot: M::Helpful,
        quick: "These files match the duplicate check. Review which copy to keep before any supported quarantine action.",
        minimal: Some("Exact duplicate evidence found. Review the group before quarantine."),
        explain: Some(
            "A duplicate check is separate from deleting or moving anything. Different regions or revisions must not be treated as duplicates just because their titles look alike.",
        ),
        technical: Some(
            "Technical details show the comparison method, the hashes, the source paths, the protected roles and the existing quarantine and undo eligibility.",
        ),
        action: Some(A::ReviewDuplicateGroup),
        ..BASE
    },
    // --- 30, 31 History --------------------------------------------------------------
    GuidanceScript {
        id: "history.undo_available",
        design_number: Some(30),
        category: C::Explain,
        topics: &[T::HistoryUndo],
        pages: &[P::History],
        scope: S::Operation,
        requires: &[K::UndoAvailable],
        excludes: &[K::UndoRefused],
        priority: 50,
        exclusive_group: Some("undo"),
        mascot: M::Helpful,
        quick: "This change has an undo option. Review what will be restored before confirming it.",
        minimal: Some("Undo available for review; current checks still apply."),
        explain: Some(
            "Undo depends on the files and recovery data still matching the recorded operation. EmuWiz checks those requirements again; an old receipt alone is not a guarantee.",
        ),
        technical: Some(
            "Technical details show the transaction ID, the expected paths and hashes, the retained recovery data and the latest eligibility.",
        ),
        action: Some(A::ReviewUndo),
        ..BASE
    },
    GuidanceScript {
        id: "history.undo_refused",
        design_number: Some(31),
        category: C::WhyBlocked,
        topics: &[T::HistoryUndo, T::RecoveryFromFailure],
        pages: &[P::History],
        scope: S::Operation,
        requires: &[K::UndoRefused],
        priority: 80,
        exclusive_group: Some("undo"),
        repeat: R::Blocker,
        mascot: M::Concerned,
        quick: "EmuWiz cannot safely undo this change because {plain_undo_reason}. Review the recovery details.",
        minimal: Some("Undo blocked: {plain_undo_reason}. Review recovery evidence."),
        explain: Some(
            "Undo must not overwrite a file that has changed since the operation. The recorded change is still available to inspect, even when automatic recovery is no longer safe.",
        ),
        technical: Some(
            "Technical details show the precise refusal, the expected and current evidence, backup availability and the supported recovery actions only.",
        ),
        action: Some(A::ReviewRecoveryDetails),
        ..BASE
    },
    // --- 32, 33 RomM and offline -----------------------------------------------------
    GuidanceScript {
        id: "romm.snapshot_unavailable",
        design_number: Some(32),
        category: C::Explain,
        topics: &[T::Romm],
        pages: &[P::Romm],
        scope: S::Source,
        requires: &[K::RommSnapshotUnavailable],
        priority: 50,
        mascot: M::Helpful,
        quick: "RomM library information is unavailable here: {plain_source_reason}. Review the integration setup.",
        minimal: Some("RomM information unavailable: {plain_source_reason}."),
        explain: Some(
            "This view uses the information supplied by the current adapter. It does not imply native browsing or downloading beyond the controls already offered. Locally verified game evidence remains separate.",
        ),
        technical: Some(
            "Technical details show the snapshot status, cache availability, the adapter’s capability and the redacted raw error.",
        ),
        action: Some(A::ReviewRommSetup),
        ..BASE
    },
    GuidanceScript {
        id: "offline.optional_source_unavailable",
        design_number: Some(33),
        category: C::Explain,
        topics: &[T::OfflineMode],
        scope: S::Source,
        requires: &[K::OptionalSourceUnavailable],
        priority: 40,
        mascot: M::Neutral,
        quick: "This online source is unavailable, but you can still browse the local game list. Review its status when you need data from it.",
        minimal: Some("Optional source unavailable. Local browsing remains available."),
        explain: Some(
            "Artwork or metadata from that source may be missing or cached. This does not establish that the whole computer is offline, and it does not by itself stop local emulation.",
        ),
        technical: Some(
            "Technical details show the affected provider, the last observed failure and any usable cache evidence. No new connectivity probe is run.",
        ),
        action: Some(A::ReviewSourceStatus),
        ..BASE
    },
    // --- 34 Empty library ------------------------------------------------------------
    GuidanceScript {
        id: "library.loaded_empty",
        design_number: Some(34),
        category: C::EmptyState,
        topics: &[T::EmptyLibrary],
        pages: &[P::Home, P::Games],
        requires: &[K::LibraryLoadedEmpty],
        excludes: &[K::ScanRunning, K::SourcesNoneConfigured],
        priority: 50,
        mascot: M::Helpful,
        quick: "The game list is empty. Review your configured folders and scan them when you’re ready.",
        minimal: Some("No games in the loaded catalogue. Review sources and scan status."),
        explain: Some(
            "An empty catalogue is different from a folder being unavailable or a scan still running. The Sources page shows where EmuWiz is configured to look and the latest scan information.",
        ),
        technical: Some(
            "Technical details show the catalogue load status, the source count and the last scan result, without guessing which files are absent.",
        ),
        action: Some(A::ReviewGameFolders),
        ..BASE
    },
    // --- 35, 36 Unsupported and read-only --------------------------------------------
    GuidanceScript {
        id: "format.operation_unsupported",
        design_number: Some(35),
        category: C::WhyBlocked,
        topics: &[T::UnsupportedFormat],
        scope: S::Operation,
        requires: &[K::FormatOperationUnsupported],
        priority: 90,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "EmuWiz recognises {format}, but it cannot perform {operation} on this representation. Review the supported options.",
        minimal: Some("{operation} unsupported for {format}. See capability details."),
        explain: Some(
            "Being able to inspect a file does not mean EmuWiz can convert, repair or launch it. No unsupported operation is made available by this guidance.",
        ),
        technical: Some(
            "Technical details show the capability result, the representation, the backend or refusal reason and the supported alternatives from the owner only.",
        ),
        action: Some(A::ReviewSupportedOptions),
        action_alternates: &[(K::NoCapabilityView, None)],
        ..BASE
    },
    GuidanceScript {
        id: "safety.destination_read_only",
        design_number: Some(36),
        category: C::WhyBlocked,
        topics: &[T::ReadOnlySafety],
        scope: S::Operation,
        requires: &[K::DestinationReadOnly],
        priority: 100,
        repeat: R::Blocker,
        mascot: M::Helpful,
        quick: "The chosen destination is read-only, so this operation cannot write its output there. Choose another destination.",
        minimal: Some("Output destination is read-only. Choose another folder."),
        explain: Some(
            "EmuWiz will not try to override the restriction. Selecting a writable output folder is separate from changing permissions on your original collection.",
        ),
        technical: Some(
            "Technical details show the destination path, the detected access restriction, the operation’s effect and the exact error.",
        ),
        action: Some(A::ReviewDestination),
        action_alternates: &[(K::DestinationChooserAvailable, Some(A::ChooseOutputFolder))],
        ..BASE
    },
    // --- 37, 38 Repair results -------------------------------------------------------
    GuidanceScript {
        id: "repair.completed_verified",
        design_number: Some(37),
        category: C::Success,
        topics: &[T::SuccessStates],
        scope: S::Operation,
        requires: &[K::RepairCompletedVerified],
        priority: 60,
        repeat: R::PerOperation,
        mascot: M::Success,
        quick: "That’s sorted. The corrected file passed verification, and the change is recorded in History.",
        minimal: Some("Repair completed and verified. Receipt recorded in History."),
        explain: Some(
            "This confirms the checks performed for this repair at that time. It does not mean every game in the collection was checked, or that a later file change would go unnoticed.",
        ),
        technical: Some(
            "Technical details show the operation ID, the verification method, result and time, the source and output behaviour and the actual undo eligibility.",
        ),
        action: Some(A::ViewRepairInHistory),
        ..BASE
    },
    GuidanceScript {
        id: "repair.completed_verification_pending",
        design_number: Some(38),
        category: C::Explain,
        topics: &[T::RecoveryFromFailure],
        scope: S::Operation,
        requires: &[K::RepairCompletedVerificationPending],
        excludes: &[K::VerificationUnavailable],
        priority: 80,
        exclusive_group: Some("repair-result"),
        mascot: M::Helpful,
        quick: "The repair step finished, but the result has not been fully verified. Run the supported verification check before relying on it.",
        minimal: Some("Repair step complete; verification incomplete. Verify the result."),
        explain: Some(
            "A completed operation is not the same as verified output. The verification view shows which checks remain; it must not turn a successful process exit into a claim about file health.",
        ),
        technical: Some(
            "Technical details show the completed stage, the outstanding checks, the current receipt and the verification capability.",
        ),
        action: Some(A::VerifyResult),
        ..BASE
    },
    // Variant of 38: say plainly that verification is unavailable, and offer a
    // review of the result instead.
    GuidanceScript {
        id: "repair.completed_verification_pending.unavailable",
        variant_of: Some("repair.completed_verification_pending"),
        category: C::Explain,
        topics: &[T::RecoveryFromFailure],
        scope: S::Operation,
        requires: &[
            K::RepairCompletedVerificationPending,
            K::VerificationUnavailable,
        ],
        priority: 80,
        exclusive_group: Some("repair-result"),
        mascot: M::Helpful,
        quick: "The repair step finished, but the result has not been fully verified. Verification is unavailable for this result.",
        minimal: Some("Repair step complete; verification unavailable for this result."),
        explain: Some(
            "A completed operation is not the same as verified output. The verification view shows which checks remain; it must not turn a successful process exit into a claim about file health.",
        ),
        technical: Some(
            "Technical details show the completed stage, the outstanding checks, the current receipt and the verification capability.",
        ),
        action: Some(A::ReviewResult),
        ..BASE
    },
    // --- 39 Recovery -----------------------------------------------------------------
    GuidanceScript {
        id: "recovery.source_changed_since_preview",
        design_number: Some(39),
        category: C::WhyBlocked,
        topics: &[T::RecoveryFromFailure, T::ReadOnlySafety],
        scope: S::Operation,
        requires: &[K::SourceChangedSincePreview],
        priority: 100,
        repeat: R::Blocker,
        mascot: M::Concerned,
        quick: "The source changed after the preview, so EmuWiz stopped before applying it. Create a new preview from the current files.",
        minimal: Some("Apply refused: source changed since preview. Preview again."),
        explain: Some(
            "A preview describes particular files at a particular time. Reusing it after those files change could apply the wrong operation. Review the new plan before confirming anything.",
        ),
        technical: Some(
            "Technical details show the rejected plan ID, the expected and current fingerprints and the explicit no-write outcome.",
        ),
        action: Some(A::PreviewAgain),
        ..BASE
    },
    // --- 42 Activity -----------------------------------------------------------------
    GuidanceScript {
        id: "activity.queued_work",
        design_number: Some(42),
        category: C::Explain,
        topics: &[T::Activity],
        requires: &[K::QueuedWork],
        priority: 50,
        mascot: M::Helpful,
        quick: "{queued_count} {queued_count?task is|tasks are} waiting to start; {running_count} {running_count?is|are} running. Open Activity to see their current states.",
        minimal: Some("{running_count} running; {queued_count} waiting."),
        explain: Some(
            "A waiting task has not started work. A completed, failed, cancelled or superseded task is retained as a result, but is not counted as active. No time estimate is inferred from the queue.",
        ),
        technical: Some(
            "Technical details show the actual job phases, the current item, the elapsed running time and whether cancellation is supported.",
        ),
        action: Some(A::ViewActivity),
        ..BASE
    },
    // =================================================================================
    // Legacy page guidance: the 35 messages the pre-engine implementation shipped,
    // word for word. They require only the legacy fact kinds, so existing pages
    // are unchanged until a page-owned adapter supplies design facts instead.
    // `replaced_by` records which designed scripts supersede each.
    // =================================================================================
    GuidanceScript {
        id: "home-empty",
        origin: ScriptOrigin::Legacy,
        category: C::EmptyState,
        topics: &[T::EmptyLibrary],
        pages: &[P::Home],
        requires: &[K::LegacyLibraryEmpty],
        priority: 50,
        mascot: M::Neutral,
        quick: "No games are listed yet. Review Sources to choose which existing folders EmuWiz may scan.",
        replaced_by: &[
            "first_run.choose_sources",
            "sources.none_configured",
            "library.loaded_empty",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "home-browse",
        origin: ScriptOrigin::Legacy,
        category: C::Tip,
        topics: &[T::Home],
        pages: &[P::Home],
        // Yields to the designed first-use tip when the page supplies its fact.
        excludes: &[K::LegacyLibraryEmpty, K::HomeFirstUseTipEligible],
        mascot: M::Neutral,
        quick: "Browse first: EmuWiz keeps inspection separate from actions that change files.",
        replaced_by: &["home.browse_ready"],
        ..BASE
    },
    GuidanceScript {
        id: "source-unavailable-history",
        origin: ScriptOrigin::Legacy,
        category: C::Warning,
        topics: &[T::Sources],
        pages: &[P::Sources],
        scope: S::Source,
        requires: &[K::LegacySourceUnavailableWithScan],
        priority: 70,
        mascot: M::Warning,
        quick: "This source is unavailable, but its catalogue history has been kept. Last scan: {scan}.",
        replaced_by: &["sources.unavailable"],
        ..BASE
    },
    GuidanceScript {
        id: "source-unavailable",
        origin: ScriptOrigin::Legacy,
        category: C::Warning,
        topics: &[T::Sources],
        pages: &[P::Sources],
        scope: S::Source,
        requires: &[K::LegacySourceUnavailable],
        priority: 70,
        mascot: M::Warning,
        quick: "This source is unavailable. Review its configured path before scanning again.",
        replaced_by: &["sources.unavailable"],
        ..BASE
    },
    GuidanceScript {
        id: "source-last-scan",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Sources],
        pages: &[P::Sources],
        scope: S::Source,
        requires: &[K::LegacySourceLastScan],
        mascot: M::Neutral,
        quick: "This folder was last scanned at {scan}.",
        ..BASE
    },
    GuidanceScript {
        id: "source-review",
        origin: ScriptOrigin::Legacy,
        category: C::Tip,
        topics: &[T::Sources],
        pages: &[P::Sources],
        excludes: &[
            K::LegacySourceUnavailable,
            K::LegacySourceUnavailableWithScan,
            K::LegacySourceLastScan,
        ],
        mascot: M::Neutral,
        quick: "Review a folder before scanning it; browsing does not move or rename source files.",
        ..BASE
    },
    GuidanceScript {
        id: "launch-identity-blocked",
        origin: ScriptOrigin::Legacy,
        category: C::WhyBlocked,
        topics: &[T::LaunchReadiness],
        pages: &[P::Launch],
        scope: S::Game,
        requires: &[K::LegacyLaunchIdentityUnverified],
        priority: 90,
        mascot: M::Helpful,
        quick: "This game is blocked because its identity has not been verified.",
        replaced_by: &[
            "identity.unknown",
            "identity.candidate_only",
            "launch.no_compatible_emulator",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "launch-verified",
        origin: ScriptOrigin::Legacy,
        category: C::Success,
        topics: &[T::LaunchReadiness],
        pages: &[P::Launch],
        scope: S::Game,
        requires: &[K::LegacyLaunchIdentityVerified],
        priority: 60,
        mascot: M::Success,
        quick: "Identity is verified; EmuWiz will still check the emulator, media and firmware before launch.",
        replaced_by: &["launch.ready"],
        ..BASE
    },
    GuidanceScript {
        id: "launch-checks",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::LaunchReadiness],
        pages: &[P::Launch],
        excludes: &[
            K::LegacyLaunchIdentityUnverified,
            K::LegacyLaunchIdentityVerified,
        ],
        mascot: M::Thinking,
        quick: "Launch readiness checks the selected game's identity and required emulator setup together.",
        replaced_by: &[
            "launch.ready",
            "launch.no_compatible_emulator",
            "launch.failed",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "problem-blocker",
        origin: ScriptOrigin::Legacy,
        category: C::WhyBlocked,
        topics: &[T::Problems],
        pages: &[P::ProblemsRepair],
        requires: &[K::LegacyProblemBlocker],
        priority: 80,
        mascot: M::Helpful,
        quick: "{blocker}",
        ..BASE
    },
    GuidanceScript {
        id: "problem-checking",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Problems],
        pages: &[P::ProblemsRepair],
        requires: &[K::LegacyProblemsChecking],
        mascot: M::Thinking,
        quick: "EmuWiz is checking what it already knows about your games. You can keep browsing.",
        ..BASE
    },
    GuidanceScript {
        id: "problem-none",
        origin: ScriptOrigin::Legacy,
        category: C::Success,
        topics: &[T::Problems, T::SuccessStates],
        pages: &[P::ProblemsRepair],
        requires: &[K::LegacyProblemsClear],
        priority: 60,
        mascot: M::Success,
        quick: "Nothing needs your attention right now. Nothing was changed while checking.",
        ..BASE
    },
    GuidanceScript {
        id: "problem-review",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Problems],
        pages: &[P::ProblemsRepair],
        requires: &[K::LegacyProblemsFindings],
        priority: 50,
        mascot: M::Helpful,
        quick: "{count} finding(s) to look at{attention_clause}. Open one to see what happened and what EmuWiz can safely do. Nothing changes until you confirm a preview.",
        replaced_by: &["problems.review_findings"],
        ..BASE
    },
    GuidanceScript {
        id: "organisation-preview",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::Organisation],
        mascot: M::Helpful,
        quick: "Organisation starts with a preview. Originals stay protected until you explicitly confirm the previewed changes.",
        replaced_by: &["playing_library.plan_ready", "mame.missing_members"],
        ..BASE
    },
    GuidanceScript {
        id: "cheats-mods-review",
        origin: ScriptOrigin::Legacy,
        category: C::Tip,
        topics: &[T::Cheats],
        pages: &[P::CheatsMods],
        mascot: M::Helpful,
        quick: "Preview a cheat or mod change first; the selected game's identity determines what can be applied safely.",
        replaced_by: &[
            "cheats.no_game_selected",
            "cheats.none_known",
            "patch.wrong_base",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "museum-browse",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::Museum],
        mascot: M::Neutral,
        quick: "Museum views the current catalogue by platform; it does not alter games.",
        ..BASE
    },
    GuidanceScript {
        id: "tape-structure",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::TapeInspector],
        requires: &[K::LegacyTapeStructure],
        priority: 50,
        mascot: M::Helpful,
        quick: "This {format} contains {blocks} blocks. TZX can store timing information that a plain TAP cannot.",
        ..BASE
    },
    GuidanceScript {
        id: "tape-review",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::TapeInspector],
        excludes: &[K::LegacyTapeStructure],
        mascot: M::Neutral,
        quick: "Tape inspection reports structure from the selected file; no conversion is performed by browsing.",
        ..BASE
    },
    GuidanceScript {
        id: "archive-review",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::ArchiveInspector],
        mascot: M::Neutral,
        quick: "Archive inspection lists bounded member evidence without extracting or changing the source archive.",
        ..BASE
    },
    GuidanceScript {
        id: "dat-provenance",
        origin: ScriptOrigin::Legacy,
        category: C::Success,
        topics: &[T::IdentityDat],
        pages: &[P::DatManagement],
        requires: &[K::LegacyDatIdentity],
        priority: 60,
        mascot: M::Success,
        quick: "This DAT supplied the identity used for this match: {name}.",
        ..BASE
    },
    GuidanceScript {
        id: "dat-review",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::IdentityDat],
        pages: &[P::DatManagement],
        excludes: &[K::LegacyDatIdentity],
        mascot: M::Helpful,
        quick: "DAT Management keeps identity data versioned and reviewable before it is used for matching.",
        replaced_by: &["dat.none_available"],
        ..BASE
    },
    GuidanceScript {
        id: "firmware-review",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::BiosFirmware],
        pages: &[P::BiosFirmware],
        mascot: M::Helpful,
        quick: "Review required firmware and detected evidence here; EmuWiz does not provide copyrighted firmware.",
        replaced_by: &["firmware.missing"],
        ..BASE
    },
    GuidanceScript {
        id: "emulator-readiness",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::EmulatorSetup],
        pages: &[P::EmulatorSetup],
        mascot: M::Helpful,
        quick: "Emulator Setup reports detected installations and readiness; it does not prove that every game can launch.",
        replaced_by: &[
            "emulator.multiple_installations",
            "launch.no_compatible_emulator",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "setup-fix-first",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::Setup],
        mascot: M::Helpful,
        quick: "Fix the items marked as needing attention first; everything else here is for information.",
        ..BASE
    },
    GuidanceScript {
        id: "check-platform",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::IdentityDat],
        pages: &[P::CheckGames],
        mascot: M::Helpful,
        quick: "Choose a platform to see which of its games are verified, unknown or need attention. Checking never renames anything.",
        ..BASE
    },
    GuidanceScript {
        id: "activity-idle",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Activity],
        pages: &[P::Activity],
        excludes: &[K::LegacyJobsRunning],
        mascot: M::Helpful,
        quick: "Nothing is running. Finished work stays listed here with its result.",
        replaced_by: &["activity.queued_work"],
        ..BASE
    },
    GuidanceScript {
        id: "activity-busy",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Activity],
        pages: &[P::Activity],
        requires: &[K::LegacyJobsRunning],
        priority: 50,
        mascot: M::Thinking,
        quick: "{count} task(s) active. You can keep browsing; work continues in the background.",
        replaced_by: &["activity.queued_work"],
        ..BASE
    },
    GuidanceScript {
        id: "history-undo",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::HistoryUndo],
        pages: &[P::History],
        mascot: M::Helpful,
        quick: "History lists changes EmuWiz made. Select an entry to see whether it can be undone.",
        replaced_by: &["history.undo_available", "history.undo_refused"],
        ..BASE
    },
    GuidanceScript {
        id: "saves-kinds",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::Saves],
        mascot: M::Neutral,
        quick: "Saves, save states and memory cards are kept separate. This page only lists them and never changes them.",
        ..BASE
    },
    GuidanceScript {
        id: "converter-preview",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Conversion],
        pages: &[P::Converter],
        mascot: M::Helpful,
        quick: "Pick a disc set, review the preview, then convert. Your original files are kept unless you choose otherwise.",
        replaced_by: &[
            "conversion.preview_available",
            "conversion.preservation_unknown",
            "format.operation_unsupported",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "artwork-game",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::ArtworkMetadata],
        pages: &[P::Artwork],
        mascot: M::Helpful,
        quick: "Artwork and manuals are shown for the selected game. Provider setup lives in Sources & Providers.",
        replaced_by: &[
            "artwork.cover_missing",
            "artwork.stale_usable_cache",
            "artwork.alternatives_available",
        ],
        ..BASE
    },
    GuidanceScript {
        id: "romm-readonly",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        topics: &[T::Romm],
        pages: &[P::Romm],
        mascot: M::Helpful,
        quick: "RomM is browsed read-only here. Its information never replaces what EmuWiz has verified locally.",
        replaced_by: &["romm.snapshot_unavailable"],
        ..BASE
    },
    GuidanceScript {
        id: "advanced-inspect",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::Advanced],
        mascot: M::Helpful,
        quick: "These tools inspect things without changing them. Everyday tasks have their own pages.",
        ..BASE
    },
    GuidanceScript {
        id: "settings-hints",
        origin: ScriptOrigin::Legacy,
        category: C::Explain,
        pages: &[P::Settings],
        mascot: M::Helpful,
        quick: "You can turn these hints off below; no control or page is ever hidden by them.",
        ..BASE
    },
    GuidanceScript {
        id: "games-browse",
        origin: ScriptOrigin::Legacy,
        category: C::Tip,
        topics: &[T::Home],
        pages: &[P::Games],
        mascot: M::Neutral,
        quick: "Games is the catalogue view. Select a title to review identity and readiness before launching.",
        ..BASE
    },
];
