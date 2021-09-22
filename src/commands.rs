use super::git::{
    branch_setting, get_current_branch, make_git_command, output_to_string, run_git_command,
    set_head, setting_exists,
};
use super::worktree::{base_tree, get_toplevel, stash_switch, Commit, GitStatus, SwitchErr};
use enum_dispatch::enum_dispatch;
use std::env;
use std::os::unix::process::CommandExt;
use std::str::FromStr;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
pub struct Cat {
    #[structopt(long, short, default_value = "")]
    tree: String,
    input: String,
}

enum TreeFile<'a> {
    IndexFile { stage: u8, path: &'a str },
    CommitFile { commit: &'a str, path: &'a str },
}

fn format_tree_file(tree_file: &TreeFile) -> String {
    match tree_file {
        TreeFile::IndexFile { stage, path } => {
            format!(":{}:./{}", stage, path)
        }
        TreeFile::CommitFile { commit, path } => {
            format!("{}:./{}", commit, path)
        }
    }
}

impl ArgMaker for Cat {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let tree_file = if &self.tree == "index" {
            TreeFile::IndexFile {
                stage: 0,
                path: &self.input,
            }
        } else {
            TreeFile::CommitFile {
                commit: &self.tree,
                path: &self.input,
            }
        };
        Ok(["show", &format_tree_file(&tree_file)]
            .iter()
            .map(|s| s.to_string())
            .collect())
    }
}

#[derive(Debug, StructOpt)]
pub struct Diff {
    /// Source commit / branch to compare.  (Defaults to HEAD.)
    #[structopt(long, short)]
    source: Option<Commit>,
    /// Target commit / branch to compare.  (Defaults to working directory.)
    #[structopt(long, short)]
    target: Option<Commit>,
    /// Use the meyers diff algorithm.  (Faster, can produce more confusing diffs.)
    #[structopt(long)]
    myers: bool,
    /// Emit modified filenames only, not diffs.
    #[structopt(long)]
    name_only: bool,
    /// Files to compare.  If empty, all are compared.
    path: Vec<String>,
}

impl ArgMaker for Diff {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let mut cmd_args = vec!["diff"];
        if !self.myers {
            cmd_args.push("--patience");
        }
        if self.name_only {
            cmd_args.push("--name-only");
        }
        let mut cmd_args: Vec<String> = cmd_args.iter().map(|s| s.to_string()).collect();
        cmd_args.push(match &self.source {
            Some(source) => source.sha.to_owned(),
            None => base_tree(),
        });
        if let Some(target) = &self.target {
            cmd_args.push(target.sha.to_owned());
        }
        if !self.path.is_empty() {
            cmd_args.push("--".to_string());
            cmd_args.extend(self.path);
        }
        Ok(cmd_args)
    }
}

#[derive(Debug, StructOpt)]
pub struct Log {
    /// The range of commits to display.  Defaults to all of HEAD.
    #[structopt(long, short)]
    range: Option<String>,
    /// If enabled, show patches for commits.
    #[structopt(long, short)]
    patch: bool,
    /// If enabled, show merged commits.  (Merge commits are always shown.)
    #[structopt(long, short)]
    include_merged: bool,
    /// Show only commits in which these files were modified.  (No filter if none supplied.)
    path: Vec<String>,
}

impl ArgMaker for Log {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let mut cmd_args = vec!["log"];
        if !self.include_merged {
            cmd_args.push("--first-parent");
        }
        if self.patch {
            cmd_args.push("--patch");
        }
        if let Some(range) = &self.range {
            cmd_args.push(range);
        }
        let mut cmd_args: Vec<String> = cmd_args.iter().map(|s| s.to_string()).collect();
        if !self.path.is_empty() {
            cmd_args.push("--".to_string());
            cmd_args.extend(self.path)
        }
        Ok(cmd_args)
    }
}

#[derive(Debug, StructOpt)]
pub struct Merge {
    source: Commit,
}

impl ArgMaker for Merge {
    fn make_args(self) -> Result<Vec<String>, i32> {
        Ok(["merge", "--no-commit", "--no-ff", &self.source.sha]
            .iter()
            .map(|s| s.to_string())
            .collect())
    }
}

