// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::branch::{
    check_link_branches, find_target_branchname, resolve_symbolic_reference, unlink_branch,
    BranchValidationError, NextRefErr, PipeNext, PipePrev, SiblingBranch,
};
use super::git::{
    get_current_branch, get_git_path, get_toplevel, make_git_command, output_to_string,
    run_git_command, setting_exists, BranchName, BranchyName, GitError, LocalBranchName,
    OpenRepoError, RefErr, ReferenceSpec, SettingTarget,
};
use super::worktree::{
    append_lines, base_tree, relative_path, set_target, stash_switch, Commit, CommitErr,
    CommitSpec, Commitish, ExtantRefName, GitStatus, SomethingSpec, SwitchErr, SwitchType, Tree,
    Treeish, WorktreeHead,
};
use clap::{ArgGroup, Args, Parser, Subcommand};
use enum_dispatch::enum_dispatch;
use git2::Repository;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Display;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;

fn to_strings(cmd_args: &[&str]) -> Vec<String> {
    cmd_args.iter().map(|s| s.to_string()).collect()
}

#[derive(Debug)]
pub enum MakeArgsErr {
    GetTreeRefFailure(GitError),
    MergeDiffNoHead,
    MergeDiffFindTarget(FindTargetErr),
    MergeDiffOpenRepo(OpenRepoError),
    MergeDiffNoRemembered,
    Restore(CommitErr),
}

impl fmt::Display for MakeArgsErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use MakeArgsErr::*;
        match &self {
            GetTreeRefFailure(err) => err.fmt(f),
            MergeDiffNoHead => {
                write!(f, "Cannot merge-diff: no commits in HEAD.")
            }
            MergeDiffOpenRepo(err) => err.fmt(f),
            MergeDiffFindTarget(err) => match err {
                FindTargetErr::NoCurrentBranch => {
                    write!(f, "No current branch.")
                }
                FindTargetErr::CommitErr(err) => err.fmt(f),
                FindTargetErr::NoRemembered => {
                    write!(f, "Target not supplied and no remembered target.")
                }
            },
            Restore(err) => match &err {
                CommitErr::NoCommit { .. } => {
                    write!(f, "Cannot restore: no commits in HEAD.")
                }
                CommitErr::GitError(err) => err.fmt(f),
            },
            _ => write!(f, ""),
        }
    }
}

#[derive(Debug, Args)]
/// Output the contents of a file for a given tree.
pub struct Cat {
    #[arg(long, short, default_value = "")]
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

#[enum_dispatch(RewriteCommand)]
pub trait ArgMaker {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr>;
}

impl ArgMaker for Cat {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
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
        Ok(to_strings(&["show", &format_tree_file(&tree_file)]))
    }
}

#[derive(Debug, Args)]
/// Summarize a commit or other object
pub struct Show {
    commit: Option<CommitSpec>,
    /// Emit modified filenames only, not diffs.
    #[arg(long)]
    name_only: bool,
    #[arg(long)]
    no_log: bool,
}

impl ArgMaker for Show {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let mut cmd = vec!["show", "-m", "--first-parent"];
        if self.name_only {
            cmd.push("--name-only");
        }
        if self.no_log {
            cmd.push("--pretty=");
        }
        let mut cmd = to_strings(&cmd);
        cmd.extend(self.commit.into_iter().map(|c| c.spec));
        Ok(cmd)
    }
}

#[derive(Debug, Args)]
/// Compare one tree to another.
pub struct Diff {
    /// Source commit / branch to compare.  (Defaults to HEAD.)
    #[arg(long, short)]
    source: Option<Commit>,
    /// Target commit / branch to compare.  (Defaults to working directory.)
    #[arg(long, short)]
    target: Option<Commit>,
    /// Use the meyers diff algorithm.  (Faster, can produce more confusing diffs.)
    #[arg(long)]
    myers: bool,
    /// Emit modified filenames only, not diffs.
    #[arg(long)]
    name_only: bool,
    /// Files to compare.  If empty, all are compared.
    path: Vec<String>,
}

impl ArgMaker for Diff {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let mut cmd_args = vec!["diff"];
        if !self.myers {
            cmd_args.push("--histogram");
        }
        if self.name_only {
            cmd_args.push("--name-only");
        }
        let mut cmd_args = to_strings(&cmd_args);
        cmd_args.push(match &self.source {
            Some(source) => source.sha.to_owned(),
            None => match base_tree().map(|x| x.get_tree_reference().into()) {
                Ok(tree) => tree,
                Err(err) => {
                    return Err(MakeArgsErr::GetTreeRefFailure(err));
                }
            },
        });
        cmd_args.extend(self.target.into_iter().map(|t| t.sha));
        if !self.path.is_empty() {
            cmd_args.push("--".to_string());
            cmd_args.extend(self.path);
        }
        Ok(cmd_args)
    }
}

