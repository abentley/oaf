// Copyright 2021 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.use enum_dispatch::enum_dispatch;
use enum_dispatch::enum_dispatch;
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command, Output};
use std::str::{from_utf8, FromStr};
use structopt::{clap, StructOpt};

struct GitStatus {
    outstr: String,
}

struct StatusIter<'a> {
    raw_entries: std::str::SplitTerminator<'a, char>,
}

#[derive(Debug, Clone, Copy)]
enum EntryLocationStatus {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    UpdatedButUnmerged,
}

impl FromStr for EntryLocationStatus {
    type Err = ();
    fn from_str(code: &str) -> std::result::Result<Self, <Self as FromStr>::Err> {
        Ok(match code {
            "." => EntryLocationStatus::Unmodified,
            "M" => EntryLocationStatus::Modified,
            "A" => EntryLocationStatus::Added,
            "D" => EntryLocationStatus::Deleted,
            "R" => EntryLocationStatus::Renamed,
            "C" => EntryLocationStatus::Copied,
            "U" => EntryLocationStatus::UpdatedButUnmerged,
            _ => {
                return Err(());
            }
        })
    }
}

#[derive(Debug, Copy, Clone)]
enum EntryState<'a> {
    Untracked,
    Ignored,
    Changed {
        staged_status: EntryLocationStatus,
        tree_status: EntryLocationStatus,
    },
    Renamed {
        staged_status: EntryLocationStatus,
        tree_status: EntryLocationStatus,
        old_filename: &'a str,
    },
}

#[derive(Debug, Copy, Clone)]
struct StatusEntry<'a> {
    state: EntryState<'a>,
    filename: &'a str,
}

impl GitStatus {
    fn iter(&self) -> StatusIter {
        StatusIter {
            // Note: there is an extra entry for each rename entry, consisting of the original
            // filename.  This is an inevitable consequence of splitting using a terminator instead
            // of performing the entry iteration in StatusIter::next()
            raw_entries: self.outstr.split_terminator('\x00'),
        }
    }

    fn new() -> GitStatus {
        let output =
            run_git_command(&["status", "--porcelain=v2", "-z"]).expect("Couldn't list directory");
        let outstr = output_to_string(&output);
        GitStatus { outstr }
    }
}

impl StatusIter<'_> {
    /**
     * Convert a "D." to "DD" if the file was deleted as well as being removed.  If the file was
     * not deleted, skip its ?? entry.
     **/
    fn fix_removals(&mut self) -> Vec<StatusEntry> {
        let mut entries = HashMap::new();
        let mut untracked = HashMap::new();
        for se in self {
            let kind_map = match se.state {
                EntryState::Untracked => &mut untracked,
                EntryState::Ignored => &mut untracked,
                _ => &mut entries,
            };
            kind_map.insert(se.filename, se);
        }
        let keys = entries
            .keys()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();
        for filename in keys {
            // If we remove an item with this filename from untracked, the entry in entries must be
            // D. already, so it does not need to be changed.
            if let Some(..) = untracked.remove(&filename as &str) {
                continue;
            }
            let old = entries[&filename as &str];
            if let EntryState::Changed {
                staged_status: EntryLocationStatus::Deleted,
                ..
            } = old.state
            {
                entries.insert(
                    old.filename,
                    StatusEntry {
                        filename: old.filename,
                        state: EntryState::Changed {
                            staged_status: EntryLocationStatus::Deleted,
                            tree_status: EntryLocationStatus::Deleted,
                        },
                    },
                );
            }
        }
        let mut sorted_entries = entries
            .values()
            .chain(untracked.values())
            .collect::<Vec<&StatusEntry>>();
        sorted_entries.sort_by_key(|v| v.filename);
        return sorted_entries.iter().map(|x| **x).collect();
    }
}

fn parse_location_status(spec: &str) -> (EntryLocationStatus, EntryLocationStatus) {
    let staged_status = spec[..1].parse::<EntryLocationStatus>().unwrap();
    let tree_status = spec[1..].parse::<EntryLocationStatus>().unwrap();
    (staged_status, tree_status)
}

