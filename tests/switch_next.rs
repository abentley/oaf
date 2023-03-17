use git2::Repository;

use oaf::branch::PipeNext;
use oaf::commands::{Runnable, SwitchNext};
use oaf::git::{
    get_current_branch, make_git_command, LocalBranchName, OpenRepoError, ReferenceSpec,
};

mod common;
use common::RunFallible;
#[test]
fn switch_next_create_existing() {
    let _work_dir = common::init_repo();
    make_git_command(&["status"]).run_check();
    SwitchNext::new(false, Some("next1")).run();
    make_git_command(&["switch", "main"]).run_check();
    assert!(get_current_branch().unwrap().branch_name() == "main");
    let status = SwitchNext::new(false, Some("next2")).run();
    assert!(status != 0);
    assert!(get_current_branch().unwrap().branch_name() == "main");
    let repo = Repository::open_from_env()
        .map_err(OpenRepoError::from)
        .expect("Can't open repo.");
    let main = LocalBranchName::from_long("refs/heads/main".to_string(), None).unwrap();
    let next = repo
        .resolve_reference_from_short_name(&PipeNext { name: main }.full())
        .unwrap();
    assert!(next.name() == Some("refs/heads/next1"))
}
