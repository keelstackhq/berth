use berth::sys::sh_from_exec_path;

fn has<'a>(files: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
    move |p| files.contains(&p)
}

#[test]
fn sh_found_next_to_git_for_windows() {
    let ep = r"C:\Program Files\Git\mingw64\libexec\git-core";
    let f = [r"C:\Program Files\Git\bin\sh.exe"];
    assert_eq!(sh_from_exec_path(ep, has(&f)).as_deref(), Some(f[0]));
}

#[test]
fn sh_falls_back_to_usr_bin_and_forward_slashes() {
    let ep = "C:/Program Files/Git/mingw64/libexec/git-core";
    let f = ["C:/Program Files/Git/usr/bin/sh.exe"];
    assert_eq!(sh_from_exec_path(ep, has(&f)).as_deref(), Some(f[0]));
}

#[test]
fn sh_prefers_bin_and_handles_variants() {
    for v in ["mingw32", "clangarm64"] {
        let ep = format!(r"D:\G\{v}\libexec\git-core");
        let f = [r"D:\G\usr\bin\sh.exe", r"D:\G\bin\sh.exe"];
        assert_eq!(sh_from_exec_path(&ep, has(&f)).as_deref(), Some(f[1]));
    }
}

#[test]
fn sh_none_when_absent() {
    assert_eq!(
        sh_from_exec_path(r"C:\Git\mingw64\libexec\git-core", |_| false),
        None
    );
}

#[test]
fn hostname_is_not_empty() {
    assert!(!berth::sys::hostname().is_empty());
}

#[test]
fn own_pid_is_alive_and_other_host_is_assumed_alive() {
    let host = berth::sys::hostname();
    assert!(berth::sys::pid_alive(std::process::id(), &host));
    assert!(berth::sys::pid_alive(1, "some-other-host.invalid"));
}
