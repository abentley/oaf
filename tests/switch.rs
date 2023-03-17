use std::env::set_current_dir;
use std::fs::File;
use std::io::Write;
use std::process;

use tempfile::TempDir;

use oaf::git::{get_current_branch, make_git_command, show_ref_match, BranchyName};
use oaf::worktree::{stash_switch, SwitchErr, SwitchType};

trait RunFallible {
    fn run_check(&mut self);
}
impl RunFallible for process::Command {
    fn run_check(&mut self) {
        assert!(self.status().unwrap().success());
    }
}

fn init_repo() -> TempDir {
    let work_dir = TempDir::new().expect("Could not create temporary directory");
    set_current_dir(&work_dir).expect("Failed to chdir to working directory");
    make_git_command(&["init", "-b", "main"]).run_check();
    make_git_command(&["config", "--worktree", "user.email", "jrandom@example.com"]).run_check();
    make_git_command(&["config", "--worktree", "user.name", "J. Random Hacker"]).run_check();
    let mut file = File::create("foo.txt").unwrap();
    file.write_all(b"bar").expect("Failed to write file.");
    make_git_command(&["add", "foo.txt"]).run_check();
    make_git_command(&["commit", "-am", "initial commit"]).run_check();
    work_dir
}

#[test]
fn non_existent() {
    let _work_dir = init_repo();
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