#[derive(Debug, StructOpt)]
pub struct MergeDiff {
    /// The branch you would merge into.  (Though any commitish will work.)
    target: Commit,
    /// Use the meyers diff algorithm.  (Faster, can produce more confusing diffs.)
    #[structopt(long)]
    myers: bool,
    /// Emit modified filenames only, not diffs.
    #[structopt(long)]
    name_only: bool,
    path: Vec<String>,
}

impl ArgMaker for MergeDiff {
    fn make_args(self) -> Result<Vec<String>, i32> {
        if let Err(..) = Commit::from_str("HEAD") {
            eprintln!("Cannot merge-diff: no commits in HEAD.");
            return Err(1);
        }
        Diff {
            source: Some(self.target.find_merge_base("HEAD")),
            target: None,
            myers: self.myers,
            name_only: self.name_only,
            path: self.path,
        }
        .make_args()
    }
}

#[derive(Debug, StructOpt)]
pub struct Pull {
    remote: Option<String>,
    source: Option<String>,
}

impl ArgMaker for Pull {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let mut cmd_args = vec!["pull", "--ff-only"];
        if let Some(remote) = &self.remote {
            cmd_args.push(remote);
        }
        if let Some(source) = &self.source {
            cmd_args.push(source);
        }
        Ok(cmd_args.iter().map(|s| s.to_string()).collect())
    }
}

#[derive(Debug, StructOpt)]
pub struct Restore {
    /// Tree/commit/branch containing the version of the file to restore.
    #[structopt(long, short)]
    source: Option<String>,
    /// File(s) to restore
    #[structopt(required = true)]
    path: Vec<String>,
}

impl ArgMaker for Restore {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let source = if let Some(source) = self.source {
            source
        } else {
            if let Err(..) = Commit::from_str("HEAD") {
                eprintln!("Cannot restore: no commits in HEAD.");
                return Err(1);
            }
            "HEAD".to_string()
        };
        let cmd_args = vec!["checkout", &source];
        let mut cmd_args: Vec<String> = cmd_args.iter().map(|s| s.to_string()).collect();
        if !self.path.is_empty() {
            cmd_args.push("--".to_string());
            cmd_args.extend(self.path);
        }
        Ok(cmd_args)
    }
}

#[enum_dispatch]
#[derive(Debug, StructOpt)]
pub enum RewriteCommand {
    /// Output the contents of a file for a given tree.
    Cat,
    /// Compare one tree to another.
    Diff,
    /// Produce a log of the commit range.  By default, exclude merged commits.
    Log,
    /// Apply the changes from another branch (or commit) to the current tree.
    Merge,
    /**
    Display a diff predicting the changes that would be merged if you merged your working tree.

    The diff includes uncommitted changes, unlike `git diff <target>...`.  It is produced by
    diffing the working tree against the merge base of <target> and HEAD.
    */
    MergeDiff,
    /// Transfer remote changes to the local repository and working tree
    Pull,
    /// Restore the contents of a file to a previous value
    Restore,
}

#[enum_dispatch(RewriteCommand)]
pub trait ArgMaker {
    fn make_args(self) -> Result<Vec<String>, i32>;
}

#[enum_dispatch]
#[derive(Debug, StructOpt)]
pub enum NativeCommand {
    /// Record the current contents of the working tree.
    Commit(CommitCmd),
    /// Transfer local changes to a remote repository and branch.
    Push,
    /**
    Switch to a branch, stashing and restoring pending changes.

    Outstanding changes are stored under "refs/branch-wip/", in the repo, with the branch's name
    as the suffix.  For example, pending changes for a branch named "foo" would
    be stored in a ref named "refs/branch-wip/foo".
    */
    Switch,
    /**
    Perform a fake merge of the specified branch/commit, leaving the local tree unmodified.

    This effectively gives the contents of the latest commit precedence over the contents of the
    source commit.
    */
    FakeMerge,
    /// Disabled to prevent accidentally discarding stashed changes.
    Checkout,
    /// Show the status of changed and unknown files in the working tree.
    Status,
}
#[derive(Debug, StructOpt)]
pub struct CommitCmd {
    #[structopt(long, short)]
    message: Option<String>,
    /// Amend the HEAD commit.
    #[structopt(long)]
    amend: bool,
    #[structopt(long, short)]
    no_verify: bool,
    ///Commit only changes in the index.
    #[structopt(long)]
    no_all: bool,
    #[structopt(long)]
    no_strict: bool,
}

