//! The release files must agree with each other (no network, reads the repo's own files).

use std::fs;
use std::path::{Path, PathBuf};

const PLATFORMS: [(&str, &str, &str, &str); 5] = [
    ("linux-x64", "x86_64-unknown-linux-musl", "linux", "x64"),
    (
        "linux-arm64",
        "aarch64-unknown-linux-musl",
        "linux",
        "arm64",
    ),
    ("darwin-x64", "x86_64-apple-darwin", "darwin", "x64"),
    ("darwin-arm64", "aarch64-apple-darwin", "darwin", "arm64"),
    ("win32-x64", "x86_64-pc-windows-msvc", "win32", "x64"),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_toml(path: &Path) -> toml::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cargo_version() -> String {
    read_toml(&root().join("Cargo.toml"))["package"]["version"]
        .as_str()
        .unwrap()
        .to_string()
}

fn npm_dir(name: &str) -> PathBuf {
    root().join("npm").join(name)
}

fn string_list(v: &serde_json::Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect()
}

#[test]
fn cargo_package_is_publishable() {
    let cargo = read_toml(&root().join("Cargo.toml"));
    let package = &cargo["package"];
    assert!(
        package.get("publish").is_none(),
        "publish = false must be gone"
    );
    for key in [
        "readme",
        "keywords",
        "categories",
        "include",
        "description",
        "license",
    ] {
        assert!(package.get(key).is_some(), "[package] needs {key}");
    }
    let include: Vec<&str> = package["include"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        include
            .iter()
            .all(|p| !p.contains("tests") && !p.contains("npm")),
        "{include:?}"
    );
}

#[test]
fn hoist_toml_targets_are_the_five_platforms() {
    let hoist = read_toml(&root().join("hoist.toml"));
    let publish = &hoist["publish"];
    assert!(publish["crates"].as_table().unwrap().is_empty());
    let release = &publish["github_release"];
    assert_eq!(release["bin"].as_str(), Some("berth"));
    let mut targets: Vec<&str> = release["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    targets.sort();
    let mut want: Vec<&str> = PLATFORMS.iter().map(|p| p.1).collect();
    want.sort();
    assert_eq!(targets, want);
}

#[test]
fn launcher_and_platform_packages_agree_with_cargo() {
    let version = cargo_version();
    let launcher = read_json(&npm_dir("berth").join("package.json"));
    assert_eq!(launcher["name"], "@keelstackhq/berth");
    assert_eq!(launcher["version"].as_str(), Some(version.as_str()));
    assert_eq!(launcher["bin"]["berth"], "bin/berth.js");

    let opt = launcher["optionalDependencies"].as_object().unwrap();
    let mut keys: Vec<String> = opt.keys().cloned().collect();
    keys.sort();
    let mut want: Vec<String> = PLATFORMS
        .iter()
        .map(|p| format!("@keelstackhq/berth-{}", p.0))
        .collect();
    want.sort();
    assert_eq!(keys, want);
    for (name, value) in opt {
        assert_eq!(
            value.as_str(),
            Some(version.as_str()),
            "optionalDependencies {name}"
        );
    }

    let hoist = read_toml(&npm_dir("berth").join("hoist.toml"));
    assert_eq!(hoist["publish"]["npm"]["launcher"].as_bool(), Some(true));

    for (platform, _target, os, cpu) in PLATFORMS {
        let pkg = read_json(&npm_dir(&format!("berth-{platform}")).join("package.json"));
        assert_eq!(pkg["name"], format!("@keelstackhq/berth-{platform}"));
        assert_eq!(
            pkg["version"].as_str(),
            Some(version.as_str()),
            "{platform}"
        );
        assert_eq!(string_list(&pkg["os"]), [os], "{platform} os");
        assert_eq!(string_list(&pkg["cpu"]), [cpu], "{platform} cpu");
        assert_eq!(string_list(&pkg["files"]), ["bin/"], "{platform} files");
        assert_eq!(pkg["publishConfig"]["access"], "public");
    }
}

#[test]
fn every_npm_package_is_a_known_one() {
    let mut found: Vec<String> = fs::read_dir(root().join("npm"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    found.sort();
    let mut want: Vec<String> = PLATFORMS.iter().map(|p| format!("berth-{}", p.0)).collect();
    want.push("berth".into());
    want.sort();
    assert_eq!(found, want);
}

#[test]
fn pnpm_workspace_lists_the_npm_packages() {
    let text = fs::read_to_string(root().join("pnpm-workspace.yaml")).unwrap();
    assert!(text.lines().any(|l| l.trim() == "- npm/*"), "{text}");
    assert!(text.contains("linkWorkspacePackages: true"));
}
