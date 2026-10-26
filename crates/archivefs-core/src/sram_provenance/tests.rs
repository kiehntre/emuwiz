use super::*;
#[test]
fn exact_object_change_and_links_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("save.srm");
    fs::write(&path, b"one").unwrap();
    let first = object(&path).unwrap().0;
    current(&first).unwrap();
    fs::write(&path, b"two").unwrap();
    assert!(current(&first).is_err());
    let link = t.path().join("alias");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(object(&link).is_err());
}
#[test]
fn sealed_origin_store_survives_restart_and_refuses_tampering() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("origins");
    let store = store::Store::open(&path, true).unwrap();
    assert!(store.receipts().unwrap().is_empty());
    drop(store);
    let store = store::Store::open(&path, false).unwrap();
    assert!(store.receipts().unwrap().is_empty());
    fs::write(path.join("forged.json"), b"{}").unwrap();
    assert!(store.receipts().is_err());
}

fn context(t: &Path) -> Context {
    fs::write(t.join("rom.gb"), b"ROM!").unwrap();
    fs::write(t.join("retroarch.cfg"), b"test=true\n").unwrap();
    let saves = t.join("saves");
    fs::create_dir_all(&saves).unwrap();
    let m = fs::metadata(&saves).unwrap();
    let s = Stamp::from_metadata(&m);
    Context {
        cartridge: object(&t.join("rom.gb")).unwrap().0,
        cartridge_identity: crate::game_identity::IdentityEvidence {
            kind: crate::game_identity::IdentityKind::LooseRomSha256,
            status: crate::game_identity::IdentityStatus::Verified,
            value: Some(object(&t.join("rom.gb")).unwrap().0.sha256),
            confidence: crate::game_identity::IdentityConfidence::StructuredMetadata,
            provenance: crate::game_identity::IdentityProvenance {
                archive_path: t.join("rom.gb"),
                member_path: None,
                member_index: None,
                method: "synthetic boundary fixture".into(),
            },
            diagnostic: "synthetic verified bytes".into(),
        },
        executable: object(&t.join("host")).unwrap().0,
        core: object(&t.join("core.so")).unwrap().0,
        scope: Scope {
            selected_executable: t.join("host"),
            selected_core: t.join("core.so"),
            configuration: object(&t.join("retroarch.cfg")).unwrap().0,
            saves,
            saves_identity: DirectoryIdentity {
                device: s.device,
                inode: s.inode,
            },
            profile: "Native/User".into(),
        },
    }
}
fn build_host(t: &Path) {
    use std::process::Command;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sram_provenance");
    assert!(
        Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(t.join("core.so"))
            .arg(source.join("test_core.c"))
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("cc")
            .arg("-o")
            .arg(t.join("host"))
            .arg(source.join("test_host.c"))
            .args(["-ldl", "-pthread"])
            .status()
            .unwrap()
            .success()
    );
}
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn observe(
    t: &Path,
    ctx: Context,
    mode: &str,
    eligible: std::collections::BTreeMap<PathBuf, SramOrigin>,
) -> protocol::Transcript {
    observe_named(t, ctx, mode, eligible, "arbitrary-name.srm")
}
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn observe_named(
    t: &Path,
    ctx: Context,
    mode: &str,
    eligible: std::collections::BTreeMap<PathBuf, SramOrigin>,
    name: &str,
) -> protocol::Transcript {
    use std::{
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32},
        },
        time::{Duration, Instant},
    };
    let lib = t.join("observer.so");
    fs::write(
        &lib,
        include_bytes!(concat!(env!("OUT_DIR"), "/sram-observer.so")),
    )
    .unwrap();
    let (parent, child) = UnixStream::pair().unwrap();
    let fd = child.as_raw_fd();
    let mut cookie = 0u64;
    let mut len = 8;
    assert_eq!(
        unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_COOKIE,
                (&mut cookie as *mut u64).cast(),
                &mut len,
            )
        },
        0
    );
    let pid = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (reader_pid, reader_stop) = (pid.clone(), stop.clone());
    let read_ctx = ctx.clone();
    let reader = std::thread::spawn(move || {
        protocol::read(parent, read_ctx, eligible, reader_pid, reader_stop)
    });
    let mut command = Command::new(&ctx.executable.path);
    command
        .args([
            ctx.core.path.as_os_str(),
            ctx.cartridge.path.as_os_str(),
            ctx.scope.saves.join(name).as_os_str(),
            ctx.scope.configuration.path.as_os_str(),
            std::ffi::OsStr::new(mode),
        ])
        .env("LD_PRELOAD", &lib)
        .env("EMUWIZ_SRAM_OBSERVER_FD", fd.to_string())
        .env("EMUWIZ_SRAM_COOKIE", cookie.to_string())
        .env("EMUWIZ_SRAM_CORE", &ctx.core.path)
        .env("EMUWIZ_SRAM_CONFIG", &ctx.scope.configuration.path)
        .env("EMUWIZ_SRAM_ROOT", &ctx.scope.saves);
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut process = command.spawn().unwrap();
    pid.store(process.id(), Ordering::Release);
    drop(child);
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = process.try_wait().unwrap() {
            assert!(status.success(), "host exit {status}, mode {mode}");
            break;
        }
        if Instant::now() > deadline {
            process.kill().unwrap();
            process.wait().unwrap();
            stop.store(true, Ordering::Release);
            panic!("Observer blocked native host in {mode}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().unwrap()
}
#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn actual_native_witness_and_refusals() {
    let t = tempfile::tempdir().unwrap();
    build_host(t.path());
    let ctx = context(t.path());
    for mode in ["normal", "blocking", "errno"] {
        let transcript = observe(t.path(), ctx.clone(), mode, Default::default());
        assert!(transcript.complete(), "{mode}: {:?}", transcript.reason);
        assert_eq!(transcript.artifacts.len(), 1);
        let artifact = transcript.artifacts.into_values().next().unwrap();
        current(&artifact).unwrap();
    }
    for mode in [
        "shadow",
        "partial",
        "state",
        "mixed",
        "interrupted",
        "historical",
    ] {
        let transcript = observe(t.path(), ctx.clone(), mode, Default::default());
        assert!(
            !transcript.complete() || transcript.artifacts.is_empty(),
            "incorrect provenance in {mode}"
        );
    }
}
#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn association_persists_and_changed_or_conflicting_evidence_needs_review() {
    use crate::persistent_state_inventory::*;
    let t = tempfile::tempdir().unwrap();
    build_host(t.path());
    let ctx = context(t.path());
    let transcript = observe(t.path(), ctx.clone(), "normal", Default::default());
    assert!(transcript.complete(), "{:?}", transcript.reason);
    let artifact = transcript.artifacts.into_values().next().unwrap();
    let origin = SramOrigin {
        state: OriginState::Associated,
        session: "first".into(),
        context: ctx.clone(),
        artifact: artifact.clone(),
        explanation: "witness".into(),
    };
    let receipt = Receipt {
        version: 1,
        session: "first".into(),
        context: ctx.clone(),
        origins: vec![origin.clone()],
        reason: None,
    };
    let store_path = t.path().join("origins");
    let store = store::Store::open(&store_path, true).unwrap();
    store.publish("first.json", &receipt).unwrap();
    drop(store);
    let store = store::Store::open(&store_path, false).unwrap();
    let roots = [PersistentStateRoot::configured(
        StateEmulator::RetroArch,
        ctx.scope.saves.clone(),
        Some(PersistentStateType::NativeSave),
    )];
    let mut inventory = inventory_persistent_state(&roots);
    inventory::project(&mut inventory, store.receipts().unwrap(), Some(&ctx.scope));
    assert_eq!(
        inventory.records[0].sram_origin.as_ref().unwrap().state,
        OriginState::Associated
    );
    assert!(inventory.records[0].game_identity.is_empty());
    let second = observe_named(
        t.path(),
        ctx.clone(),
        "normal",
        Default::default(),
        "save-witness.srm",
    );
    assert!(second.complete());
    let second_artifact = second.artifacts.into_values().next().unwrap();
    let mut second_receipt = receipt.clone();
    second_receipt.session = "second".into();
    second_receipt.origins[0].session = "second".into();
    second_receipt.origins[0].artifact = second_artifact.clone();
    store.publish("second.json", &second_receipt).unwrap();
    let sibling = ctx.scope.saves.join("save-history.srm");
    fs::write(&sibling, fs::read(&artifact.path).unwrap()).unwrap();
    let generic_roots = [PersistentStateRoot::configured(
        StateEmulator::RetroArch,
        ctx.scope.saves.clone(),
        None,
    )];
    let mut generic = inventory_persistent_state(&generic_roots);
    assert!(
        generic
            .records
            .iter()
            .all(|r| r.state_type == PersistentStateType::Unknown)
    );
    let old_portability = generic
        .records
        .iter()
        .find(|r| r.path == second_artifact.path)
        .unwrap()
        .portability_class;
    assert!(generic.records.iter().all(|r| r.path != artifact.path));
    inventory::project(&mut generic, store.receipts().unwrap(), Some(&ctx.scope));
    let attested = generic
        .records
        .iter()
        .find(|r| r.path == artifact.path)
        .unwrap();
    assert_eq!(attested.state_type, PersistentStateType::NativeSave);
    assert_eq!(attested.portability_class, PortabilityClass::DoNotTouch);
    let classified = generic
        .records
        .iter()
        .find(|r| r.path == second_artifact.path)
        .unwrap();
    assert_eq!(classified.state_type, PersistentStateType::NativeSave);
    assert_eq!(classified.portability_class, old_portability);
    assert!(classified.game_identity.is_empty());
    assert!(attested.game_identity.is_empty());
    assert_eq!(
        attested.sram_origin.as_ref().unwrap().state,
        OriginState::Associated
    );
    let historical = generic.records.iter().find(|r| r.path == sibling).unwrap();
    assert_eq!(historical.state_type, PersistentStateType::Unknown);
    assert!(historical.sram_origin.is_none());
    let known_state_roots = [PersistentStateRoot::configured(
        StateEmulator::RetroArch,
        ctx.scope.saves.clone(),
        Some(PersistentStateType::SaveState),
    )];
    let mut states = inventory_persistent_state(&known_state_roots);
    inventory::project(&mut states, vec![receipt.clone()], Some(&ctx.scope));
    assert!(
        states
            .records
            .iter()
            .all(|r| r.state_type == PersistentStateType::SaveState && r.sram_origin.is_none())
    );
    let overlap_roots = [generic_roots[0].clone(), known_state_roots[0].clone()];
    let mut overlap = inventory_persistent_state(&overlap_roots);
    inventory::project(&mut overlap, store.receipts().unwrap(), Some(&ctx.scope));
    assert!(overlap.records.iter().all(|r| {
        r.sram_origin
            .as_ref()
            .is_none_or(|o| o.state == OriginState::NeedsReview)
    }));
    assert!(
        overlap
            .records
            .iter()
            .filter(|r| r.path == second_artifact.path)
            .all(|r| r.state_type != PersistentStateType::NativeSave)
    );
    let mut identity_root = roots[0].clone();
    let mut foreign = ctx.cartridge_identity.clone();
    foreign.value = Some("different cartridge identity".into());
    identity_root.identity_evidence = vec![foreign];
    let mut ownership = inventory_persistent_state(&[identity_root]);
    inventory::project(&mut ownership, store.receipts().unwrap(), Some(&ctx.scope));
    assert!(ownership.records.iter().all(|r| {
        r.sram_origin
            .as_ref()
            .is_none_or(|o| o.state == OriginState::NeedsReview)
    }));
    let mut conflicting = second_receipt.clone();
    conflicting.context.cartridge.sha256 = "conflicting cartridge".into();
    conflicting.origins[0].context = conflicting.context.clone();
    let mut uncertain = inventory_persistent_state(&generic_roots);
    inventory::project(
        &mut uncertain,
        vec![second_receipt.clone(), conflicting],
        Some(&ctx.scope),
    );
    let uncertain = uncertain
        .records
        .iter()
        .find(|r| r.path == second_artifact.path)
        .unwrap();
    assert_eq!(uncertain.state_type, PersistentStateType::Unknown);
    assert_eq!(
        uncertain.sram_origin.as_ref().unwrap().state,
        OriginState::NeedsReview
    );
    let mut eligible = std::collections::BTreeMap::new();
    eligible.insert(artifact.path.clone(), origin.clone());
    assert!(observe(t.path(), ctx.clone(), "historical", eligible).complete());
    let mut conflict = receipt.clone();
    conflict.context.cartridge.sha256 = "another cartridge".into();
    conflict.origins[0].context = conflict.context.clone();
    let mut inventory = inventory_persistent_state(&roots);
    inventory::project(
        &mut inventory,
        vec![receipt.clone(), conflict],
        Some(&ctx.scope),
    );
    assert_eq!(
        inventory.records[0].sram_origin.as_ref().unwrap().state,
        OriginState::NeedsReview
    );
    let mut changed_scope = ctx.scope.clone();
    changed_scope.selected_executable = t.path().join("another installation");
    let mut selected = inventory_persistent_state(&roots);
    inventory::project(&mut selected, vec![receipt.clone()], Some(&changed_scope));
    assert_eq!(
        selected.records[0].sram_origin.as_ref().unwrap().state,
        OriginState::NeedsReview
    );
    let moved = ctx.scope.saves.join("same bytes elsewhere.srm");
    fs::rename(&artifact.path, &moved).unwrap();
    let mut missing = inventory_persistent_state(&roots);
    inventory::project(&mut missing, vec![receipt.clone()], Some(&ctx.scope));
    assert!(missing.records.iter().all(|r| r.sram_origin.is_none()));
    assert!(
        missing
            .warnings
            .iter()
            .any(|w| w.contains("missing, moved, unreadable"))
    );
    fs::rename(&moved, &artifact.path).unwrap();
    fs::write(&artifact.path, b"edited save").unwrap();
    fs::write(&second_artifact.path, b"changed witnessed save").unwrap();
    let mut stale = inventory_persistent_state(&generic_roots);
    inventory::project(&mut stale, vec![second_receipt.clone()], Some(&ctx.scope));
    let stale = stale
        .records
        .iter()
        .find(|r| r.path == second_artifact.path)
        .unwrap();
    assert_eq!(stale.state_type, PersistentStateType::Unknown);
    assert_eq!(
        stale.sram_origin.as_ref().unwrap().state,
        OriginState::NeedsReview
    );
    let mut inventory = inventory_persistent_state(&roots);
    inventory::project(&mut inventory, vec![receipt], Some(&ctx.scope));
    assert_eq!(
        inventory.records[0].sram_origin.as_ref().unwrap().state,
        OriginState::NeedsReview
    );
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn loaded_mapping_and_cartridge_bytes_cannot_be_substituted() {
    let t = tempfile::tempdir().unwrap();
    build_host(t.path());
    let ctx = context(t.path());
    let mut wrong = ctx.clone();
    wrong.core.stamp.inode += 1;
    assert!(!observe(t.path(), wrong, "normal", Default::default()).complete());
    let mut wrong = ctx;
    wrong.cartridge.sha256 = "different verified cartridge bytes".into();
    assert!(!observe(t.path(), wrong, "normal", Default::default()).complete());
}
#[test]
fn receipt_store_never_follows_replaced_metadata_parents() {
    let t = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), t.path().join("linked")).unwrap();
    assert!(store::Store::open(&t.path().join("linked/origins"), true).is_err());
    assert!(!outside.path().join("origins").exists());
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn caller_relative_loader_lookup_is_unchanged_with_observer() {
    use std::process::Command;
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::write(
        root.join("a.c"),
        r#"#define _GNU_SOURCE
#include <dlfcn.h>
int probe_symbol(void){return 11;}
int next_probe(void){int(*p)(void)=dlsym(RTLD_NEXT,"probe_symbol");return p?p():-1;}
"#,
    )
    .unwrap();
    fs::write(root.join("b.c"), "int probe_symbol(void){return 22;}\n").unwrap();
    fs::write(
        root.join("scope.c"),
        r#"#define _GNU_SOURCE
#include <dlfcn.h>
int local_only(void){return 42;}
int default_probe(void){int(*p)(void)=dlsym(RTLD_DEFAULT,"local_only");return p?p():-1;}
"#,
    )
    .unwrap();
    fs::write(root.join("main.c"),r#"#define _GNU_SOURCE
#include <dlfcn.h>
int next_probe(void);
int main(int argc,char**argv){void*h=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);if(!h)return 3;int(*p)(void)=dlsym(h,"default_probe");return next_probe()==22&&p&&p()==42?0:4;}
"#).unwrap();
    for name in ["a", "b", "scope"] {
        assert!(
            Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(root.join(format!("lib{name}.so")))
                .arg(root.join(format!("{name}.c")))
                .arg("-ldl")
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(
        Command::new("cc")
            .arg(root.join("main.c"))
            .arg("-o")
            .arg(root.join("loader"))
            .arg(format!("-L{}", root.display()))
            .args([
                "-Wl,--no-as-needed",
                "-la",
                "-lb",
                "-ldl",
                "-Wl,-rpath,$ORIGIN"
            ])
            .status()
            .unwrap()
            .success()
    );
    let observer = root.join("observer.so");
    fs::write(
        &observer,
        include_bytes!(concat!(env!("OUT_DIR"), "/sram-observer.so")),
    )
    .unwrap();
    for preload in [false, true] {
        let mut command = Command::new(root.join("loader"));
        command.arg(root.join("libscope.so"));
        if preload {
            command.env("LD_PRELOAD", &observer);
        }
        assert!(
            command.status().unwrap().success(),
            "caller scope changed with preload={preload}"
        );
    }
}
