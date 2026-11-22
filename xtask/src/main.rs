//! `cargo xtask version [set X.Y.Z]` and `cargo xtask ico <out.ico> <png>...`: the repository's
//! own tasks, run by the release workflow and `packaging/icons.sh`.
//!
//! `[workspace.package] version` in the root `Cargo.toml` is the single source of the app's
//! version. `version` prints it; `version set X.Y.Z[-pre]` rewrites it and refreshes the
//! workspace's own entries in `Cargo.lock`.

mod ico;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let ok = match args.as_slice() {
        ["version"] => print_version(&workspace_root()),
        ["version", "set", v] => set_version(&workspace_root(), v),
        ["ico", rest @ ..] => ico::run(rest).map_err(|e| eprintln!("ico: {e}")).is_ok(),
        _ => {
            eprintln!("usage: cargo xtask version [set X.Y.Z] | ico <out.ico> <png>...");
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn workspace_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent().map(Path::to_path_buf).unwrap_or(here)
}

fn print_version(root: &Path) -> bool {
    let path = root.join("Cargo.toml");
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| current(&t))
    {
        Ok((_, v)) => {
            println!("{v}");
            true
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            false
        }
    }
}

fn set_version(root: &Path, v: &str) -> bool {
    let path = root.join("Cargo.toml");
    let result = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| rewrite(&text, v).map(|(new, old)| (text, new, old)));
    let (_, new, old) = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return false;
        }
    };
    if let Err(e) = std::fs::write(&path, new) {
        eprintln!("can't write {}: {e}", path.display());
        return false;
    }
    eprintln!("version {old} -> {v}");
    run("cargo", &["update", "--workspace", "--offline"])
        || run("cargo", &["update", "--workspace"])
}

fn run(cmd: &str, args: &[&str]) -> bool {
    eprintln!("$ {cmd} {}", args.join(" "));
    Command::new(cmd)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

/// The line index and value of `version = "..."` inside `[workspace.package]`.
fn current(text: &str) -> Result<(usize, String), String> {
    let mut in_pkg = false;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[workspace.package]";
            continue;
        }
        if in_pkg
            && let Some(v) = t
                .strip_prefix("version = \"")
                .and_then(|r| r.strip_suffix('"'))
        {
            return Ok((i, v.to_string()));
        }
    }
    Err("no version in [workspace.package]".to_string())
}

fn valid(v: &str) -> bool {
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let numeric = |p: &&str| {
        !p.is_empty()
            && p.chars().all(|c| c.is_ascii_digit())
            && (p.len() == 1 || !p.starts_with('0'))
    };
    let pre_ok = pre.is_none_or(|p| {
        p.split('.')
            .all(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    });
    parts.len() == 3 && parts.iter().all(numeric) && pre_ok
}

/// `text` with the workspace version replaced by `v`, and the old version.
fn rewrite(text: &str, v: &str) -> Result<(String, String), String> {
    if !valid(v) {
        return Err(format!(
            "version must look like 1.2.3 or 1.2.3-rc.1, not {v:?}"
        ));
    }
    let (line_no, old) = current(text)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(line) = lines.get_mut(line_no) {
        *line = format!("version = \"{v}\"");
    }
    let mut new = lines.join("\n");
    if text.ends_with('\n') {
        new.push('\n');
    }
    Ok((new, old))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\n\n[dependencies]\nfoo = { version = \"9.9.9\" }\n";

    #[test]
    fn set_changes_only_the_workspace_version() {
        let (new, old) = rewrite(MANIFEST, "0.2.0-rc.1").expect("valid version");
        assert_eq!(old, "0.1.0");
        assert_eq!(current(&new).expect("still has a version").1, "0.2.0-rc.1");
        assert!(new.contains("foo = { version = \"9.9.9\" }"));
        assert!(new.ends_with('\n'));
    }

    #[test]
    fn set_refuses_what_is_not_a_semver_version() {
        for bad in [
            "1.2", "1.2.3.4", "v1.2.3", "01.2.3", "1.2.3-", "1.2.3 ", "1.2.x",
        ] {
            assert!(rewrite(MANIFEST, bad).is_err(), "{bad:?} was accepted");
        }
    }
}
