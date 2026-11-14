//! `emuwiz-cli transform`: plan, preview, apply, recover, inspect and undo a
//! single-file relocation (Project Ouroboros).
//!
//! Every sub-command takes an explicit `--journal` (and `plan` an explicit
//! `--database`); there is no default to the user's real catalogue. Planning,
//! preview, status and Undo planning are read-only. `apply` and `recover` are
//! refused by the engine in every build of this binary: execution needs a
//! test-only authority that only EmuWiz's test suite can construct (no flag,
//! setting, environment variable or file enables it). `--help` anywhere
//! prints usage and does nothing else.

use std::path::PathBuf;

use archivefs_core::Database;
use archivefs_core::transform::{
    self, Context, Plan, PlanKind, TxnState, engine, plan_relocation, plan_undo, undo_blocker,
};

const USAGE: &str = "\
usage: emuwiz-cli transform <command> --journal <dir> [options]

  plan    --database <db> --source <file> --to <folder> [--plan-out <file>] [--json]
          Read-only preview: blockers, effects, dependencies, strategy, digest.
  apply   --plan <file> --digest <sha256>
          Not available in this build: always refused, nothing is changed.
  status  [--txn <id>] [--json]
          Read-only state of one or every transaction.
  recover [--txn <id>]
          Not available in this build: always refused, nothing is changed.
  undo-plan --txn <id> [--plan-out <file>]
          Read-only: plans an Undo or explains why it is not available.

Relocation runs only inside EmuWiz's test suite on scratch folders it creates.
Nothing here deletes an original: cross-filesystem relocation retains it.";

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty()
        || args
            .iter()
            .any(|a| a == "--help" || a == "-h" || a == "help")
    {
        println!("{USAGE}");
        return Ok(());
    }
    let command = args[0].clone();
    let mut rest = args[1..].to_vec();
    let journal = take(&mut rest, "--journal")?
        .map(PathBuf::from)
        .ok_or("--journal <dir> is required")?;
    let ctx = Context::new(journal);
    match command.as_str() {
        "plan" => plan(&ctx, rest),
        "apply" => apply(&ctx, rest),
        "status" => status(&ctx, rest),
        "recover" => recover(&ctx, rest),
        "undo-plan" => undo(&ctx, rest),
        other => Err(format!("unknown transform command '{other}'\n{USAGE}").into()),
    }
}

fn take(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(pos) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if pos + 1 >= args.len() {
        return Err(format!("{flag} requires a value").into());
    }
    let value = args.remove(pos + 1);
    args.remove(pos);
    if args.iter().any(|a| a == flag) {
        return Err(format!("{flag} may be given only once").into());
    }
    Ok(Some(value))
}

fn flag(args: &mut Vec<String>, name: &str) -> bool {
    let had = args.iter().any(|a| a == name);
    args.retain(|a| a != name);
    had
}

fn finish(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments {args:?}").into())
    }
}

