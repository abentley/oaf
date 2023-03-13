use std::env::set_current_dir;
use std::fs::File;
use std::io::Write;
use std::process;

use git2::Repository;
use tempfile::TempDir;

use oaf::branch::PipeNext;
use oaf::commands::{Runnable, SwitchNext};
use oaf::git::{
    get_current_branch, make_git_command, LocalBranchName, OpenRepoError, ReferenceSpec,
};

trait RunFallible {
    fn run_check(&mut self);
}
impl RunFallible for process::Command {
    fn run_check(&mut self) {
        assert!(self.status().unwrap().success());
    }
}

#[test]
fn switch_next_create_existing() {
    let work_dir = TempDir::new().expect("Could not create temporary directory");
    set_current_dir(&work_dir).expect("Failed to chdir to working directory");
    make_git_command(&["init"]).run_check();
    make_git_command(&["config", "--worktree", "user.email", "jrandom@example.com"]).run_check();
    make_git_command(&["config", "--worktree", "user.name", "J. Random Hacker"]).run_check();
    let mut file = File::create("foo.txt").unwrap();
    file.write_all(b"bar").expect("Failed to write file.");
    make_git_command(&["add", "foo.txt"]).run_check();
    make_git_command(&["commit", "-am", "initial commit"]).run_check();
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
