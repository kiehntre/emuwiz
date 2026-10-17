//! Explicit operator tool. Ordinary EmuWiz GUI/CLI operation remains unchanged.
#[cfg(target_os = "linux")]
fn run(args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    use archivefs_core::dat::rename_apply::compact_recovery::{
        controller::RecoveryAction, deployment as d,
    };
    use std::path::Path;
    fn action(s: &str) -> Result<RecoveryAction, String> {
        match s {
            "apply" => Ok(RecoveryAction::Apply),
            "undo" => Ok(RecoveryAction::Undo),
            "retain-foreign" => Ok(RecoveryAction::RetainForeign),
            _ => Err("action must be apply, undo or retain-foreign".into()),
        }
    }
    fn output(value: &impl serde::Serialize) -> Result<(), serde_json::Error> {
        println!("{}", serde_json::to_string_pretty(value)?);
        Ok(())
    }
    fn read_grant(
        path: &Path,
    ) -> Result<
        archivefs_core::dat::rename_apply::compact_recovery::controller::SignedApproval,
        String,
    > {
        let bytes=d::read_document(path).map_err(|e|format!("Grant document unavailable: {e}; nothing consumed. Preserve this document and authority evidence; review again and use a new grant filename."))?;
        serde_json::from_slice(&bytes).map_err(|_|"Grant document empty, incomplete or invalid (possibly interrupted approval); nothing consumed. Preserve it and the authority records; obtain a fresh reviewed approval using a new grant filename. Do not overwrite or reset authority.".into())
    }
    const ENABLE: &str = "--enable-contract-1-i";
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["issuer-init",dir] => { output(&d::issuer_init(Path::new(dir))?)?; }
        ["provision",root,state,public,operator,"--accept-contract-1-i"] => {
            let key:Vec<u8>=serde_json::from_slice(&d::read_document(Path::new(public))?)?;
            d::provision(Path::new(root),Path::new(state),&key,operator,"Contract1-I")?;
            output(&serde_json::json!({"provisioned":true,"enabled":false}))?;
        }
        ["plan",state,file,pairs @ ..] if !pairs.is_empty() && pairs.len().is_multiple_of(2) => {
            let pairs=pairs.chunks_exact(2).map(|p|(p[0].to_owned(),p[1].to_owned())).collect::<Vec<_>>();
            let plan=d::plan(Path::new(state),&pairs)?;
            let bytes=serde_json::to_vec_pretty(&plan)?;
            d::write_document(Path::new(file),&bytes)?;
            output(&serde_json::json!({"plan":plan,"confirmation":d::plan_confirmation(&bytes)?}))?;
        }
        ["inspect-plan",file] => {
            let bytes=d::read_document(Path::new(file))?;
            output(&serde_json::json!({"plan":serde_json::from_slice::<serde_json::Value>(&bytes)?,"confirmation":d::plan_confirmation(&bytes)?}))?;
        }
        ["enroll",state,file,"--confirm",seal,flag @ ..] if flag.is_empty() || *flag==[ENABLE] => {
            d::enroll(Path::new(state),&d::read_document(Path::new(file))?,seal,*flag==[ENABLE])?;
            output(&serde_json::json!({"enrolled":true,"applied":false}))?;
        }
        ["preview-reuse",state,next] => {output(&d::preview_reuse(Path::new(state),Path::new(next))?)?;}
        ["reuse",state,next,"--confirm",seal,flag @ ..] if flag.is_empty() || *flag==[ENABLE] => {
            d::reuse(Path::new(state),Path::new(next),seal,*flag==[ENABLE])?;
            output(&serde_json::json!({"successor_state":next,"same_collection_root":true,"enrolled":false,"enabled":false}))?;
        }
        ["history",state] => {output(&d::history(Path::new(state))?)?;}
        ["review",state,requested] => {output(&d::review(Path::new(state),action(requested)?)?)?;}
        ["approve",state,signer,requested,"--confirm",seal,file] => {
            let grant=d::approve(Path::new(state),Path::new(signer),action(requested)?,seal)?;
            d::write_document(Path::new(file),&serde_json::to_vec(&grant)?)?;
            output(&serde_json::json!({"approval_written":file,"executed":false}))?;
        }
        ["status",state] => {
            // Read-only restart interpretation; never recovers, consumes or repairs.
            let status=d::status(Path::new(state));
            output(&status)?;
            return Ok(!matches!(status.state,d::RestartState::ReviewRequired|d::RestartState::Busy));
        }
        ["retained",state] => {
            let status=d::status(Path::new(state));
            let retained=status.entries.iter().filter_map(|e|e.retained_foreign.as_ref()).collect::<Vec<_>>();
            output(&serde_json::json!({"read_only":true,"state":status.state,"ownership":status.ownership,"retained":retained,"reasons":status.reasons,
                "retrieval":"Preserve held files and original anchors in place. Copy an identity-verified retained path to a NEW operator-owned filename with no-clobber, then verify the printed SHA-256. A copy is a new inode, not the original object. No automatic deletion or GC is supported."}))?;
            return Ok(!matches!(status.state,d::RestartState::ReviewRequired|d::RestartState::Busy));
        }
        ["inspect-grant",state,requested,file] => {
            // Read-only: reports whether a grant is spent, superseded or usable without burning it.
            let requested=action(requested)?;
            let grant=match read_grant(Path::new(file)) {
                Ok(g)=>g,Err(reason)=>{
                    output(&d::GrantStatus{verdict:d::GrantVerdict::ForeignOrInvalid,reason,status:d::status(Path::new(state))})?;
                    return Ok(false);
                }
            };
            let result=d::inspect_grant(Path::new(state),&grant,requested);
            output(&result)?;
            return Ok(matches!(result.verdict,d::GrantVerdict::Usable));
        }
        ["execute",state,requested,file,flag @ ..] if flag.is_empty() || *flag==[ENABLE] => {
            let grant=read_grant(Path::new(file))?;
            let result=d::execute(Path::new(state),&grant,action(requested)?,*flag==[ENABLE])?;
            output(&result)?;
            return Ok(result.completed);
        }
        _=>return Err("R2 is OFF by default. Commands: issuer-init DIR; provision ROOT STATE PUBLIC_JSON OPERATOR --accept-contract-1-i; plan STATE PLAN_JSON SOURCE DEST [SOURCE DEST ...]; inspect-plan PLAN_JSON; enroll STATE PLAN_JSON --confirm DIGEST --enable-contract-1-i; review STATE apply|undo|retain-foreign; approve STATE SIGNER_DIR apply|undo|retain-foreign --confirm DIGEST GRANT_JSON; execute STATE apply|undo|retain-foreign GRANT_JSON --enable-contract-1-i; preview-reuse STATE NEW_STATE; reuse STATE NEW_STATE --confirm DIGEST --enable-contract-1-i; history STATE (verified closed predecessors); status STATE (read-only restart status); retained STATE (verified read-only retained-file inventory); inspect-grant STATE apply|undo|retain-foreign GRANT_JSON (read-only; never consumes). Private absolute paths required. Review Contract 1-I before provisioning.".into()),
    }
    Ok(true)
}
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    {
        match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
            Ok(true) => std::process::ExitCode::SUCCESS,
            Ok(false) => std::process::ExitCode::from(2),
            Err(e) => {
                eprintln!("R2 refused: {e}");
                std::process::ExitCode::from(2)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("R2 requires a validated Linux ext4 root; disabled on this platform.");
        std::process::ExitCode::from(2)
    }
}