#[derive(Debug, Args)]
/// Produce a log of the commit range.  By default, exclude merged commits.
pub struct Log {
    /// The range of commits to display.  Defaults to all of HEAD.
    #[arg(long, short)]
    range: Option<String>,
    /// If enabled, show patches for commits.
    #[arg(long, short)]
    patch: bool,
    /// If enabled, show merged commits.  (Merge commits are always shown.)
    #[arg(long, short)]
    include_merged: bool,
    /// Show only commits in which these files were modified.  (No filter if none supplied.)
    path: Vec<String>,
}

impl ArgMaker for Log {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let mut cmd_args = vec!["log"];
        if !self.include_merged {
            cmd_args.push("--first-parent");
        }
        if self.patch {
            cmd_args.extend(["-m", "--patch"]);
        }
        cmd_args.extend(self.range.iter().map(|s| s.as_str()));
        let mut cmd_args = to_strings(&cmd_args);
        if !self.path.is_empty() {
            cmd_args.push("--".to_string());
            cmd_args.extend(self.path)
        }
        Ok(cmd_args)
    }
}

#[derive(Debug)]
pub enum FindTargetErr {
    NoCurrentBranch,
    CommitErr(CommitErr),
    NoRemembered,
}

impl From<CommitErr> for FindTargetErr {
    fn from(err: CommitErr) -> Self {
        FindTargetErr::CommitErr(err)
    }
}

/**
 * Find a commit spec to merge into.
 * note: Errors could be caused by a failed status command instead of a failed parse.
 */
fn find_target() -> Result<ExtantRefName, FindTargetErr> {
    use FindTargetErr::*;
    let current = find_current_branch().transpose().ok_or(NoCurrentBranch)?;
    let result = find_target_branchname(current?)
        .transpose()
        .ok_or(NoRemembered)?;
    ExtantRefName::try_from(result).map_err(|e| e.into())
}

/// Ensure a source branch is set, falling back to remembered branch.
fn ensure_source(repo: &Repository, source: Option<CommitSpec>) -> Result<CommitSpec, i32> {
    if let Some(source) = source {
        return Ok(source);
    }
    use FindTargetErr::*;
    match find_target() {
        Ok(spec) => {
            eprintln!("Using remembered value {:?}", spec.find_shortest(repo));
            Ok(spec.into())
        }
        Err(NoCurrentBranch) => {
            eprintln!("No current branch.");
            Err(1)
        }
        Err(CommitErr(err)) => {
            eprintln!("{}", err);
            Err(1)
        }
        Err(NoRemembered) => {
            eprintln!("Source not supplied and no remembered source.");
            Err(1)
        }
    }
}

#[derive(Debug, Args)]
/// Apply the changes from another branch (or commit) to the current tree.
pub struct Merge {
    /// The branch (or commit spec) to merge from
    #[arg(long, short)]
    source: Option<CommitSpec>,
    /// Remember this source and default to it next time.
    #[arg(long)]
    remember: bool,
}

impl Runnable for Merge {
    fn run(self) -> i32 {
        let current_branch = get_current_branch().expect("Current branch");
        let repo = match Repository::open_from_env().map_err(OpenRepoError::from) {
            Ok(repo) => repo,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        let Ok(source) = ensure_source(&repo, self.source) else {
            return 1;
        };
        let args = ["merge", "--no-commit", "--no-ff", &source.spec];
        let mut cmd = make_git_command(&args);
        let Ok(status) = cmd.status() else {return 1};
        let Some(code) = status.code() else {return 1};
        if code != 0 || !self.remember {
            return code;
        };
        let Some(ExtantRefName {
            name: Ok(target), ..
        }) = ExtantRefName::resolve(&source.get_commit_spec()) else {return code};
        set_target(&current_branch, &target).expect("Could not set target branch.");
        code
    }
}

fn find_current_branch() -> Result<Option<LocalBranchName>, CommitErr> {
    match GitStatus::new().map_err(CommitErr::GitError) {
        Ok(GitStatus {
            head: WorktreeHead::Attached { head, .. },
            ..
        }) => Ok(Some(head)),
        Err(err) => Err(err),
        _ => Ok(None),
    }
}

#[derive(Debug, Args)]
/**
Display a diff predicting the changes that would be merged if you merged your working tree.

The diff includes uncommitted changes, unlike `git diff <target>...`.  It is produced by
diffing the working tree against the merge base of <target> and HEAD.
*/
pub struct MergeDiff {
    /// The branch you would merge into.  (Though any commitish will work.)
    #[arg(long, short)]
    target: Option<CommitSpec>,
    /// Use the meyers diff algorithm.  (Faster, can produce more confusing diffs.)
    #[arg(long)]
    myers: bool,
    /// Emit modified filenames only, not diffs.
    #[arg(long)]
    name_only: bool,
    path: Vec<String>,
    #[arg(long)]
    remember: bool,
}

impl MergeDiff {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        if Commit::from_str("HEAD").is_err() {
            return Err(MakeArgsErr::MergeDiffNoHead);
        }
        use FindTargetErr::*;
        let target = match self.target {
            Some(target) => target,
            None => match find_target() {
                Ok(spec) => {
                    let repo = match Repository::open_from_env().map_err(OpenRepoError::from) {
                        Ok(repo) => repo,
                        Err(err) => {
                            return Err(MakeArgsErr::MergeDiffOpenRepo(err));
                        }
                    };
                    eprintln!("Using remembered value {:?}", spec.find_shortest(&repo));
                    Ok(spec.into())
                }
                Err(NoCurrentBranch) => Err(MakeArgsErr::MergeDiffFindTarget(NoCurrentBranch)),
                Err(CommitErr(err)) => Err(MakeArgsErr::MergeDiffFindTarget(CommitErr(err))),
                Err(NoRemembered) => Err(MakeArgsErr::MergeDiffFindTarget(NoRemembered)),
            }?,
        };
        Diff {
            source: Some(target.find_merge_base(CommitSpec::from_str("HEAD").unwrap().as_ref())),
            target: None,
            myers: self.myers,
            name_only: self.name_only,
            path: self.path,
        }
        .make_args()
    }
}

