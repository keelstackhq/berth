//! Build-cache helpers (spec §12): seed the turbo cache from the main checkout.

use std::fs;
use std::path::Path;

use crate::config::Seed;
use crate::copy::{copy_one, symlink_in_parents};
use crate::git;
use crate::report::{Action, Item};
use crate::vlog;

const SRC_REL: &str = ".turbo/cache";

fn skipped(reason: impl Into<String>) -> Item {
    Item::new("cache", "turbo", Action::Skipped, reason)
}

/// Seed `<wt>/.turbo/cache` from `<main>/.turbo/cache`. Never fails `up`: any problem is a
/// `skipped` item plus a `warning:` on stderr (error kind only, no paths or file names).
pub fn seed_turbo(main: &Path, wt: &Path, mode: Seed) -> Item {
    let source = main.join(SRC_REL);
    match fs::metadata(&source) {
        Ok(m) if m.is_dir() => {}
        _ => return skipped("no .turbo/cache in the main checkout"),
    }
    match git::ignored(main, &[format!("{SRC_REL}/x")]) {
        Ok(set) if !set.is_empty() => {}
        Ok(_) => return skipped(".turbo/ is not gitignored; add it to .gitignore"),
        Err(_) => return failed("git check-ignore failed"),
    }
    match seed(&source, wt, mode) {
        Ok((linked, copied)) => Item::new(
            "cache",
            "turbo",
            Action::Create,
            format!(
                "seeded {} entries ({linked} linked, {copied} copied)",
                linked + copied
            ),
        ),
        Err(kind) => failed(&format!("seed failed: {kind}")),
    }
}

fn failed(what: &str) -> Item {
    eprintln!("warning: turbo cache not seeded: {what}");
    skipped(what)
}

/// `(linked, copied)`; the error is an `io::ErrorKind` name, nothing more.
fn seed(source: &Path, wt: &Path, mode: Seed) -> Result<(usize, usize), String> {
    let kind = |e: std::io::Error| format!("{:?}", e.kind());
    let other = |_| "Other".to_string();
    if symlink_in_parents(wt, ".turbo/cache/x").map_err(other)?
        || symlink_in_parents(wt, ".berth/tmp/x").map_err(other)?
    {
        return Err("symlink in target path".into());
    }
    let dest = wt.join(SRC_REL);
    let tmp_dir = wt.join(".berth").join("tmp");
    fs::create_dir_all(&dest).map_err(kind)?;
    fs::create_dir_all(&tmp_dir).map_err(kind)?;
    let mut entries: Vec<_> = fs::read_dir(source)
        .map_err(kind)?
        .collect::<Result<_, _>>()
        .map_err(kind)?;
    entries.sort_by_key(|e| e.file_name());
    let (mut linked, mut copied) = (0, 0);
    for entry in entries {
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".tmp") {
            continue;
        }
        let from = entry.path();
        let regular = fs::symlink_metadata(&from)
            .map(|m| m.file_type().is_file())
            .map_err(kind)?;
        if !regular {
            continue;
        }
        let to = dest.join(&name);
        if fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        vlog("seed turbo cache entry");
        if mode == Seed::Link {
            match fs::hard_link(&from, &to) {
                Ok(()) => {
                    linked += 1;
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => {}
            }
        }
        if copy_one(&from, &to, &tmp_dir).map_err(other)? {
            copied += 1;
        }
    }
    Ok((linked, copied))
}
