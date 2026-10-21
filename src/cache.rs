//! Build-cache helpers (spec §12): seed the turbo cache from the main checkout.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{CacheCfg, Remote, Seed};
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

/// A running `hoard serve`, read live from its port and token files (spec §12). `token` is a
/// secret: it goes into hook environments only, never into output, logs or the manifest.
pub struct Hoard {
    pub api: String,
    pub token_file: PathBuf,
    pub token: String,
}

/// The hoard server of `main`, for `remote = "hoard"` only. When no server is running, one
/// `warning:` goes to stderr and the result is `None`; call it once per command.
pub fn hoard(main: &Path, cache: &CacheCfg) -> Option<Hoard> {
    if cache.remote != Remote::Hoard {
        return None;
    }
    match locate(main, cache) {
        Ok(h) => Some(h),
        Err((file, why)) => {
            eprintln!(
                "warning: no running hoard server ({} {why}); start hoard serve --storage fs in {}",
                file.display(),
                main.display()
            );
            None
        }
    }
}

/// Like `hoard` without the warning (for `doctor`, which reports the absence as an item).
pub fn hoard_quiet(main: &Path, cache: &CacheCfg) -> Option<Hoard> {
    if cache.remote != Remote::Hoard {
        return None;
    }
    locate(main, cache).ok()
}

/// The server's port and token files, or the file that is missing, invalid or empty.
fn locate(main: &Path, cache: &CacheCfg) -> Result<Hoard, (PathBuf, &'static str)> {
    let dir = main.join(&cache.hoard_dir);
    let port_file = dir.join("port");
    let token_file = dir.join("token");
    let port = match fs::read_to_string(&port_file) {
        Ok(text) => text.trim().parse::<u16>().map_err(|_| "invalid"),
        Err(_) => Err("missing"),
    };
    let token = match fs::read_to_string(&token_file) {
        Ok(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        Ok(_) => Err("empty"),
        Err(_) => Err("missing"),
    };
    match (port, token) {
        (Ok(port), Ok(token)) => Ok(Hoard {
            api: format!("http://127.0.0.1:{port}"),
            token_file,
            token,
        }),
        (Err(why), _) => Err((port_file, why)),
        (_, Err(why)) => Err((token_file, why)),
    }
}

/// `base` plus `TURBO_API`, `TURBO_TEAM` and `TURBO_TOKEN` when a server runs. For hooks only.
pub fn hook_env(
    base: &BTreeMap<String, String>,
    hoard: Option<&Hoard>,
) -> BTreeMap<String, String> {
    let mut env = base.clone();
    if let Some(h) = hoard {
        env.insert("TURBO_API".into(), h.api.clone());
        env.insert("TURBO_TEAM".into(), "hoard".into());
        env.insert("TURBO_TOKEN".into(), h.token.clone());
    }
    env
}
