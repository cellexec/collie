//! collie: `herdr update` aktualisiert den Quell-Checkout, aus dem dieses
//! Binary gebaut wurde: `git pull --ff-only` auf master, danach
//! `cargo build --release --locked`. Upstream-Releases lädt collie nie, sie
//! würden den Fork ersetzen. Den laufenden Server stellt erst
//! `herdr live-restart` auf das neue Binary um.

use std::path::{Path, PathBuf};
use std::process::Command;

const BRANCH: &str = "master";
const USAGE: &str = "usage: herdr update\n\npull the collie checkout this binary was built from (fast-forward on master), rebuild the release binary, then run `herdr live-restart`";

pub fn run(args: &[String]) -> i32 {
    match args {
        [] => {}
        [flag] if flag == "-h" || flag == "--help" => {
            println!("{USAGE}");
            return 0;
        }
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    }
    match update() {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("update failed: {err}");
            1
        }
    }
}

fn update() -> Result<(), String> {
    let exe = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .map_err(|err| format!("cannot locate the running binary: {err}"))?;
    let repo = checkout_root(&exe)?;
    eprintln!("checkout: {}", repo.display());

    let branch = git(&repo, &["symbolic-ref", "--short", "HEAD"]).map_err(|_| {
        format!(
            "{} is not on a branch; check out {BRANCH} first",
            repo.display()
        )
    })?;
    if branch != BRANCH {
        return Err(format!(
            "{} is on branch {branch}; collie updates only from {BRANCH}",
            repo.display()
        ));
    }

    let before = git(&repo, &["rev-parse", "--short", "HEAD"])?;
    run_step(&repo, "git", &["pull", "--ff-only"])?;
    let after = git(&repo, &["rev-parse", "--short", "HEAD"])?;
    if before == after {
        eprintln!("{BRANCH} is already at {after}");
    } else {
        eprintln!("{BRANCH}: {before} -> {after}");
        let log = git(&repo, &["log", "--oneline", &format!("{before}..{after}")])?;
        eprintln!("{log}");
    }

    run_step(&repo, "cargo", &["build", "--release", "--locked"])?;
    eprintln!(
        "build done. run `herdr live-restart` to move the running session to the new binary."
    );
    Ok(())
}

/// Wurzel des Git-Checkouts, in dem das Binary liegt (z. B.
/// `<repo>/target/release/herdr`).
fn checkout_root(exe: &Path) -> Result<PathBuf, String> {
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    git(dir, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| {
            format!(
                "{} is not inside a git checkout; reinstall collie with scripts/collie/install.sh",
                exe.display()
            )
        })
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Führt einen Schritt mit sichtbarer Ausgabe im Checkout aus.
fn run_step(dir: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    eprintln!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .current_dir(dir)
        .args(args)
        .status()
        .map_err(|err| format!("cannot run {program}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`{program} {}` exited with {status}",
            args.join(" ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_arguments() {
        assert_eq!(run(&["--handoff".to_string()]), 2);
    }

    #[test]
    fn finds_checkout_root_from_a_file_inside_it() {
        // Ein Verzeichnis, das in jedem Checkout existiert; `target/` fehlt in
        // Worktrees mit gemeinsamem Target-Verzeichnis.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_exe = manifest.join("src").join("herdr");
        let root = checkout_root(&fake_exe).unwrap();
        assert_eq!(
            root.canonicalize().unwrap(),
            manifest.canonicalize().unwrap()
        );
    }

    #[test]
    fn reports_binaries_outside_a_checkout() {
        let err = checkout_root(Path::new("/herdr")).unwrap_err();
        assert!(err.contains("not inside a git checkout"), "{err}");
    }
}
