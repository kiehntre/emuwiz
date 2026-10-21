//! Builds the disposable verified-game profile at a path the caller owns, for
//! the Blue Xvfb smoke harness. Everything goes through the production
//! pipeline (see `archivefs_core::verified_game_fixture`); nothing outside the
//! given root is touched.
//!
//!   verified_game_profile build <root> [--fake-retroarch]
//!   verified_game_profile verify <root>     # exit 0 iff still verified
//!
//! `verify` reopens the database afresh, so running it after Blue has been
//! started and stopped on the same root proves the identity survived.

use archivefs_core::verified_game_fixture::{PLATFORM, VerifiedGameFixture};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (command, root, flags) = match arguments.as_slice() {
        [command, root, flags @ ..] => (command.as_str(), PathBuf::from(root), flags),
        _ => {
            eprintln!("usage: verified_game_profile build|verify <root> [--fake-retroarch]");
            return ExitCode::from(2);
        }
    };
    if !root.is_absolute() {
        eprintln!("the root must be an absolute path");
        return ExitCode::from(2);
    }
    let result = match command {
        "build" => build(&root, flags.iter().any(|flag| flag == "--fake-retroarch")),
        "verify" => verify(&root),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("verified_game_profile: {message}");
            ExitCode::from(1)
        }
    }
}

fn build(root: &Path, fake_retroarch: bool) -> Result<(), String> {
    let fixture = VerifiedGameFixture::build_at(root)?;
    if !fixture.is_verified()? {
        return Err("the fixture game did not verify".into());
    }
    if fake_retroarch {
        fixture.fake_retroarch("mGBA")?;
    }
    let mut lines: Vec<String> = fixture
        .child_environment()
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    lines.push(format!("ARCHIVE_ID={}", fixture.archive_id()?));
    lines.push(format!("PLATFORM={PLATFORM}"));
    lines.push(format!("DATABASE={}", fixture.database_path.display()));
    std::fs::write(root.join("profile.env"), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    println!("{}", lines.join("\n"));
    Ok(())
}

fn verify(root: &Path) -> Result<(), String> {
    let fixture = VerifiedGameFixture::reopen(root)?;
    if fixture.is_verified()? {
        println!("verified");
        Ok(())
    } else {
        Err("the game is no longer verified".into())
    }
}