impl ArgMaker for CommitCmd {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let mut cmd_args = vec!["commit"];
        if !self.no_all {
            cmd_args.push("--all")
        }
        if let Some(message) = &self.message {
            cmd_args.push("--message");
            cmd_args.push(message);
        }
        if self.amend {
            cmd_args.push("--amend");
        }
        if self.no_verify {
            cmd_args.push("--no-verify");
        }
        Ok(cmd_args.iter().map(|s| s.to_string()).collect())
    }
}

#[enum_dispatch(NativeCommand)]
pub trait Runnable {
    fn run(self) -> i32;
}

impl Runnable for CommitCmd {
    fn run(self) -> i32 {
        if !self.no_strict {
            let status = GitStatus::new();
            let untracked = status.untracked_filenames();
            if !untracked.is_empty() {
                eprintln!("Untracked files are present:");
                for entry in untracked {
                    eprintln!("{}", entry);
                }
                eprintln!("You can add them with \"nit add\", ignore them by editing .gitignore, or use --no-strict.");
                return 1;
            }
        }
        make_git_command(&match self.make_args() {
            Ok(args) => args,
            Err(status) => return status,
        })
        .exec();
        0
    }
}

#[derive(Debug, StructOpt)]
pub struct Push {}

impl Runnable for Push {
    fn run(self) -> i32 {
        let branch = get_current_branch();
        if setting_exists(&branch_setting(&branch, "remote")) {
            if !setting_exists(&branch_setting(&branch, "merge")) {
                panic!("Branch in unsupported state");
            }
            make_git_command(&["push"]).exec();
        } else {
            if let Ok(head) = Commit::from_str("HEAD") {
                head
            } else {
                eprintln!("Cannot push: no commits in HEAD.");
                return 1;
            };
            make_git_command(&["push", "-u", "origin", "HEAD"]).exec();
        }
        0
    }
}

#[derive(Debug, StructOpt)]
pub struct Switch {
    /// The branch to switch to.
    branch: String,
    #[structopt(long, short)]
    create: bool,
}

impl Runnable for Switch {
    fn run(self) -> i32 {
        match stash_switch(&self.branch, self.create) {
            Ok(()) => 0,
            Err(SwitchErr::BranchInUse { path }) => {
                println!("Branch {} is already in use at {}", self.branch, path);
                1
            }
            Err(SwitchErr::AlreadyExists) => {
                eprintln!("Branch {} already exists", self.branch);
                1
            }
            Err(SwitchErr::NotFound) => {
                eprintln!("Branch {} not found", self.branch);
                1
            }
        }
    }
}

#[derive(Debug, StructOpt)]
pub struct FakeMerge {
    /// The source for the fake merge.
    source: Commit,
    /// The message to use for the fake merge.  (Default: "Fake merge.")
    #[structopt(long, short)]
    message: Option<String>,
}

impl Runnable for FakeMerge {
    fn run(self) -> i32 {
        let head = if let Ok(head) = Commit::from_str("HEAD") {
            head
        } else {
            eprintln!("Cannot fake-merge: no commits in HEAD.");
            return 1;
        };
        let message = if let Some(msg) = &self.message {
            &msg
        } else {
            "Fake merge."
        };
        let output = run_git_command(&[
            "commit-tree",
            "-p",
            "HEAD",
            "-p",
            &self.source.sha,
            &head.get_tree_reference(),
            "-m",
            message,
        ])
        .expect("Could not generate commit.");
        let fm_hash = output_to_string(&output);
        set_head(&fm_hash);
        0
    }
}

#[derive(Debug, StructOpt)]
pub struct Checkout {
    /// The branch to switch to.
    branch_name: String,
    #[structopt(long, short)]
    branch: bool,
}

impl Runnable for Checkout {
    fn run(self) -> i32 {
        eprintln!(
            "Please use \"switch\" to change branches or \"restore\" to restore files to a known state"
        );
        1
    }
}

#[derive(Debug, StructOpt)]
pub struct Status {}

impl Runnable for Status {
    fn run(self) -> i32 {
        let gs = GitStatus::new();
        let mut gs_iter = gs.iter();
        let cwd = env::current_dir().expect("Need cwd");
        let top = get_toplevel();
        let top_rel = cwd.strip_prefix(top).unwrap();
        for se in gs_iter.fix_removals() {
            let out = se.format_entry(&top_rel);
            println!("{}", out);
        }
        1
    }
}
