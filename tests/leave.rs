//! `leave` moves the process out of a worktree before it is removed. It changes the process
//! cwd, so this file holds exactly one test (its own process).

use berth::ops::down::leave;

#[test]
fn leave_moves_cwd_from_inside_the_worktree_to_main_and_only_then() {
    let root = tempfile::TempDir::new().unwrap();
    let main = root.path().join("main");
    let wt = root.path().join("wt");
    let deep = wt.join("a").join("b");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&deep).unwrap();
    let canon = |p: &std::path::Path| berth::sys::canonicalize(p).unwrap();

    // cwd elsewhere: untouched.
    std::env::set_current_dir(&main).unwrap();
    std::env::set_current_dir(root.path()).unwrap();
    leave(&main, &wt);
    assert_eq!(canon(&std::env::current_dir().unwrap()), canon(root.path()));

    // cwd deep inside the worktree: moved to main.
    std::env::set_current_dir(&deep).unwrap();
    leave(&main, &wt);
    assert_eq!(canon(&std::env::current_dir().unwrap()), canon(&main));

    // a sibling sharing the name prefix is not "inside".
    let sibling = root.path().join("wt-other");
    std::fs::create_dir_all(&sibling).unwrap();
    std::env::set_current_dir(&sibling).unwrap();
    leave(&main, &wt);
    assert_eq!(canon(&std::env::current_dir().unwrap()), canon(&sibling));
}
