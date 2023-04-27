mod common;

use oaf::branch::find_target_branchname;
use oaf::git::{BranchName, LocalBranchName};
use oaf::worktree::set_target;

#[test]
fn from_settings() {
    let _work_dir = common::init_blank_repo();
    set_target(
        &LocalBranchName::from("main".to_owned()),
        &BranchName::Local(LocalBranchName::from("missing".to_owned())),
    ).unwrap();
    let target = find_target_branchname(LocalBranchName::from("main".to_owned()));
    eprintln!("{:?}", target);
    assert!(target == Ok(None));
}