impl Runnable for MergeDiff {
    fn run(self) -> i32 {
        if self.remember {
            let current_branch = get_current_branch().expect("Current branch");
            if let Some(target) = self.target.as_ref().and_then(|t| {
                ExtantRefName::resolve(&t.get_commit_spec()).and_then(|s| s.name.ok())
            }) {
                set_target(&current_branch, &target).expect("Could not set target branch.");
            }
        }
        let args = match self.make_args() {
            Ok(args) => args,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        let mut cmd = make_git_command(&args);
        let Ok(status) = cmd.status() else {return 1};
        status.code().unwrap_or(1)
    }
}

#[derive(Debug, Args)]
/// Transfer remote changes to the local repository and working tree
pub struct Pull {
    ///The Remote entry to pull from
    remote: Option<String>,
    ///The branch to pull from
    source: Option<String>,
}

impl ArgMaker for Pull {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let mut cmd_args = vec!["pull", "--ff-only"];
        cmd_args.extend(self.remote.iter().map(|s| s.as_str()));
        cmd_args.extend(self.source.iter().map(|s| s.as_str()));
        Ok(to_strings(&cmd_args))
    }
}

#[derive(Debug, Args)]
/// Restore the contents of a file to a previous value
pub struct Restore {
    /// Tree/commit/branch containing the version of the file to restore.
    #[arg(long, short)]
    source: Option<SomethingSpec>,
    /// File(s) to restore
    #[arg(required = true)]
    path: Vec<String>,
}

impl ArgMaker for Restore {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let source = self
            .source
            .ok_or(())
            .or_else(|_| SomethingSpec::from_str("HEAD"))
            .map_err(MakeArgsErr::Restore)?;

        let mut cmd_args = to_strings(&["checkout", &source.get_treeish_spec()]);
        if !self.path.is_empty() {
            cmd_args.push("--".to_string());
            cmd_args.extend(self.path);
        }
        Ok(cmd_args)
    }
}

#[derive(Debug, Args)]
/// Revert a previous commit.
pub struct Revert {
    /// The commit to revert
    source: CommitSpec,
}

impl ArgMaker for Revert {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let cmd_args = to_strings(&["revert", "-m1", &self.source.get_commit_spec()]);
        Ok(cmd_args)
    }
}

#[enum_dispatch]
#[derive(Debug, Subcommand)]
pub enum RewriteCommand {
    Cat,
    Show,
    Diff,
    Log,
    Pull,
    PushTags,
    Restore,
    Revert,
}

#[enum_dispatch]
#[derive(Debug, Parser)]
pub enum NativeCommand {
    DisconnectBranch,
    #[command(flatten)]
    RewriteCommand(RewriteCommand),
    Commit(CommitCmd),
    IgnoreChanges,
    Push,
    Switch,
    SwitchNext,
    SwitchPrev,
    FakeMerge,
    Merge,
    MergeDiff,
    NextBranch,
    Pipeline,
    SquashCommit,
    Checkout,
    Status,
    #[command()]
    Ignore,
    Revno,
}
#[derive(Debug, Args)]
/// Record the current contents of the working tree.
pub struct CommitCmd {
    #[arg(long, short)]
    message: Option<String>,
    /// Amend the HEAD commit.
    #[arg(long)]
    amend: bool,
    #[arg(long, short)]
    no_verify: bool,
    ///Commit only changes in the index.
    #[arg(long)]
    no_all: bool,
    #[arg(long)]
    no_strict: bool,
}

impl ArgMaker for CommitCmd {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let mut cmd_args = vec!["commit"];
        if !self.no_all {
            cmd_args.push("--all")
        }
        if let Some(message) = &self.message {
            cmd_args.extend(["--message", message]);
        }
        if self.amend {
            cmd_args.push("--amend");
        }
        if self.no_verify {
            cmd_args.push("--no-verify");
        }
        Ok(to_strings(&cmd_args))
    }
}

pub trait Runnable {
    fn run(self) -> i32;
}