impl<'a> Iterator for StatusIter<'a> {
    type Item = StatusEntry<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        for line in &mut self.raw_entries {
            let (es, mut remain) = line.split_at(2);
            let se = match es {
                "? " => EntryState::Untracked,
                "! " => EntryState::Ignored,
                "1 " => {
                    let (staged_status, tree_status) = parse_location_status(&remain[..2]);
                    remain = &remain[111..];
                    EntryState::Changed {
                        staged_status,
                        tree_status,
                    }
                }
                "2 " => {
                    let (staged_status, tree_status) = parse_location_status(&remain[..2]);
                    let score = &remain[111..];
                    let mut score_remain = score.splitn(2, ' ');
                    score_remain.next();
                    remain = score_remain.next().unwrap();
                    EntryState::Renamed {
                        staged_status,
                        tree_status,
                        old_filename: self.raw_entries.next().unwrap(),
                    }
                }
                _ => continue,
            };
            let filename = remain;
            return Some(StatusEntry {
                state: se,
                filename,
            });
        }
        None
    }
}

#[derive(Debug)]
struct Commit {
    sha: String,
}

impl Commit {
    /*fn get_tree(self) -> String {
        output_to_string(
            &run_git_command(&["show", "--pretty=format:%T", "-q", &self.sha])
                .expect("Cannot find tree."),
        )
    }*/
    fn get_tree_reference(self) -> String {
        format!("{}^{{tree}}", self.sha)
    }
    fn find_merge_base(&self, commit: &str) -> Commit {
        let output = run_git_command(&["merge-base", &self.sha, commit]);
        Commit {
            sha: output_to_string(&output.expect("Couldn't find merge base.")),
        }
    }
}

#[derive(Debug)]
enum CommitErr {
    NoCommit { spec: String },
}

impl fmt::Display for CommitErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CommitErr::NoCommit { spec } => write!(f, "No commit found for \"{}\"", spec),
        }
    }
}

impl FromStr for Commit {
    type Err = CommitErr;
    fn from_str(spec: &str) -> std::result::Result<Self, <Self as FromStr>::Err> {
        match eval_rev_spec(spec) {
            Err(..) => Err(CommitErr::NoCommit {
                spec: spec.to_string(),
            }),
            Ok(sha) => Ok(Commit { sha }),
        }
    }
}

fn base_tree() -> String {
    match Commit::from_str("HEAD") {
        Ok(commit) => commit.get_tree_reference(),
        Err(..) => "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string(),
    }
}

#[enum_dispatch(NativeCommand)]
trait Runnable {
    fn run(self) -> i32;
}

#[derive(Debug, StructOpt)]
struct Switch {
    /// The branch to switch to.
    branch: String,
    #[structopt(long, short)]
    create: bool,
}

impl Runnable for Switch {
    fn run(self) -> i32 {
        match eval_rev_spec(&format!("refs/heads/{}", self.branch)) {
            Err(..) => {
                if !self.create {
                    if let Err(..) = eval_rev_spec(&format!("refs/remotes/origin/{}", self.branch))
                    {
                        eprintln!("Branch {} not found", self.branch);
                        exit(1);
                    }
                }
            }
            Ok(..) => {
                if self.create {
                    eprintln!("Branch {} already exists", self.branch);
                    exit(1);
                }
            }
        };
        if self.create {
            eprintln!("Retaining any local changes.");
        } else if let Some(current_ref) = create_branch_stash() {
            eprintln!("Stashed WIP changes to {}", current_ref);
        } else {
            eprintln!("No changes to stash");
        }
        git_switch(&self.branch, self.create, !self.create);
        eprintln!("Switched to {}", self.branch);
        if !self.create {
            if apply_branch_stash(&self.branch) {
                eprintln!("Applied WIP changes for {}", self.branch);
            } else {
                eprintln!("No WIP changes for {} to restore", self.branch);
            }
        }
        0
    }
}

