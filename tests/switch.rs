use std::fs::File;
use std::io::Write;

use oaf::git::{get_current_branch, make_git_command, show_ref_match, BranchyName};
use oaf::worktree::{stash_switch, SwitchErr, SwitchType};
mod common;
use common::RunFallible;

#[test]
fn non_existent() {
    let _work_dir = common::init_repo();
    let mut file = File::create("bar.txt").unwrap();
    file.write_all(b"baz").expect("Failed to write file.");
    make_git_command(&["add", "bar.txt"]).run_check();
    let branchy_name = BranchyName::LocalBranch("foo".to_string().into());
    if let Err(SwitchErr::NotFound) = stash_switch(SwitchType::WithStash(branchy_name)) {
    } else {
        panic!("Did not return NotFound");
    }
    assert!(get_current_branch().unwrap().branch_name() == "main");
    assert!(show_ref_match("refs/branch-wip/main").len() == 0);
}
