use super::git::{
    get_current_branch, get_git_path, get_settings, get_toplevel, make_git_command, setting_exists,
    short_branch, LocalBranchName, ReferenceSpec, SettingEntry,
};
use super::worktree::{
    append_lines, base_tree, relative_path, stash_switch, target_branch_setting, Commit, CommitErr,
    CommitSpec, Commitish, GitStatus, SomethingSpec, SwitchErr, Tree, Treeish, WorktreeHead,
};
use enum_dispatch::enum_dispatch;
use std::env;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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
pub struct Show {
    commit: Option<CommitSpec>,
}

impl ArgMaker for Show {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let mut cmd: Vec<String> = ["show", "-m", "--first-parent"]
            .iter()
            .map(|s| s.to_string()).collect();
        cmd.extend(self.commit.into_iter().map(|c| c.spec));
        Ok(cmd)
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
            cmd_args.push("--histogram");
        }
        if self.name_only {
            cmd_args.push("--name-only");
        }
        let mut cmd_args: Vec<String> = cmd_args.iter().map(|s| s.to_string()).collect();
        cmd_args.push(match &self.source {
            Some(source) => source.sha.to_owned(),
            None => match base_tree() {
                Ok(tree) => tree.get_tree_reference(),
                Err(err) => {
                    eprintln!("{}", err);
                    return Err(1);
                }
            },
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
            cmd_args.push("-m");
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
    #[structopt(long, short)]
    source: Option<CommitSpec>,
}

impl ArgMaker for Merge {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let source = if let Some(source) = self.source {
            source
        } else {
            match find_target() {
                Ok(Some(target)) => {
                    eprintln!("Using remembered value {:?}", short_branch(&target.spec));
                    target
                }
                Ok(None) => {
                    eprintln!("Source not supplied and no remembered source.");
                    return Err(1);
                }
                Err(err) => {
                    eprintln!("{}", err);
                    return Err(1);
                }
            }
        };
        Ok(["merge", "--no-commit", "--no-ff", &source.spec]
            .iter()
            .map(|s| s.to_string())
            .collect())
    }
}

#[derive(Debug, StructOpt)]
pub struct MergeDiff {
    /// The branch you would merge into.  (Though any commitish will work.)
    #[structopt(long, short)]
    target: Option<CommitSpec>,
    /// Use the meyers diff algorithm.  (Faster, can produce more confusing diffs.)
    #[structopt(long)]
    myers: bool,
    /// Emit modified filenames only, not diffs.
    #[structopt(long)]
    name_only: bool,
    path: Vec<String>,
}

/**
 * Find a commit spec to merge into.
 * note: Errors could be caused by a failed status command instead of a failed parse.
 */
fn find_target() -> Result<Option<CommitSpec>, CommitErr> {
    let branch_name: LocalBranchName = match GitStatus::new() {
        Ok(GitStatus {
            head: WorktreeHead::Attached { head, .. },
            ..
        }) => head.parse().unwrap(),
        Err(err) => {
            return Err(CommitErr::GitError(err));
        }
        _ => {
            return Ok(None);
        }
    };
    let mut remote = None;
    let target_branch = {
        let mut target_branch = None;
        let prefix = branch_name.setting_name("");
        let target_setting = target_branch_setting(&branch_name);
        let remote_setting = branch_name.setting_name("remote");
        for entry in get_settings(&prefix, &["oaf-target-branch", "remote"]) {
            if let SettingEntry::Valid { key, value } = entry {
                if key == target_setting {
                    target_branch = Some(value);
                } else if key == remote_setting {
                    remote = Some(value);
                }
            }
        }
        if let Some(target_branch) = target_branch {
            if let Some(remote) = remote {
                let local_branch = LocalBranchName {
                    name: short_branch(&target_branch),
                };
                local_branch.with_repo(remote).full()
            } else {
                target_branch
            }
        } else {
            return Ok(None);
        }
    };
    Ok(Some(target_branch.parse()?))
}