#[derive(Debug, StructOpt)]
struct FakeMerge {
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
struct Checkout {
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
struct Status {}

impl Runnable for Status {
    fn run(self) -> i32 {
        let gs = GitStatus::new();
        let mut gs_iter = gs.iter();
        for se in gs_iter.fix_removals() {
            let track_char = match se.state {
                EntryState::Untracked => "?",
                EntryState::Ignored => "!",
                EntryState::Changed { staged_status, .. } => match staged_status {
                    EntryLocationStatus::Added => "+",
                    EntryLocationStatus::Deleted => "-",
                    _ => " ",
                },
                EntryState::Renamed { .. } => "R",
            };
            let disk_char = match se.state {
                EntryState::Untracked => "?",
                EntryState::Ignored => "!",
                EntryState::Changed {
                    staged_status,
                    tree_status,
                } => {
                    match (staged_status, tree_status) {
                        (.., EntryLocationStatus::Deleted) => "D",
                        (EntryLocationStatus::Added, ..) => "A",
                        // TODO: distinguish between "-D" and "- ".
                        // Note: "- " is expressed as a double entry with -D
                        // and ?? for the same filename.
                        (EntryLocationStatus::Modified, ..) => "M",
                        (.., EntryLocationStatus::Modified) => "M",
                        _ => " ",
                    }
                }
                EntryState::Renamed { tree_status, .. } => match tree_status {
                    EntryLocationStatus::Unmodified => " ",
                    EntryLocationStatus::Modified => "M",
                    _ => "$",
                },
            };
            let rename_str = if let EntryState::Renamed { old_filename, .. } = se.state {
                format!("{} -> ", old_filename)
            } else {
                "".to_owned()
            };
            println!("{}{} {}{}", track_char, disk_char, rename_str, se.filename);
        }
        1
    }
}

#[derive(Debug, StructOpt)]
struct Push {}

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

#[enum_dispatch]
#[derive(Debug, StructOpt)]
enum NativeCommand {
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

#[enum_dispatch(RewriteCommand)]
trait ArgMaker {
    fn make_args(self) -> Result<Vec<String>, i32>;
}

#[derive(Debug, StructOpt)]
struct Cat {
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
struct CommitCmd {
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

impl Runnable for CommitCmd {
    fn run(self) -> i32 {
        if !self.no_strict {
            let status = GitStatus::new();
            let untracked: Vec<StatusEntry> = status
                .iter()
                .filter(|f| matches!(f.state, EntryState::Untracked))
                .collect();
            if !untracked.is_empty() {
                eprintln!("Untracked files are present:");
                for entry in untracked {
                    eprintln!("{}", entry.filename);
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
struct Diff {
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
struct Log {
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
struct Merge {
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
struct MergeDiff {
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
struct Pull {
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
struct Restore {
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
enum RewriteCommand {
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

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt {
    #[structopt(flatten)]
    NativeCommand(NativeCommand),
    #[structopt(flatten)]
    RewriteCommand(RewriteCommand),
}

fn output_to_string(output: &Output) -> String {
    from_utf8(&output.stdout)
        .expect("Output is not utf-8")
        .trim()
        .to_string()
}

fn run_for_string(cmd: &mut Command) -> String {
    output_to_string(&cmd.output().expect("Could not run command."))
}

fn get_current_branch() -> String {
    run_for_string(&mut make_git_command(&["branch", "--show-current"]))
}

fn branch_setting(branch: &str, setting: &str) -> String {
    format!("branch.{}.{}", branch, setting)
}

fn setting_exists(setting: &str) -> bool {
    match run_git_command(&["config", "--get", setting]) {
        Ok(..) => true,
        Err(..) => false,
    }
}

fn create_stash() -> Option<String> {
    let oid = run_for_string(&mut make_git_command(&["stash", "create"]));
    if oid.is_empty() {
        return None;
    }
    Some(oid)
}

fn create_branch_stash() -> Option<String> {
    let current_ref = make_wip_ref(&get_current_branch());
    match create_stash() {
        Some(oid) => {
            if let Err(..) = upsert_ref(&current_ref, &oid) {
                panic!("Failed to set reference {} to {}", current_ref, oid);
            }
            Some(current_ref)
        }
        None => {
            if let Err(..) = delete_ref(&current_ref) {
                panic!("Failed to delete ref {}", current_ref);
            }
            None
        }
    }
}

fn run_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Result<Output, Output> {
    let output = make_git_command(args_vec)
        .output()
        .expect("Couldn't run command");
    if !output.status.success() {
        return Err(output);
    }
    Ok(output)
}

fn eval_rev_spec(rev_spec: &str) -> Result<String, Output> {
    Ok(output_to_string(&run_git_command(&[
        "rev-list", "-n1", rev_spec,
    ])?))
}

fn apply_branch_stash(target_branch: &str) -> bool {
    let target_ref = make_wip_ref(target_branch);
    match eval_rev_spec(&target_ref) {
        Err(..) => false,
        Ok(target_oid) => {
            run_git_command(&["stash", "apply", &target_oid]).unwrap();
            delete_ref(&target_ref).unwrap();
            true
        }
    }
}

fn git_switch(target_branch: &str, create: bool, discard_changes: bool) {
    // Actual "switch" is not broadly deployed yet.
    // let mut switch_cmd = vec!["switch", "--discard-changes"];
    // --force means "discard local changes".
    let mut switch_cmd = vec!["checkout"];
    if discard_changes {
        switch_cmd.push("--force");
    }
    if create {
        if discard_changes {
            if let Err(..) = run_git_command(&["reset", "--hard"]) {
                panic!("Failed to reset tree");
            }
        }
        switch_cmd.push("-b");
    }
    switch_cmd.push(target_branch);
    if let Err(..) = run_git_command(&switch_cmd) {
        panic!("Failed to switch to {}", target_branch);
    }
}

fn make_wip_ref(branch: &str) -> String {
    format!("refs/branch-wip/{}", branch)
}

fn upsert_ref(git_ref: &str, value: &str) -> Result<(), Output> {
    run_git_command(&["update-ref", git_ref, value])?;
    Ok(())
}

fn delete_ref(git_ref: &str) -> Result<(), Output> {
    run_git_command(&["update-ref", "-d", git_ref])?;
    Ok(())
}

fn set_head(new_head: &str) {
    run_git_command(&["reset", "--soft", new_head]).expect("Failed to update HEAD.");
}

enum Args {
    NativeCommand(NativeCommand),
    GitCommand(Vec<String>),
}

fn parse_args() -> Args {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec: Vec<String> = args_iter.collect();
    let args_vec2: Vec<String> = env::args().collect();
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "nit" => {
            if args_vec2.len() > 1 {
                let x = Opt::from_iter_safe(&args_vec2[0..2]);
                if let Err(err) = x {
                    if err.kind == clap::ErrorKind::UnknownArgument {
                        return Args::GitCommand(args_vec);
                    }
                    if err.kind == clap::ErrorKind::InvalidSubcommand {
                        return Args::GitCommand(args_vec);
                    }
                }
            }
            Opt::from_args()
        }
        _ => {
            let mut args = vec!["nit".to_string()];
            let mut subcmd_iter = progname.split('-');
            subcmd_iter.next();
            for arg in subcmd_iter {
                args.push(arg.to_string());
            }
            for arg in &args_vec {
                args.push(arg.to_string());
            }
            Opt::from_iter(args)
        }
    };
    match opt {
        Opt::RewriteCommand(cmd) => Args::GitCommand(match cmd.make_args() {
            Ok(args) => args,
            Err(status) => {
                exit(status);
            }
        }),
        Opt::NativeCommand(cmd) => Args::NativeCommand(cmd),
    }
}

fn make_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args_vec);
    cmd
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::NativeCommand(cmd) => exit(cmd.run()),
        Args::GitCommand(args_vec) => {
            make_git_command(&args_vec).exec();
        }
    };
}
