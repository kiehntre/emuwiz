//! Dedicated short-lived R2 operator process.
fn main() -> std::process::ExitCode {
    // A sealing-capable build: registered so it is never taken for an older one.
    let _sealed_writer = archivefs_core::dat::rename_apply::writer_gate::register_at_startup()
        .map_err(|error| eprintln!("emuwiz-r2: {error}"))
        .ok();
    archivefs_cli::r2_operator::main()
}
