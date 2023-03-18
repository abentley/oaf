use std::env::set_current_dir;
use std::fs::File;
use std::io::Write;
use std::process;
use tempfile::TempDir;

use oaf::git::make_git_command;

pub trait RunFallible {
    fn run_check(&mut self);
}

impl RunFallible for process::Command {
    fn run_check(&mut self) {
        assert!(self.status().unwrap().success());
    }
}

#[allow(dead_code)]
pub fn init_blank_repo() -> TempDir {
    let work_dir = TempDir::new().expect("Could not create temporary directory");
    make_git_command(&[
        "-C",
        &work_dir.path().to_string_lossy(),
        "init",
        "-b",
        "main",
    ])
    .current_dir(&work_dir)
    .run_check();
    work_dir
}

#[allow(dead_code)]
pub fn init_repo_no_chdir() -> TempDir {
    let work_dir = init_blank_repo();
    make_git_command(&["config", "--worktree", "user.email", "jrandom@example.com"])
        .current_dir(&work_dir)
        .run_check();
    make_git_command(&["config", "--worktree", "user.name", "J. Random Hacker"])
        .current_dir(&work_dir)
        .run_check();
    let mut file = File::create(work_dir.path().join("foo.txt")).unwrap();
    file.write_all(b"bar").expect("Failed to write file.");
    make_git_command(&["add", "foo.txt"])
        .current_dir(&work_dir)
        .run_check();
    make_git_command(&["commit", "-am", "initial commit"])
        .current_dir(&work_dir)
        .run_check();
    set_current_dir(&work_dir).expect("Failed to chdir to working directory");
    work_dir
}

#[allow(dead_code)]
pub fn init_repo() -> TempDir {
    let work_dir = init_repo_no_chdir();
    set_current_dir(&work_dir).expect("Failed to chdir to working directory");
    work_dir
}