#[enum_dispatch(NativeCommand)]
pub trait RunExit {
    fn run_exit(self) -> !;
}

impl<T: Runnable> RunExit for T {
    fn run_exit(self) -> ! {
        exit(self.run());
    }
}

impl RunExit for Vec<String> {
    fn run_exit(self) -> ! {
        make_git_command(&self).exec();
        exit(1);
    }
}

impl RunExit for RewriteCommand {
    fn run_exit(self) -> ! {
        match self.make_args() {
            Ok(args_vec) => {
                args_vec.run_exit();
            }
            Err(err) => {
                eprintln!("{}", err);
                exit(1);
            }
        };
    }
}

impl Runnable for CommitCmd {
    fn run(self) -> i32 {
        if !self.no_strict {
            let status = match GitStatus::new() {
                Ok(status) => status,
                Err(err) => {
                    eprintln!("{}", err);
                    return 1;
                }
            };
            let untracked = status.untracked_filenames();
            if !untracked.is_empty() {
                eprintln!("Untracked files are present:");
                for entry in untracked {
                    eprintln!("{}", entry);
                }
                eprintln!("You can add them with \"oaf add\", ignore them with \"oaf ignore\", or use --no-strict.");
                return 1;
            }
        }
        let args = match self.make_args() {
            Ok(args) => args,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        make_git_command(&args).exec();
        0
    }
}

#[derive(Debug, Args)]
/**
Transfer local changes to a remote repository and branch.

If upstream is unset, the equivalent location on "origin" will be used.
*/
pub struct Push {
    #[arg(long, short)]
    /// Allow changing history on the remote branch
    force: bool,
    repository: Option<String>,
}

impl Runnable for Push {
    fn run(self) -> i32 {
        let branch = match get_current_branch() {
            Ok(branch) => branch,
            Err(unhandled) => {
                eprintln!("Unhandled: {}", unhandled.name);
                return 1;
            }
        };
        let mut args = vec!["push"];
        args.extend(if setting_exists(&branch.setting_name("remote")) {
            if !setting_exists(&branch.setting_name("merge")) {
                panic!("Branch in unsupported state");
            }
            self.repository.iter().map(|s| s.as_str()).collect()
        } else {
            match Commit::from_str("HEAD") {
                Ok(_) => {
                    let repo = self.repository.as_deref().unwrap_or("origin");
                    vec!["-u", repo, "HEAD"]
                }
                Err(CommitErr::NoCommit { .. }) => {
                    eprintln!("Cannot push: no commits in HEAD.");
                    return 1;
                }
                Err(CommitErr::GitError(err)) => {
                    eprintln!("{}", err);
                    return 1;
                }
            }
        });
        if self.force {
            args.push("--force");
        }
        make_git_command(&args).exec();
        0
    }
}

#[derive(Debug, Args)]
/// Push all tags to the remote repository.
pub struct PushTags {
    /// The repository to push to (optional)
    repository: Option<String>,
}

impl ArgMaker for PushTags {
    fn make_args(self) -> Result<Vec<String>, MakeArgsErr> {
        let mut args = to_strings(&["push", "--tags"]);
        args.extend(self.repository.into_iter());
        Ok(args)
    }
}

#[derive(Debug, Args)]
/**
Switch to a branch, stashing and restoring pending changes.

Outstanding changes are stored under "refs/branch-wip/", in the repo, with the branch's name
as the suffix.  For example, pending changes for a branch named "foo" would
be stored in a ref named "refs/branch-wip/foo".
*/
pub struct Switch {
    /// The branch to switch to.
    branch: String,
    /// Create the branch and switch to it
    #[arg(long, short)]
    create: bool,
    /// Switch without stashing/unstashing changes.
    #[arg(long, short)]
    keep: bool,
}

impl Runnable for Switch {
    fn run(self) -> i32 {
        // Actually a RefName, not a local branch (even if that refname refers to a local branch)
        let switch_type = if self.create {
            // For creation, any value is a branch name
            SwitchType::Create(LocalBranchName::from(self.branch.clone()))
        } else {
            let target = BranchyName::UnresolvedName(self.branch.clone());
            if self.keep {
                SwitchType::PlainSwitch(target)
            } else {
                SwitchType::WithStash(target)
            }
        };
        match stash_switch(switch_type) {
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
            Err(SwitchErr::InvalidBranchName(invalid_branch)) => {
                eprintln!(
                    "'{}' is not a valid branch name",
                    invalid_branch.branch_name()
                );
                1
            }
            Err(SwitchErr::GitError(err)) => {
                eprintln!("{}", err);
                1
            }
            Err(SwitchErr::OpenRepoError(err)) => {
                eprintln!("{}", err);
                1
            }
            Err(SwitchErr::LinkFailure(err)) => {
                eprintln!("{}", err);
                1
            }
        }
    }
}

fn handle_switch(switch_type: SwitchType) -> i32 {
    use SwitchType::*;
    let target = match switch_type.clone() {
        Create(target) | CreateNext(target) => target.branch_name().to_owned(),
        PlainSwitch(target) | WithStash(target) => target.get_as_branch().to_string(),
    };
    match stash_switch(switch_type) {
        Ok(()) => 0,
        Err(SwitchErr::BranchInUse { path }) => {
            println!("Branch {} is already in use at {}", target, path);
            1
        }
        Err(SwitchErr::AlreadyExists) => {
            eprintln!("Branch {} already exists", target);
            1
        }
        Err(SwitchErr::NotFound) => {
            eprintln!("Branch {} not found", target);
            1
        }
        Err(SwitchErr::InvalidBranchName(invalid_branch)) => {
            eprintln!(
                "'{}' is not a valid branch name",
                invalid_branch.branch_name()
            );
            1
        }
        Err(SwitchErr::GitError(err)) => {
            eprintln!("{}", err);
            1
        }
        Err(SwitchErr::OpenRepoError(err)) => {
            eprintln!("{}", err);
            1
        }
        Err(SwitchErr::LinkFailure(err)) => {
            eprintln!("{}", err);
            1
        }
    }
}

/// Switch to the next branch a sequence (or create the next branch).
#[derive(Debug, Args)]
#[clap(group(ArgGroup::new("creation").args(&["create", "next_num"])))]
pub struct SwitchNext {
    /// Switch without stashing/unstashing changes.
    #[arg(long, short)]
    keep: bool,
    /// Create and switch to a named next branch.
    #[arg(long, short)]
    create: Option<String>,
    /// Create and switch to a next branch named after the current branch, with an incremented
    /// number.
    #[arg(long, short)]
    next_num: bool,
}

impl SwitchNext {
    pub fn new(keep: bool, create: Option<impl Into<String>>, next_num: bool) -> SwitchNext {
        SwitchNext {
            keep,
            create: create.map(|v| v.into()),
            next_num,
        }
    }
}

fn get_local_current(repo: &Repository) -> Result<LocalBranchName, String> {
    let head_ref = repo
        .find_reference("HEAD")
        .map_err(|e| e.message().to_owned())?;
    let Some(head_target) = head_ref.symbolic_target() else {
        return Err("HEAD is detached".to_owned());
    };
    let Ok(BranchName::Local(branch)) = BranchName::from_str(head_target) else {
        return Err("HEAD is not a local branch".to_owned());
    };
    Ok(branch)
}

fn switch_sibling<T: SiblingBranch>(keep: bool) -> i32
where
    T::BranchError: Display,
{
    let repo = match Repository::open_from_env().map_err(OpenRepoError::from) {
        Ok(repo) => repo,
        Err(err) => {
            eprintln!("{}", err);
            return 1;
        }
    };
    let sibling_ref = match get_local_current(&repo).map(T::from) {
        Err(err) => {
            eprintln!("{}", err);
            return 1;
        }
        Ok(sibling_ref) => sibling_ref,
    };
    let target = match resolve_symbolic_reference(&repo, &sibling_ref).map_err(T::BranchError::from)
    {
        Ok(target) => target,
        Err(err) => {
            eprintln!("{}", err);
            return 1;
        }
    };
    let target = match BranchyName::from(target)
        .resolve(&repo)
        .map_err(T::BranchError::from)
    {
        Ok(target) => target,
        Err(err) => {
            eprintln!("{}", err);
            return 1;
        }
    };
    handle_switch(if keep {
        SwitchType::PlainSwitch(target)
    } else {
        SwitchType::WithStash(target)
    })
}

impl Runnable for SwitchNext {
    fn run(self) -> i32 {
        let create_name = match (self.create, self.next_num) {
            (Some(create), false) => Some(LocalBranchName::from(create)),
            (Some(_), true) => {
                // Parser is supposed to prevent this case.
                panic!("Cannot specify both --create and --next-num");
            }
            (None, true) => {
                let current = get_current_branch().expect("No current branch.");
                let next_str = current.branch_name().to_owned();
                Some(LocalBranchName::from(PipeNext::make_name(next_str)))
            }
            (None, false) => None,
        };
        let Some(create) = create_name else {
            return switch_sibling::<PipeNext>(self.keep)
        };
        handle_switch(SwitchType::CreateNext(create))
    }
}

/// Switch to the previous branch in a sequence.
#[derive(Debug, Args)]
pub struct SwitchPrev {
    /// Switch without stashing/unstashing changes.
    #[arg(long, short)]
    keep: bool,
}

impl Runnable for SwitchPrev {
    fn run(self) -> i32 {
        switch_sibling::<PipePrev>(self.keep)
    }
}

#[derive(Debug, Args)]
/**
Remove a branch from its sequence.

If the branch was in the middle of a sequence, the next and previous branches will be connected to
each other.
*/
pub struct DisconnectBranch {
    /// The name of the branch to disconnect.
    name: String,
}

impl Runnable for DisconnectBranch {
    fn run(self) -> i32 {
        let repo = Repository::open_from_env().expect("Repository not found");
        let Ok(_) = unlink_branch(&repo, &LocalBranchName::from(self.name)) else {
            println!("No such branch.");
            return 1;
        };
        0
    }
}

#[derive(Debug, Args)]
/**
View and / or set the next branch.

See also "pipeline".
*/
pub struct NextBranch {
    /// The branch to set as the next branch
    next: Option<String>,
}

impl Runnable for NextBranch {
    fn run(self) -> i32 {
        let repo = match Repository::open_from_env().map_err(OpenRepoError::from) {
            Ok(repo) => repo,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        let current = match get_local_current(&repo) {
            Err(err) => {
                println!("{}", err);
                return 1;
            }
            Ok(current) => current,
        };
        let Some(next_name) = self.next else {
            match resolve_symbolic_reference(&repo, &PipeNext::from(current)) {
                Ok(next) => {
                    println!("{}", next.find_shortest(&repo));
                    return 0;
                }
                Err(RefErr::NotFound(_)) => {
                    eprintln!("No next branch");
                    return 0;
                }
                Err(err) => {
                    eprintln!("{}", NextRefErr(err));
                    return 1;
                }

            }
        };
        let next = match repo
            .resolve_reference_from_short_name(&next_name)
            .map_err(RefErr::from)
        {
            Ok(next) => next,
            Err(RefErr::NotFound(_)) => {
                eprintln!("{} does not exist", next_name);
                return 1;
            }
            Err(RefErr::Other(err)) => {
                eprintln!("{}", err);
                return 1;
            }
            Err(err) => {
                println!("{}", NextRefErr(err));
                return 1;
            }
        };
        let next_branch = match LocalBranchName::try_from(&next) {
            Ok(next_branch) => next_branch,
            Err(BranchValidationError::NotLocalBranch(_)) => {
                eprintln!("Not a local branch: {}", next_name);
                return 1;
            }
            Err(BranchValidationError::NotUtf8(_)) => {
                eprintln!("Not a utf8 string: {}", next_name);
                return 1;
            }
        };
        if let Err(err) =
            check_link_branches(&repo, current.into(), next_branch.into()).map(|x| x.link(&repo))
        {
            eprintln!("{}", err);
            return 1;
        }
        0
    }
}

/// List a branch sequence
#[derive(Debug, Args)]
pub struct Pipeline {}

impl Runnable for Pipeline {
    fn run(self) -> i32 {
        let repo = match Repository::open_from_env().map_err(OpenRepoError::from) {
            Ok(repo) => repo,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        let current_lb = match get_local_current(&repo) {
            Err(err) => {
                println!("{}", err);
                return 1;
            }
            Ok(current) => current,
        };
        let mut previous = vec![];
        let mut loop_lb = advance::<PipePrev>(&repo, current_lb.clone());
        loop {
            let tmp = match loop_lb {
                Err(_) => {
                    eprintln!("Error!");
                    return 1;
                }
                Ok(Some(current)) => current,
                Ok(None) => break,
            };
            previous.push(tmp.branch_name().to_owned());
            loop_lb = advance::<PipePrev>(&repo, tmp);
        }
        previous.reverse();
        for branch in previous {
            println!("  {}", branch);
        }
        println!("* {}", current_lb.branch_name());
        let mut loop_lb = advance::<PipeNext>(&repo, current_lb);
        loop {
            let tmp = match loop_lb {
                Err(_) => {
                    eprintln!("Error!");
                    return 1;
                }
                Ok(Some(current)) => current,
                Ok(None) => break,
            };
            println!("  {}", tmp.branch_name());
            loop_lb = advance::<PipeNext>(&repo, tmp);
        }
        0
    }
}

fn advance<T: SiblingBranch + From<LocalBranchName> + ReferenceSpec>(
    repo: &Repository,
    current_lb: LocalBranchName,
) -> Result<Option<LocalBranchName>, RefErr> {
    let ref_string = match resolve_symbolic_reference(repo, &T::from(current_lb)) {
        Ok(next) => next.name,
        Err(RefErr::NotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };
    match BranchName::from_str(&ref_string) {
        Ok(BranchName::Local(local)) => Ok(Some(local)),
        _ => Err(RefErr::NotBranch),
    }
}

#[derive(Debug, Args)]
/**
Perform a fake merge of the specified branch/commit, leaving the local tree unmodified.

This effectively gives the contents of the latest commit precedence over the contents of the
source commit.
*/
pub struct FakeMerge {
    /// The source for the fake merge.
    source: CommitSpec,
    /// The message to use for the fake merge.  (Default: "Fake merge.")
    #[arg(long, short)]
    message: Option<String>,
}

impl Runnable for FakeMerge {
    fn run(self) -> i32 {
        let head = match head_for_squash() {
            Ok(head) => head,
            Err(exit_status) => return exit_status,
        };
        let message = &self.message.unwrap_or_else(|| "Fake merge.".to_string());
        let fm_commit = head
            .commit(&head, Some(self.source), message)
            .expect("Could not generate commit.");
        fm_commit.set_wt_head();
        0
    }
}

#[derive(Debug, Args)]
/// Convert all commits from a branch-point into a single commit.
///
/// The last-committed state is turned into a new commit.  The branch-point
/// or latest merge is used as the parent of the new commit.  By default,
/// the remembered merge branch is used to find the parent, but this can be
/// overridden.
pub struct SquashCommit {
    /// The item we want to squash relative to.
    #[arg(long, short)]
    branch_point: Option<CommitSpec>,
    /// The message to use for the squash commit.  (Default: "Squash commit.")
    #[arg(long, short)]
    message: Option<String>,
}

fn head_for_squash() -> Result<Commit, i32> {
    let Ok(head) = Commit::from_str("HEAD") else {
        eprintln!("Cannot squash commit: no commits in HEAD.");
        return Err(1)
    };
    Ok(head)
}

impl Runnable for SquashCommit {
    fn run(self) -> i32 {
        let head = match head_for_squash() {
            Ok(head) => head,
            Err(exit_status) => return exit_status,
        };
        let repo = match Repository::open_from_env().map_err(OpenRepoError::from) {
            Ok(repo) => repo,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        let branch_point = match ensure_source(&repo, self.branch_point) {
            Ok(branch_point) => branch_point,
            Err(exit_status) => {
                return exit_status;
            }
        };
        let parent = head.find_merge_base(branch_point.as_ref());
        let message = &self.message.unwrap_or_else(|| "Squash commit".to_owned());
        let fm_commit = head
            .commit(&parent, None, message)
            .expect("Could not generate commit.");
        fm_commit.set_wt_head();
        eprintln!("Commit squashed.  To undo: oaf reset {}", head.sha);
        0
    }
}
#[derive(Debug, Args)]
/// Disabled to prevent accidentally discarding stashed changes.
pub struct Checkout {
    /// The branch to switch to.
    _branch_name: String,
    #[arg(long, short)]
    _branch: bool,
}

impl Runnable for Checkout {
    fn run(self) -> i32 {
        eprintln!(
            "Please use \"switch\" to change branches or \"restore\" to restore files to a known state"
        );
        1
    }
}

#[derive(Debug, Args)]
/// Show the status of changed and unknown files in the working tree.
pub struct Status {}

impl Runnable for Status {
    fn run(self) -> i32 {
        let gs = match GitStatus::new() {
            Ok(status) => status,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        match &gs.head {
            WorktreeHead::Attached { head, upstream, .. } => {
                println!("On branch {}", head.branch_name());
                if let Some(upstream) = upstream {
                    let msg = match (upstream.added, upstream.removed) {
                        (0, 0) => format!("Your branch is up to date with '{}'.", upstream.name),
                        (0, removed) => format!(
                            "Your branch is behind '{}' by {} commit(s), and can be \
                            fast-forwarded.",
                            upstream.name, removed
                        ),
                        (added, 0) => format!(
                            "Your branch is ahead of '{}' by {} commit(s).",
                            upstream.name, added
                        ),
                        (added, removed) => format!(
                            "Your branch and '{}' have diverged,\n\
                        and have {} and {} different commits each, respectively.\n  \
                        (use \"oaf merge {}\" to merge the remote branch into yours)",
                            upstream.name, added, removed, upstream.name
                        ),
                    };
                    println!("{}", msg);
                }
            }
            WorktreeHead::Detached(_) => {}
        }
        let mut gs_iter = gs.iter();
        let cwd = env::current_dir().expect("Need cwd");
        let top = match get_toplevel() {
            Ok(top) => top,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        };
        let top_rel = cwd.strip_prefix(top).unwrap();
        for se in gs_iter.fix_removals() {
            let out = se.format_entry(&top_rel);
            println!("{}", out);
        }
        1
    }
}

#[derive(Debug, Args)]
/**
Tell git to ignore a file (that has not been added).

This updates the top-level .gitignore, not any lower ones.

To ignore changes to files that have been added, see "ignore-changes".
*/
pub struct Ignore {
    /// Ignores the file in the local repository, instead of the worktree .gitignore.
    #[arg(long)]
    local: bool,
    /// Arguments should apply recursively.
    #[arg(long, short)]
    recurse: bool,
    /// The list of files to ignore
    files: Vec<String>,
}

/// Best-effort canonicalization.
///
/// Canonicalizes the portions of the path that exist, ignores the rest.
/// Does not traverse terminal symlinks.
fn normpath(path: &Path) -> io::Result<PathBuf> {
    let mut abspath = std::env::current_dir().unwrap();
    abspath.push(path);
    for ancestor in abspath.ancestors().skip(1) {
        if let Ok(canonical) = ancestor.canonicalize() {
            return Ok(canonical.join(abspath.strip_prefix(ancestor).unwrap()));
        }
    }
    Ok(abspath)
}

enum IgnoreEntry {
    RecursiveEntry(PathBuf),
    SpecificEntry(PathBuf),
}

impl IgnoreEntry {
    fn make_string(&self) -> String {
        match self {
            IgnoreEntry::RecursiveEntry(path) => path.to_str().unwrap().to_owned(),
            IgnoreEntry::SpecificEntry(path) => {
                let mut result = path.to_str().unwrap().to_owned();
                if !result.contains('/') {
                    result.insert(0, '/');
                }
                result
            }
        }
    }
}

fn add_ignores(entries: Vec<IgnoreEntry>, ignore_file: &Path) {
    let ignores = match fs::read_to_string(ignore_file) {
        Ok(ignores) => ignores,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => panic!("{}", e),
    };
    fs::write(
        ignore_file,
        append_lines(ignores, entries.into_iter().map(|e| e.make_string())),
    )
    .expect("Can't write .gitignore");
}

impl Ignore {
    fn make_specific_entry(top: &Path, file: &str) -> IgnoreEntry {
        let path = normpath(&PathBuf::from(file)).unwrap();
        IgnoreEntry::SpecificEntry(relative_path(top, path).unwrap())
    }
}

impl Runnable for Ignore {
    fn run(self) -> i32 {
        let top = PathBuf::from(match get_toplevel() {
            Ok(top) => top,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        });
        let top = top.canonicalize().unwrap();
        let mut entries = vec![];
        for line in &self.files {
            if self.recurse {
                if line.contains('/') {
                    eprintln!(
                        "Warning: \"{}\" will not be recursive because it contains a slash.",
                        line
                    );
                }
                entries.push(IgnoreEntry::RecursiveEntry(PathBuf::from(line)))
            } else {
                entries.push(Self::make_specific_entry(&top, line));
            }
        }
        let ignore_file = if self.local {
            get_git_path("info/exclude")
        } else {
            top.join(".gitignore")
        };
        add_ignores(entries, &ignore_file);
        if !self.local {
            let mut cmd =
                make_git_command(&[&OsString::from("add"), &ignore_file.as_os_str().to_owned()]);
            let Ok(status) = cmd.status() else {return 1};
            status.code().unwrap_or(1)
        } else {
            0
        }
    }
}

#[derive(Debug, Args)]
/// Ignore changes to a file.
///
/// While active, changes to a file are ignored by "status" and "commit", even if you "add" the
/// file.  May be disabled by --unset.
///
/// If no files are supplied, list ignored files.
///
/// To ignore files that have not been added, see `ignore`.
pub struct IgnoreChanges {
    files: Vec<String>,
    #[arg(long)]
    /// Stop ignoring (possible) changes to listed files
    unset: bool,
}

impl Runnable for IgnoreChanges {
    fn run(self) -> i32 {
        if !self.files.is_empty() {
            let action = if self.unset {
                "--no-assume-unchanged"
            } else {
                "--assume-unchanged"
            };
            let mut args = vec!["update-index", action];
            args.extend(self.files.iter().map(|s| s.as_str()));
            make_git_command(&args).exec();
        } else {
            let output = run_git_command(&["ls-files", "-v"]).expect("Can't list files.");
            let mut matched = false;
            for line in output_to_string(&output).lines() {
                if let Some(ignored_file) = line.strip_prefix("h ") {
                    matched = true;
                    println!("{}", ignored_file);
                }
            }
            if !matched {
                eprintln!("No files have ignore-changes set.");
            }
        }
        0
    }
}

trait RunOrError {
    type Error;
    fn run(self) -> Result<i32, Self::Error>;
}

impl<T: Display, U: RunOrError<Error = T>> Runnable for U {
    fn run(self) -> i32 {
        match U::run(self) {
            Err(err) => {
                eprintln!("{}", err);
                1
            }
            Ok(status) => status,
        }
    }
}

#[derive(Debug, Args)]
pub struct Revno {
    commit: Option<CommitSpec>,
}

impl RunOrError for Revno {
    type Error = CommitErr;
    fn run(self) -> Result<i32, Self::Error> {
        let repo = Repository::open_from_env()?;
        let commit_spec = match self.commit {
            Some(spec) => spec,
            None => CommitSpec::from_str("HEAD")?,
        };
        println!("{}", calc_revno(&repo, commit_spec.as_ref())?);
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_to_string() {
        assert_eq!(
            "foo/bar",
            IgnoreEntry::SpecificEntry(PathBuf::from("foo/bar")).make_string()
        );
        assert_eq!(
            "/foo",
            IgnoreEntry::SpecificEntry(PathBuf::from("foo")).make_string()
        );
        assert_eq!(
            "foo",
            IgnoreEntry::RecursiveEntry(PathBuf::from("foo")).make_string()
        );
    }
}

/**
 * Revnos are *sort-of* 1-indexed.  0 is reserved for the "parent" of the first commit in contexts
 * where that makes sense (e.g. diff).
 */
fn calc_revno(repo: &Repository, oid: &Commit) -> Result<i32, git2::Error> {
    let mut walker = repo.revwalk()?;
    walker.push(oid.sha.parse::<git2::Oid>()?)?;
    walker.simplify_first_parent()?;
    Ok((walker.count() + 1).try_into().unwrap())
}
