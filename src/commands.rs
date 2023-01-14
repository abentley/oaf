// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::branch::{PipeNext, PipePrev, SiblingBranch};
use super::git::{
    get_current_branch, get_git_path, get_settings, get_toplevel, make_git_command,
    output_to_string, run_git_command, setting_exists, BranchName, LocalBranchName, ReferenceSpec,
    SettingEntry, UnparsedReference,
};
use super::worktree::{
    append_lines, base_tree, relative_path, set_target, stash_switch, target_branch_setting,
    Commit, CommitErr, CommitSpec, Commitish, ExtantRefName, GitStatus, SomethingSpec, SwitchErr,
    SwitchType, Tree, Treeish, WorktreeHead,
};
use clap::{Args, Parser, Subcommand};
use enum_dispatch::enum_dispatch;
use git2::Repository;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;

fn to_strings(cmd_args: &[&str]) -> Vec<String> {
    cmd_args.iter().map(|s| s.to_string()).collect()
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
    fn make_args(self) -> Result<Vec<String>, ()>;
}

impl ArgMaker for Cat {
    fn make_args(self) -> Result<Vec<String>, ()> {
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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
            None => match base_tree() {
                Ok(tree) => tree.get_tree_reference().into(),
                Err(err) => {
                    eprintln!("{}", err);
                    return Err(());
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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

enum FindTargetErr {
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
fn ensure_source(source: Option<CommitSpec>) -> Result<CommitSpec, i32> {
    if let Some(source) = source {
        return Ok(source);
    }
    use FindTargetErr::*;
    match find_target() {
        Ok(spec) => {
            eprintln!("Using remembered value {:?}", spec.short());
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
        let Ok(source) = ensure_source(self.source) else {
            return 1;
        };
        let args = ["merge", "--no-commit", "--no-ff", &source.spec];
        let mut cmd = make_git_command(&args);
        let Ok(status) = cmd.status() else {return 1};
        let Some(code) = status.code() else {return 1};
        {
            if code == 0 && self.remember {
                if let Some(ExtantRefName {
                    name: Ok(target), ..
                }) = ExtantRefName::resolve(&source.get_commit_spec())
                {
                    set_target(&current_branch, &target).expect("Could not set target branch.");
                }
            }
            code
        }
    }
}

fn find_current_branch() -> Result<Option<LocalBranchName>, CommitErr> {
    match GitStatus::new() {
        Ok(GitStatus {
            head: WorktreeHead::Attached { head, .. },
            ..
        }) => Ok(Some(head)),
        Err(err) => Err(CommitErr::GitError(err)),
        _ => Ok(None),
    }
}

fn find_target_branchname(
    branch_name: LocalBranchName,
) -> Result<Option<BranchName>, UnparsedReference> {
    let prefix = branch_name.setting_name("");
    let target_setting = target_branch_setting(&branch_name);
    let remote_setting = branch_name.setting_name("remote");
    let mut remote = None;
    let mut target_branch = None;
    for entry in get_settings(prefix, &["oaf-target-branch", "remote"]) {
        if let SettingEntry::Valid { key, value } = entry {
            if key == target_setting {
                target_branch = Some(value);
            } else if key == remote_setting {
                remote = Some(value);
            }
        }
    }
    let Some(target_branch) = target_branch else {
        return Ok(None);
    };
    let target_branch = { target_from_settings(target_branch, remote)? };
    Ok(Some(target_branch))
}

fn target_from_settings(
    target_branch: String,
    remote: Option<String>,
) -> Result<BranchName, UnparsedReference> {
    let refname = ExtantRefName::resolve(&target_branch).unwrap();
    match (remote, refname.name) {
        (Some(remote), Ok(BranchName::Local(local_branch))) => {
            Ok(BranchName::Remote(local_branch.with_remote(remote)))
        }
        (_, refname) => refname,
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
    fn make_args(self) -> Result<Vec<String>, ()> {
        if let Err(..) = Commit::from_str("HEAD") {
            eprintln!("Cannot merge-diff: no commits in HEAD.");
            return Err(());
        }
        use FindTargetErr::*;
        let target = match self.target {
            Some(target) => target,
            None => match find_target() {
                Ok(spec) => {
                    eprintln!("Using remembered value {:?}", spec.short());
                    Ok(spec.into())
                }
                Err(NoCurrentBranch) => {
                    eprintln!("No current branch.");
                    Err(())
                }
                Err(CommitErr(err)) => {
                    eprintln!("{}", err);
                    Err(())
                }
                Err(NoRemembered) => {
                    eprintln!("Target not supplied and no remembered target.");
                    Err(())
                }
            }?,
        };
        Diff {
            source: Some(target.find_merge_base(&CommitSpec::from_str("HEAD").unwrap())),
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
            if let Some(target) = &self.target {
                if let Some(ExtantRefName {
                    name: Ok(target), ..
                }) = ExtantRefName::resolve(&target.get_commit_spec())
                {
                    set_target(&current_branch, &target).expect("Could not set target branch.");
                }
            }
        }
        let Ok(args) = self.make_args() else { return 1 };
        let mut cmd = make_git_command(&args);
        let Ok(status) = cmd.status() else {return 1};
        let Some(code) = status.code() else {return 1};
        code
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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
    fn make_args(self) -> Result<Vec<String>, ()> {
        let source = if let Some(source) = self.source {
            source
        } else {
            match SomethingSpec::from_str("HEAD") {
                Ok(source) => source,
                Err(CommitErr::NoCommit { .. }) => {
                    eprintln!("Cannot restore: no commits in HEAD.");
                    return Err(());
                }
                Err(CommitErr::GitError(err)) => {
                    eprintln!("{}", err);
                    return Err(());
                }
            }
        };
        let source = source.get_treeish_spec();
        let mut cmd_args = to_strings(&["checkout", &source]);
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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
    SquashCommit,
    Checkout,
    Status,
    #[command()]
    Ignore,
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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
        let Ok(args_vec) = self.make_args() else {
            exit(1);
        };
        args_vec.run_exit();
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
        let Ok(args) = self.make_args() else { return 1 };
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
            if let Err(err) = Commit::from_str("HEAD") {
                match err {
                    CommitErr::NoCommit { .. } => {
                        eprintln!("Cannot push: no commits in HEAD.");
                        return 1;
                    }
                    CommitErr::GitError(err) => {
                        eprintln!("{}", err);
                        return 1;
                    }
                }
            };
            let repo = self.repository.as_deref().unwrap_or("origin");
            vec!["-u", repo, "HEAD"]
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
    fn make_args(self) -> Result<Vec<String>, ()> {
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
        let switch_type = if self.create {
            SwitchType::Create
        } else if self.keep {
            SwitchType::PlainSwitch
        } else {
            SwitchType::WithStash
        };
        match stash_switch(&self.branch, switch_type) {
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
                eprintln!("'{}' is not a valid branch name", invalid_branch.short());
                1
            }
            Err(SwitchErr::GitError(err)) => {
                eprintln!("{}", err);
                1
            }
        }
    }
}

fn handle_switch(target: &str, switch_type: SwitchType) -> i32 {
    match stash_switch(target, switch_type) {
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
            eprintln!("'{}' is not a valid branch name", invalid_branch.short());
            1
        }
        Err(SwitchErr::GitError(err)) => {
            eprintln!("{}", err);
            1
        }
    }
}

#[derive(Debug, Args)]
pub struct SwitchNext {
    /// Switch without stashing/unstashing changes.
    #[arg(long, short)]
    keep: bool,
}

fn switch_sibling<T: SiblingBranch + From<LocalBranchName>>(keep: bool) -> i32 {
    let switch_type = if keep {
        SwitchType::PlainSwitch
    } else {
        SwitchType::WithStash
    };
    let current = get_current_branch().expect("current branch");
    let next_ref = T::from(current);
    let repo = match Repository::open_from_env() {
        Ok(repo) => repo,
        Err(err) => {
            eprintln!("Oops!, {}", err);
            return 1;
        }
    };
    let target = match next_ref.resolve_symbolic(&repo) {
        Ok(target) => target,
        Err(err) => {
            eprintln!("{}", err);
            return 1;
        }
    };
    handle_switch(&target, switch_type)
}

impl Runnable for SwitchNext {
    fn run(self) -> i32 {
        switch_sibling::<PipeNext>(self.keep)
    }
}

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
pub struct NextBranch {
    next: Option<String>,
}

impl Runnable for NextBranch {
    fn run(self) -> i32 {
        let current = get_current_branch().expect("current branch");
        let next_ref = PipeNext::from(current);
        let Some(next_name) = self.next else  {
            println!("{}", next_ref.get_symbolic_short().unwrap());
            return 0;
        };
        let Some(
            ExtantRefName{
                name: Ok(BranchName::Local(next)),
                ..
            }) = ExtantRefName::resolve(&next_name) else {
            println!("{} is not a local branch.", next_name);
            return 1;
        };
        next_ref.set_symbolic(&next).unwrap();
        let prev_ref = PipePrev::from(next);
        prev_ref.set_symbolic(&next_ref.name).unwrap();
        0
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
            .commit(&head, Some(&self.source), message)
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
        let branch_point = match ensure_source(self.branch_point) {
            Ok(branch_point) => branch_point,
            Err(exit_status) => {
                return exit_status;
            }
        };
        let parent = head.find_merge_base(&branch_point);
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
                println!("On branch {}", head.name);
                if let Some(upstream) = upstream {
                    println!(
                        "{}",
                        match (upstream.added, upstream.removed) {
                            (0, 0) =>
                                format!("Your branch is up to date with '{}'.", upstream.name),
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
                        }
                    );
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
            {
                let Some(code) = status.code() else {return 1};
                code
            }
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
                if let Some(("", ignored_file)) = line.split_once("h ") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_branch_setting() {
        assert_eq!(
            target_branch_setting(&LocalBranchName {
                name: "my-branch".to_string()
            }),
            "branch.my-branch.oaf-target-branch"
        );
    }
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