fn plan(ctx: &Context<'_>, mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let json = flag(&mut args, "--json");
    let db = take(&mut args, "--database")?.ok_or("--database <db> is required")?;
    let source = take(&mut args, "--source")?.ok_or("--source <file> is required")?;
    let to = take(&mut args, "--to")?.ok_or("--to <folder> is required")?;
    let out = take(&mut args, "--plan-out")?;
    finish(&args)?;
    let _ = ctx;
    let db = Database::open_read_only(&db)?;
    let preview = plan_relocation(
        &db,
        std::path::Path::new(&source),
        std::path::Path::new(&to),
    );
    if let (Some(out), Some(p)) = (&out, &preview.plan) {
        write_new(out, &engine::plan_bytes(p))?;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    println!(
        "Relocate {}\n      to {}",
        preview.source.display(),
        preview.destination.display()
    );
    if let Some(s) = preview.strategy {
        println!("Strategy: {s:?}");
    }
    if let Some(d) = &preview.dependencies {
        println!(
            "Dependencies: {} grounded edge(s), {} unknown",
            d.edges, d.unknowns
        );
    }
    for e in &preview.effects {
        println!("  effect: {e}");
    }
    if preview.blockers.is_empty() {
        println!("Plan digest: {}", preview.digest.as_deref().unwrap_or("-"));
        if let Some(out) = out {
            println!("Plan written to {out}");
        }
    } else {
        for b in &preview.blockers {
            println!("  BLOCKED: {b}");
        }
        return Err("relocation is blocked; nothing was planned".into());
    }
    Ok(())
}

fn write_new(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn read_plan(path: &str) -> Result<Plan, Box<dyn std::error::Error>> {
    Ok(engine::parse_plan(&std::fs::read(path)?)?)
}

fn report(id: &str, state: &TxnState) {
    println!("{id}: {}", state.label());
    match state {
        TxnState::Blocked {
            reason, safe_facts, ..
        } => {
            println!("  reason: {reason}");
            for f in safe_facts {
                println!("  known: {f}");
            }
        }
        TxnState::Compensated { residue } => {
            for r in residue {
                println!("  retained for review: {}", r.display());
            }
        }
        TxnState::Corrupt { reason } => println!("  reason: {reason}"),
        _ => {}
    }
}

fn exit_for(state: &TxnState) -> Result<(), Box<dyn std::error::Error>> {
    if state.is_success() {
        Ok(())
    } else {
        Err(format!("not complete: {}", state.label()).into())
    }
}

fn apply(ctx: &Context<'_>, mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let plan = read_plan(&take(&mut args, "--plan")?.ok_or("--plan <file> is required")?)?;
    let digest = take(&mut args, "--digest")?.ok_or("--digest <sha256> is required (from plan)")?;
    finish(&args)?;
    let state = match plan.kind {
        PlanKind::Undo { .. } => transform::apply_undo(ctx, &plan, &digest),
        PlanKind::Relocate => transform::apply(ctx, &plan, &digest),
    }
    .map_err(|e| e.to_string())?;
    report(&plan.txn_id, &state);
    exit_for(&state)
}

fn status(ctx: &Context<'_>, mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let json = flag(&mut args, "--json");
    let txn = take(&mut args, "--txn")?;
    finish(&args)?;
    let rows: Vec<(String, TxnState, Option<Plan>)> = match txn {
        Some(id) => {
            let (s, p) = transform::status(ctx, &id);
            vec![(id, s, p)]
        }
        None => transform::list(ctx),
    };
    if json {
        let out: Vec<_> = rows
            .iter()
            .map(|(id, s, p)| {
                serde_json::json!({
                    "txn": id,
                    "state": s.label(),
                    "success": s.is_success(),
                    "detail": format!("{s:?}"),
                    "source": p.as_ref().map(|p| p.source_path.clone()),
                    "destination": p.as_ref().map(|p| p.destination_path.clone()),
                    "undo_blocker": (matches!(s, TxnState::Complete) && p.as_ref().is_some_and(|p| matches!(p.kind, PlanKind::Relocate)))
                        .then(|| undo_blocker(ctx, id)).flatten(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    for (id, s, p) in &rows {
        report(id, s);
        if let Some(p) = p {
            println!(
                "  {} -> {}",
                p.source_path.display(),
                p.destination_path.display()
            );
        }
    }
    Ok(())
}

fn recover(ctx: &Context<'_>, mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let txn = take(&mut args, "--txn")?;
    finish(&args)?;
    let results = match txn {
        Some(id) => vec![(id.clone(), transform::recover(ctx, &id))],
        None => transform::recover_all(ctx),
    };
    let mut all_ok = true;
    for (id, r) in results {
        match r {
            Ok(state) => {
                all_ok &= state.is_terminal();
                report(&id, &state);
            }
            Err(e) => {
                all_ok = false;
                println!("{id}: {e}");
            }
        }
    }
    if all_ok {
        Ok(())
    } else {
        Err("some transactions still need attention".into())
    }
}

fn undo(ctx: &Context<'_>, mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let txn = take(&mut args, "--txn")?.ok_or("--txn <id> is required")?;
    let out = take(&mut args, "--plan-out")?;
    finish(&args)?;
    let plan = plan_undo(ctx, &txn)?;
    let digest = transform::plan_digest(&plan);
    if let Some(out) = &out {
        write_new(out, &engine::plan_bytes(&plan))?;
    }
    println!("Undo of {txn} is available. Plan digest: {digest}");
    Ok(())
}
