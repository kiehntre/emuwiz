//! Standalone harness for smoke-testing the isolated PDF renderer. Production
//! GUI launchers run the same helper entrypoint in their own executable.
fn main() {
    if let Err(error) = archivefs_core::manual_document::pdf_render::run_helper() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
