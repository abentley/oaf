use std::env::set_current_dir;
use std::fs::File;
use std::io::Write;
use std::process;

use tempfile::TempDir;

use oaf::commands::{Runnable, SwitchNext};
use oaf::git::make_git_command;

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
    let mut file = File::create("foo.txt").unwrap();
    file.write_all(b"bar");
    make_git_command(&["add", "foo.txt"]).run_check();
    make_git_command(&["commit", "-am", "initial commit"]).run_check();
    make_git_command(&["status"]).run_check();
    SwitchNext::new(false, Some("next1")).run();
    make_git_command(&["switch", "main"]).run_check();
    let status = SwitchNext::new(false, Some("next2")).run();
    assert!(status == 0)
}