impl ArgMaker for MergeDiff {
    fn make_args(self) -> Result<Vec<String>, i32> {
        if let Err(..) = Commit::from_str("HEAD") {
            eprintln!("Cannot merge-diff: no commits in HEAD.");
            return Err(1);
        }
        let target = match self.target {
            Some(target) => target,
            None => match find_target() {
                Ok(Some(target)) => {
                    eprintln!("Using remembered value {:?}", short_branch(&target.spec));
                    target
                }
                Ok(None) => {
                    eprintln!("Target not supplied and no saved target.");
                    return Err(1);
                }
                Err(err) => {
                    eprintln!("{}", err);
                    return Err(1);
                }
            },
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

#[derive(Debug, StructOpt)]
pub struct Pull {
    ///The Remote entry to pull from
    remote: Option<String>,
    ///The branch to pull from
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
    source: Option<SomethingSpec>,
    /// File(s) to restore
    #[structopt(required = true)]
    path: Vec<String>,
}

impl ArgMaker for Restore {
    fn make_args(self) -> Result<Vec<String>, i32> {
        let source = if let Some(source) = self.source {
            source
        } else {
            match SomethingSpec::from_str("HEAD") {
                Ok(source) => source,
                Err(CommitErr::NoCommit { .. }) => {
                    eprintln!("Cannot restore: no commits in HEAD.");
                    return Err(1);
                }
                Err(CommitErr::GitError(err)) => {
                    eprintln!("{}", err);
                    return Err(1);
                }
            }
        };
        let source = source.get_treeish_spec();
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
    /// Summarize a commit or other object
    Show,
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
    /// Convert all commits from a branch-point into a single commit.
    SquashCommit,
    /// Disabled to prevent accidentally discarding stashed changes.
    Checkout,
    /// Show the status of changed and unknown files in the working tree.
    Status,
    /// Tell git to ignore a file.
    ///
    /// This updates the top-level .gitignore, not any lower ones.
    Ignore,
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
        make_git_command(&match self.make_args() {
            Ok(args) => args,
            Err(status) => return status,
        })
        .exec();
        0
    }
}

#[derive(Debug, StructOpt)]
pub struct Push {
    #[structopt(long, short)]
    /// Allow changing history on the remote branch
    force: bool,
}

impl Runnable for Push {
    fn run(self) -> i32 {
        let branch = match get_current_branch().parse::<LocalBranchName>() {
            Ok(branch) => branch,
            Err(unhandled) => {
                eprintln!("Unhandled: {}", unhandled.name);
                return 1;
            }
        };
        let mut args;
        if setting_exists(&branch.setting_name("remote")) {
            if !setting_exists(&branch.setting_name("merge")) {
                panic!("Branch in unsupported state");
            }
            args = vec!["push".to_string()];
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
            args = ["push", "-u", "origin", "HEAD"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        }
        if self.force {
            args.push("--force".to_string());
        }
        make_git_command(&args).exec();
        0
    }
}

#[derive(Debug, StructOpt)]
pub struct Switch {
    /// The branch to switch to.
    branch: LocalBranchName,
    #[structopt(long, short)]
    create: bool,
}

impl Runnable for Switch {
    fn run(self) -> i32 {
        if self.create && !self.branch.is_valid() {
            eprintln!("'{}' is not a valid branch name.", self.branch.name);
            return 1;
        }
        match stash_switch(&self.branch, self.create) {
            Ok(()) => 0,
            Err(SwitchErr::BranchInUse { path }) => {
                println!("Branch {} is already in use at {}", self.branch.name, path);
                1
            }
            Err(SwitchErr::AlreadyExists) => {
                eprintln!("Branch {} already exists", self.branch.name);
                1
            }
            Err(SwitchErr::NotFound) => {
                eprintln!("Branch {} not found", self.branch.name);
                1
            }
            Err(SwitchErr::GitError(err)) => {
                eprintln!("{}", err);
                1
            }
        }
    }
}

#[derive(Debug, StructOpt)]
pub struct FakeMerge {
    /// The source for the fake merge.
    source: CommitSpec,
    /// The message to use for the fake merge.  (Default: "Fake merge.")
    #[structopt(long, short)]
    message: Option<String>,
}

impl Runnable for FakeMerge {
    fn run(self) -> i32 {
        let head = match head_for_squash() {
            Ok(head) => head,
            Err(exit_status) => return exit_status,
        };
        let message = if let Some(msg) = &self.message {
            &msg
        } else {
            "Fake merge."
        };
        let fm_commit = head
            .commit(&head, Some(&self.source), message)
            .expect("Could not generate commit.");
        fm_commit.set_wt_head();
        0
    }
}

#[derive(Debug, StructOpt)]
pub struct SquashCommit {
    /// The item we want to squash relative to.  All commits between the common ancestor with HEAD
    /// will be squashed.  Typically, this is the branch you want to merge into.
    branch_point: CommitSpec,
    /// The message to use for the squash commit.  (Default: "Squash commit.")
    #[structopt(long, short)]
    message: Option<String>,
}

fn head_for_squash() -> Result<Commit, i32> {
    if let Ok(head) = Commit::from_str("HEAD") {
        Ok(head)
    } else {
        eprintln!("Cannot squash commit: no commits in HEAD.");
        Err(1)
    }
}

impl Runnable for SquashCommit {
    fn run(self) -> i32 {
        let head = match head_for_squash() {
            Ok(head) => head,
            Err(exit_status) => return exit_status,
        };
        let parent = head.find_merge_base(&self.branch_point);
        let message = if let Some(msg) = &self.message {
            &msg
        } else {
            "Squash commit"
        };
        let fm_commit = head
            .commit(&parent, None, message)
            .expect("Could not generate commit.");
        fm_commit.set_wt_head();
        eprintln!("Commit squashed.  To undo: oaf reset {}", head.sha);
        0
    }
}
#[derive(Debug, StructOpt)]
pub struct Checkout {
    /// The branch to switch to.
    _branch_name: String,
    #[structopt(long, short)]
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

#[derive(Debug, StructOpt)]
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
                println!("On branch {}", head);
                if let Some(upstream) = upstream {
                    if upstream.added == 0 && upstream.removed == 0 {
                        println!("Your branch is up to date with '{}'.", upstream.name);
                    } else if upstream.added == 0 {
                        println!(
                            "Your branch is behind '{}' by {} commit(s), and can be fast-forwarded.", upstream.name, upstream.removed);
                    } else if upstream.removed == 0 {
                        println!(
                            "Your branch is ahead of '{}' by {} commit(s).",
                            upstream.name, upstream.added
                        );
                        println!("  (use \"oaf push\" to publish your local commits");
                    } else {
                        println!("Your branch and '{}' have diverged,", upstream.name);
                        println!(
                            "and have {} and {} different commits each, respectively.",
                            upstream.added, upstream.removed
                        );
                        println!(
                            "  (use \"oaf merge {}\" to merge the remote branch into yours)",
                            upstream.name
                        );
                    }
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

#[derive(Debug, StructOpt)]
pub struct Ignore {
    /// Ignores the file in the local repository, instead of the worktree .gitignore.
    #[structopt(long)]
    local: bool,
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

fn add_ignores(new_files: Vec<String>, ignore_file: &Path) {
    let ignores = fs::read_to_string(&ignore_file).expect("Can't read ignores");
    fs::write(ignore_file, append_lines(ignores, new_files)).expect("Can't write .gitignore");
}

impl Runnable for Ignore {
    fn run(self) -> i32 {
        let mut new_files = vec![];
        let top = PathBuf::from(match get_toplevel() {
            Ok(top) => top,
            Err(err) => {
                eprintln!("{}", err);
                return 1;
            }
        });
        let top = top.canonicalize().unwrap();
        for file in &self.files {
            let path = normpath(&PathBuf::from(file)).unwrap();
            let mut relpath = relative_path(&top, path)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            if !relpath.contains('/') {
                relpath.insert(0, '/');
            }
            new_files.push(relpath);
        }
        let ignore_file = if self.local {
            get_git_path("info/exclude")
        } else {
            top.join(".gitignore")
        };
        add_ignores(new_files, &ignore_file);
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
}
