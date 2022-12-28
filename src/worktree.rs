// Copyright 2021 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::git::{
    create_stash, delete_ref, eval_rev_spec, get_toplevel, git_switch, make_git_command,
    output_to_string, resolve_refname, run_git_command, set_head, set_setting, upsert_ref,
    BranchName, ConfigErr, GitError, LocalBranchName, ReferenceSpec, SettingLocation,
    UnparsedReference,
};
use enum_dispatch::enum_dispatch;
use std::collections::HashMap;
use std::fmt;
use std::io::prelude::*;
use std::path::{Path, PathBuf, StripPrefixError};
use std::process::{Output, Stdio};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryLocationStatus {
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

fn parse_location_status(spec: &str) -> (EntryLocationStatus, EntryLocationStatus) {
    let staged_status = spec[..1].parse::<EntryLocationStatus>().unwrap();
    let tree_status = spec[1..].parse::<EntryLocationStatus>().unwrap();
    (staged_status, tree_status)
}

pub fn relative_path<T: AsRef<Path>, U: AsRef<Path>>(
    from: T,
    to: U,
) -> Result<PathBuf, StripPrefixError> {
    let mut result = PathBuf::from("");
    let to = to.as_ref();
    let from = from.as_ref();
    if from.has_root() == to.has_root() {
        for ancestor in from.ancestors() {
            if let Ok(relpath) = to.strip_prefix(ancestor) {
                result.push(relpath);
                return Ok(result);
            }
            result.push("..");
        }
    }
    Ok(PathBuf::from(to.strip_prefix(from)?))
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct StatusEntry<'a> {
    pub state: EntryState<'a>,
    pub filename: &'a str,
}

impl StatusEntry<'_> {
    pub fn format_entry<T: AsRef<Path>>(&self, current_dir: &T) -> String {
        let track_char = match self.state {
            EntryState::Untracked => "?",
            EntryState::Ignored => "!",
            EntryState::Changed { staged_status, .. } => match staged_status {
                EntryLocationStatus::Added => "+",
                EntryLocationStatus::Deleted => "-",
                _ => " ",
            },
            EntryState::Renamed { .. } => "R",
            EntryState::Unmerged { .. } => "C",
        };
        let disk_char = match self.state {
            EntryState::Untracked => "?",
            EntryState::Ignored => "!",
            EntryState::Changed {
                staged_status,
                tree_status,
            } => match (staged_status, tree_status) {
                (.., EntryLocationStatus::Deleted) => "D",
                (EntryLocationStatus::Added, ..) => "A",
                (EntryLocationStatus::Modified, ..) => "M",
                (.., EntryLocationStatus::Modified) => "M",
                _ => " ",
            },
            EntryState::Renamed { tree_status, .. } => match tree_status {
                EntryLocationStatus::Unmodified => " ",
                EntryLocationStatus::Modified => "M",
                _ => "$",
            },
            EntryState::Unmerged { state } => match state {
                UnmergedState::BothModified => "M",
                UnmergedState::Added(Changer::Both) | UnmergedState::Added(Changer::Us) => "A",
                UnmergedState::Deleted(Changer::Both) | UnmergedState::Deleted(Changer::Us) => "D",
                UnmergedState::Deleted(Changer::Them) | UnmergedState::Added(Changer::Them) => " ",
            },
        };
        let rename_str = if let EntryState::Renamed { old_filename, .. } = self.state {
            format!(
                "{} -> ",
                relative_path(current_dir, old_filename)
                    .unwrap()
                    .to_string_lossy()
            )
        } else {
            "".to_owned()
        };
        format!(
            "{}{} {}{}",
            track_char,
            disk_char,
            rename_str,
            relative_path(current_dir, self.filename)
                .unwrap()
                .to_string_lossy()
        )
    }
}

pub struct StatusIter<'a> {
    raw_entries: std::str::SplitTerminator<'a, char>,
}

impl StatusIter<'_> {
    /**
     * Convert a "D." to "DD" if the file was deleted as well as being removed.  If the file was
     * not deleted, skip its ?? entry.
     **/
    pub fn fix_removals(&mut self) -> Vec<StatusEntry> {
        let mut entries = HashMap::new();
        let mut untracked = HashMap::new();
        for se in self {
            let kind_map = match se.state {
                EntryState::Untracked | EntryState::Ignored => &mut untracked,
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
                    remain = match score.split_once(' ') {
                        Some((_, remain)) => remain,
                        _ => {
                            panic!("Malformed entry {}", score);
                        }
                    };
                    EntryState::Renamed {
                        staged_status,
                        tree_status,
                        old_filename: self.raw_entries.next().unwrap(),
                    }
                }
                "u " => {
                    let result = EntryState::Unmerged {
                        state: remain[..2].parse().unwrap(),
                    };
                    remain = &remain[159..];
                    result
                }
                "# " => {
                    continue;
                }
                _ => {
                    eprintln!("Unhandled: {}", line);
                    continue;
                }
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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Changer {
    Both,
    Us,
    Them,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum UnmergedState {
    Added(Changer),
    BothModified,
    Deleted(Changer),
}

#[derive(Debug)]
pub enum UnmergedStateParseError {
    UnhandledString,
}

impl FromStr for UnmergedState {
    type Err = UnmergedStateParseError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        Ok(match spec {
            "DD" => UnmergedState::Deleted(Changer::Both),
            "AU" => UnmergedState::Added(Changer::Us),
            "UD" => UnmergedState::Deleted(Changer::Them),
            "UA" => UnmergedState::Added(Changer::Them),
            "DU" => UnmergedState::Deleted(Changer::Us),
            "AA" => UnmergedState::Added(Changer::Both),
            "UU" => UnmergedState::BothModified,
            _ => return Err(UnmergedStateParseError::UnhandledString),
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryState<'a> {
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
    Unmerged {
        state: UnmergedState,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum BranchCommit {
    Initial,
    Oid(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct UpstreamInfo {
    pub name: String,
    pub added: u16,
    pub removed: u16,
}

#[derive(PartialEq, Debug, Eq)]
pub enum WorktreeHead {
    Detached(String),
    Attached {
        commit: BranchCommit,
        head: LocalBranchName,
        upstream: Option<UpstreamInfo>,
    },
}

impl UpstreamInfo {
    fn factory<'a>(mut raw_entries: impl Iterator<Item = &'a str>) -> Option<Self> {
        let name = match raw_entries
            .next()
            .map(|r| r.split_once("# branch.upstream "))
            .unwrap_or(None)
        {
            Some(("", name)) => name.to_string(),
            _ => return None,
        };
        let branch_info = raw_entries.next().unwrap();
        let segments = branch_info.split_once("# branch.ab ");
        let Some((_, commits)) = segments else {
            panic!("Malformed branch info: {}", branch_info);
        };
        let Some((added_str, removed_str)) = commits.split_once(' ') else {
            panic!("Malformed commit info: {}", commits);
        };
        let ("+", Ok(added), "-", Ok(removed)) = (
            &added_str[0..1],
            added_str[1..].parse::<u16>(),
            &removed_str[0..1],
            removed_str[1..].parse::<u16>(),
        ) else {
            panic!("malformed commit info: {}", commits);
        };
        Some(UpstreamInfo {
            name,
            added,
            removed,
        })
    }
}

pub fn make_worktree_head<'a>(mut raw_entries: impl Iterator<Item = &'a str>) -> WorktreeHead {
    if let Some(raw_oid) = raw_entries.next() {
        let Some(("", oid)) = raw_oid.split_once("# branch.oid ") else {
            panic!()
        };
        let Some(raw_head) = raw_entries.next() else { panic!() };
        let Some(("", head)) = raw_head.split_once("# branch.head ") else { panic!() };
        if head == "(detached)" {
            WorktreeHead::Detached(oid.to_string())
        } else {
            let upstream = UpstreamInfo::factory(raw_entries);
            WorktreeHead::Attached {
                commit: BranchCommit::Oid(oid.to_string()),
                head: LocalBranchName {
                    name: head.to_string(),
                },
                upstream,
            }
        }
    } else {
        WorktreeHead::Attached {
            commit: BranchCommit::Initial,
            head: LocalBranchName {
                name: "".to_string(),
            },
            upstream: Some(UpstreamInfo {
                name: "".to_string(),
                added: 0,
                removed: 0,
            }),
        }
    }
}

/// Represents `git status` output
#[derive(Debug)]
pub struct GitStatus {
    outstr: String,
    pub head: WorktreeHead,
}

impl GitStatus {
    ///Return an iterator over [StatusEntry]
    pub fn iter(&self) -> StatusIter {
        StatusIter {
            // Note: there is an extra entry for each rename entry, consisting of the original
            // filename.  This is an inevitable consequence of splitting using a terminator instead
            // of performing the entry iteration in StatusIter::next()
            raw_entries: self.outstr.split_terminator('\0'),
        }
    }

    ///Return an [GitStatus] for the current directory
    pub fn new() -> Result<GitStatus, GitError> {
        let output = match run_git_command(&["status", "--porcelain=v2", "-z", "--branch"]) {
            Err(output) => match GitError::from(output) {
                GitError::UnknownError(_) => {
                    panic!("Couldn't list directory");
                }
                err => Err(err),
            }?,
            Ok(output) => output,
        };
        let outstr = output_to_string(&output);
        let info_iter = outstr.split_terminator('\0');
        let head = make_worktree_head(info_iter);
        let result = GitStatus { outstr, head };
        Ok(result)
    }

    /** List untracked filenames

    This is a convenience wrapper for callers that just want to fail on untracked files.
     */
    pub fn untracked_filenames(&self) -> Vec<String> {
        self.iter()
            .filter(|f| matches!(f.state, EntryState::Untracked))
            .map(|es| es.filename.to_string())
            .collect()
    }
}

/// Refers to a tree object specifically, not a commit
pub trait Tree {
    fn get_tree_reference(&self) -> String;

    /// Use the commit-tree command to generate a fake-merge commit.
    fn commit<P: Commitish>(
        &self,
        parent: &P,
        merge_parent: Option<&dyn Commitish>,
        message: &str,
    ) -> Result<Commit, Output> {
        let mut cmd = vec!["commit-tree".to_string(), "-p".to_string()];
        let parent_spec = parent.get_commit_spec();
        cmd.push(parent_spec);
        if let Some(merge_parent) = merge_parent {
            cmd.push("-p".to_string());
            cmd.push(merge_parent.get_commit_spec());
        }
        cmd.push(self.get_tree_reference());
        cmd.push("-m".to_string());
        cmd.push(message.to_string());
        let output = run_git_command(&cmd)?;
        Ok(Commit {
            sha: output_to_string(&output),
        })
    }
}

/// Refers to a treeish object, whether tree or commit.
#[enum_dispatch]
pub trait Treeish {
    fn get_treeish_spec(self) -> String;
}

/// Object that refers to a commit object, not a tree.
pub trait Commitish {
    fn get_commit_spec(&self) -> String;
    fn find_merge_base(&self, commit: &dyn Commitish) -> Commit {
        let output = run_git_command(&[
            "merge-base",
            &self.get_commit_spec(),
            &commit.get_commit_spec(),
        ]);
        Commit {
            sha: output_to_string(&output.expect("Couldn't find merge base.")),
        }
    }
}

impl<T: Commitish> Tree for T {
    fn get_tree_reference(&self) -> String {
        format!("{}^{{tree}}", self.get_commit_spec())
    }
}

impl<T: Commitish> Treeish for T {
    fn get_treeish_spec(self) -> String {
        self.get_commit_spec()
    }
}

#[derive(Debug, Clone)]
pub struct TreeSpec {
    // Must identify a tree suitably for commit-tree, not just a commit
    // oid / reference.
    pub reference: String,
}

impl Tree for TreeSpec {
    fn get_tree_reference(&self) -> String {
        self.reference.clone()
    }
}

impl Treeish for TreeSpec {
    fn get_treeish_spec(self) -> String {
        self.get_tree_reference()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
}

impl Commit {
    /*fn get_tree(self) -> String {
        output_to_string(
            &run_git_command(&["show", "--pretty=format:%T", "-q", &self.sha])
                .expect("Cannot find tree."),
        )
    }*/
    pub fn set_wt_head(&self) {
        set_head(&self.sha);
    }
}

mod ers {
    use super::{
        resolve_refname, BranchName, Commit, CommitSpec, FromStr, ReferenceSpec, UnparsedReference,
    };
    #[derive(Debug)]
    pub struct ExtantRefName {
        pub name: Result<BranchName, UnparsedReference>,
        commit: Commit,
    }

    impl ExtantRefName {
        pub fn resolve(refname: &str) -> Option<Self> {
            let (full_spec, sha) = resolve_refname(refname)?;
            let name: Result<BranchName, UnparsedReference> = BranchName::from_str(&full_spec);
            Some(Self {
                name,
                commit: Commit { sha },
            })
        }
        pub fn extract(self) -> (Result<BranchName, UnparsedReference>, Commit) {
            (self.name, self.commit)
        }
    }
    impl From<ExtantRefName> for CommitSpec {
        fn from(expec: ExtantRefName) -> Self {
            Self {
                spec: expec.full(),
                _commit: expec.commit,
            }
        }
    }
}

pub use self::ers::ExtantRefName;

impl TryFrom<Result<BranchName, UnparsedReference>> for ExtantRefName {
    type Error = CommitErr;
    fn try_from(name: Result<BranchName, UnparsedReference>) -> Result<ExtantRefName, Self::Error> {
        let full = match name {
            Ok(ref name) => name.full(),
            Err(ref name) => name.full(),
        };
        match ExtantRefName::resolve(&full) {
            Some(refspec) => Ok(refspec),
            None => Err(CommitErr::NoCommit { spec: full }),
        }
    }
}

impl ReferenceSpec for ExtantRefName {
    fn full(&self) -> String {
        match &self.name {
            Ok(name) => name.full(),
            Err(name) => name.full(),
        }
    }
    fn short(&self) -> String {
        match &self.name {
            Ok(name) => name.short(),
            Err(name) => name.short(),
        }
    }
}

impl Commitish for Commit {
    fn get_commit_spec(&self) -> String {
        self.sha.clone()
    }
}

#[derive(Debug, Clone)]
pub struct CommitSpec {
    pub spec: String,
    _commit: Commit,
}

#[enum_dispatch(Treeish)]
#[derive(Clone, Debug)]
pub enum SomethingSpec {
    CommitSpec(CommitSpec),
    TreeSpec(TreeSpec),
}

impl FromStr for SomethingSpec {
    type Err = CommitErr;

    fn from_str(spec: &str) -> Result<Self, CommitErr> {
        let mut cmd = make_git_command(&["cat-file", "--batch-check"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        cmd.stdin
            .as_ref()
            .unwrap()
            .write_all(spec.as_bytes())
            .unwrap();
        if !cmd.wait().unwrap().success() {
            let mut stderr_bytes = Vec::<u8>::new();
            cmd.stderr.unwrap().read_to_end(&mut stderr_bytes).unwrap();
            return Err(CommitErr::GitError(stderr_bytes.into()));
        }
        let mut result = String::new();
        cmd.stdout.unwrap().read_to_string(&mut result).unwrap();
        if result.ends_with("missing\n") {
            return Err(CommitErr::NoCommit {
                spec: spec.to_string(),
            });
        }
        let mut sections = result.split(' ');
        let oid = sections.next().unwrap();
        let otype = sections.next().unwrap();
        Ok(match otype {
            "commit" => SomethingSpec::CommitSpec(CommitSpec {
                spec: spec.to_string(),
                _commit: Commit {
                    sha: oid.to_string(),
                },
            }),
            "tree" => SomethingSpec::TreeSpec(TreeSpec {
                reference: spec.to_string(),
            }),
            _ => panic!("Unhandled type {}", otype),
        })
    }
}

#[derive(Debug)]
pub enum CommitErr {
    NoCommit { spec: String },
    GitError(GitError),
}

impl fmt::Display for CommitErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CommitErr::NoCommit { spec } => write!(f, "No commit found for \"{}\"", spec),
            CommitErr::GitError(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CommitErr {}

impl From<GitError> for CommitErr {
    fn from(err: GitError) -> Self {
        CommitErr::GitError(err)
    }
}

impl FromStr for CommitSpec {
    type Err = CommitErr;
    fn from_str(spec: &str) -> std::result::Result<Self, <Self as FromStr>::Err> {
        let commit = Commit::from_str(spec)?;
        Ok(CommitSpec {
            spec: spec.to_string(),
            _commit: commit,
        })
    }
}

impl Commitish for CommitSpec {
    fn get_commit_spec(&self) -> String {
        self.spec.clone()
    }
}

impl FromStr for Commit {
    type Err = CommitErr;
    fn from_str(spec: &str) -> std::result::Result<Self, <Self as FromStr>::Err> {
        match eval_rev_spec(spec) {
            Err(proc_output) => match GitError::from(proc_output) {
                GitError::UnknownError(_) => Err(CommitErr::NoCommit {
                    spec: spec.to_string(),
                }),
                err => Err(err.into()),
            },
            Ok(sha) => Ok(Commit { sha }),
        }
    }
}

pub fn base_tree() -> Result<TreeSpec, GitError> {
    let reference = match Commit::from_str("HEAD") {
        Ok(commit) => commit.get_tree_reference(),
        Err(CommitErr::NoCommit { .. }) => "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string(),
        Err(CommitErr::GitError(err)) => return Err(err),
    };
    Ok(TreeSpec { reference })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeState {
    DetachedHead {
        head: Commit,
    },
    UncommittedBranch {
        branch: LocalBranchName,
    },
    CommittedBranch {
        branch: LocalBranchName,
        head: Commit,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeListEntry {
    pub path: String,
    pub state: WorktreeState,
}

fn parse_worktree_list(lines: &str) -> Vec<WorktreeListEntry> {
    let mut line_iter = lines.lines();
    let mut result: Vec<WorktreeListEntry> = vec![];
    loop {
        let Some(line) = line_iter.next() else {break};
        let path = &line[9..];
        let line = line_iter.next().unwrap();
        let head = match &line[5..] {
            "0000000000000000000000000000000000000000" => None,
            _ => Some(Commit {
                sha: line[5..].to_string(),
            }),
        };
        let line = line_iter.next().unwrap();
        let wt_state = if &line[..6] == "branch" {
            match (head, line[7..].parse::<BranchName>()) {
                (Some(head), Ok(BranchName::Local(branch))) => {
                    WorktreeState::CommittedBranch { branch, head }
                }
                (None, Ok(BranchName::Local(branch))) => {
                    WorktreeState::UncommittedBranch { branch }
                }
                _ => {
                    panic!("Unhandled branch: {}", &line[7..])
                }
            }
        } else {
            WorktreeState::DetachedHead {
                head: head.unwrap(),
            }
        };
        result.push(WorktreeListEntry {
            path: path.to_string(),
            state: wt_state,
        });
        line_iter.next();
    }
    result
}

pub fn list_worktree() -> Vec<WorktreeListEntry> {
    let output =
        run_git_command(&["worktree", "list", "--porcelain"]).expect("Couldn't list worktrees");
    parse_worktree_list(&output_to_string(&output))
}

pub fn create_wip_stash(wt: &WorktreeState) -> Option<String> {
    let current_ref = WipReference::from_worktree_state(wt).full();
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

pub fn apply_wip_stash(target_wt: &WorktreeState) -> bool {
    let target_ref = WipReference::from_worktree_state(target_wt);
    let Ok(target_oid) = target_ref.eval() else {return false};
    run_git_command(&["stash", "apply", &target_oid]).unwrap();
    delete_ref(&target_ref.full()).unwrap();
    true
}

pub fn make_wip_ref(wt: &WorktreeState) -> String {
    let branch = match wt {
        WorktreeState::DetachedHead { head } => return format!("refs/commits-wip/{}", head.sha),
        WorktreeState::UncommittedBranch { branch } => branch,
        WorktreeState::CommittedBranch { branch, .. } => branch,
    };
    format!("refs/branch-wip/{}", branch.short())
}

struct WipReference {
    full_name: String,
}

impl WipReference {
    pub fn from_worktree_state(wt: &WorktreeState) -> Self {
        WipReference {
            full_name: make_wip_ref(wt),
        }
    }
}

impl ReferenceSpec for WipReference {
    fn full(&self) -> String {
        self.full_name.to_string()
    }
    fn short(&self) -> String {
        self.full()
    }
}

fn check_switch_branch(
    top: &str,
    branch: &LocalBranchName,
) -> Result<WorktreeListEntry, SwitchErr> {
    let top = PathBuf::from(top).canonicalize().unwrap();
    let mut self_wt = None;
    for wt in list_worktree() {
        if PathBuf::from(&wt.path).canonicalize().unwrap() == top {
            self_wt = Some(wt);
            continue;
        }
        let target_branch = match wt.state {
            WorktreeState::UncommittedBranch { branch } => branch,
            WorktreeState::CommittedBranch { branch, .. } => branch,
            WorktreeState::DetachedHead { .. } => continue,
        };
        if target_branch == *branch {
            return Err(SwitchErr::BranchInUse { path: wt.path });
        }
    }
    Ok(self_wt.expect("Could not find self in worktree list."))
}

pub enum SwitchErr {
    AlreadyExists,
    NotFound,
    BranchInUse { path: String },
    InvalidBranchName(LocalBranchName),
    GitError(GitError),
}

pub fn determine_switch_create_target(
    branch: LocalBranchName,
    head: Option<Commit>,
) -> Result<WorktreeState, SwitchErr> {
    if branch.eval().is_ok() {
        return Err(SwitchErr::AlreadyExists);
    }
    if !branch.is_valid() {
        return Err(SwitchErr::InvalidBranchName(branch));
    }
    let Some(current_head) = head else {return Ok(WorktreeState::UncommittedBranch { branch })};
    Ok(WorktreeState::CommittedBranch {
        branch,
        head: Commit {
            sha: current_head.sha,
        },
    })
}

pub fn determine_switch_target(branch: &str) -> Result<WorktreeState, SwitchErr> {
    let Some(resolved) = ExtantRefName::resolve(branch).map(|r| r.extract()) else {
        return Err(SwitchErr::NotFound);
    };
    let commit = resolved.1;
    let branch = resolved
        .0
        .map(|name| match name {
            BranchName::Local(lb) => lb,
            BranchName::Remote(rb) => LocalBranchName { name: rb.name },
        })
        .ok();
    Ok(if let Some(branch) = branch {
        WorktreeState::CommittedBranch {
            branch,
            head: commit,
        }
    } else {
        WorktreeState::DetachedHead { head: commit }
    })
}

pub fn target_branch_setting(branch: &LocalBranchName) -> String {
    branch.setting_name("oaf-target-branch")
}

#[derive(PartialEq, Eq)]
pub enum SwitchType {
    Create,
    WithStash,
    PlainSwitch,
}

impl From<GitError> for SwitchErr {
    fn from(err: GitError) -> SwitchErr {
        SwitchErr::GitError(err)
    }
}

pub fn stash_switch(branch: &str, switch_type: SwitchType) -> Result<(), SwitchErr> {
    use SwitchType::*;
    let top: Result<String, SwitchErr> = get_toplevel().map_err(|e| e.into());
    let apparent_target = LocalBranchName {
        name: branch.to_owned(),
    };
    let self_wt = check_switch_branch(&top?, &apparent_target)?;
    let self_head = match &self_wt.state {
        WorktreeState::CommittedBranch { head, .. } | WorktreeState::DetachedHead { head } => {
            Some(head)
        }
        WorktreeState::UncommittedBranch { .. } => None,
    };
    let create = switch_type == Create;
    let target_wt = if create {
        determine_switch_create_target(apparent_target, self_head.map(|c| c.to_owned()))?
    } else {
        determine_switch_target(branch)?
    };
    match switch_type {
        Create | PlainSwitch => {
            eprintln!("Retaining any local changes.");
        }
        WithStash => {
            if let Some(current_ref) = create_wip_stash(&self_wt.state) {
                eprintln!("Stashed WIP changes to {}", current_ref);
            } else {
                eprintln!("No changes to stash");
            }
        }
    }
    if let Err(..) = git_switch(branch, create, !create) {
        panic!("Failed to switch to {}", branch);
    }
    eprintln!("Switched to {}", branch);
    if switch_type == WithStash {
        if apply_wip_stash(&target_wt) {
            eprintln!("Applied WIP changes for {}", branch);
        } else {
            eprintln!("No WIP changes for {} to restore", branch);
        }
    }
    match (self_wt.state, target_wt) {
        (
            WorktreeState::CommittedBranch {
                branch: old_branch, ..
            }
            | WorktreeState::UncommittedBranch { branch: old_branch },
            WorktreeState::CommittedBranch {
                branch: target_branch,
                ..
            }
            | WorktreeState::UncommittedBranch {
                branch: target_branch,
            },
        ) if create => {
            set_target(&target_branch, &BranchName::Local(old_branch))
                .expect("Could not set target branch.");
        }
        _ => (),
    };
    Ok(())
}

pub fn set_target(branch: &LocalBranchName, target: &BranchName) -> Result<(), ConfigErr> {
    let name = target_branch_setting(branch);
    set_setting(SettingLocation::Local, &name, &target.full())
}

fn join_lines(lines: &[String]) -> String {
    lines
        .iter()
        .map(|s| s.to_string() + "\n")
        .collect::<Vec<String>>()
        .join("")
}

pub fn append_lines<T: IntoIterator<Item = String>>(string: String, new_lines: T) -> String {
    let mut lines: Vec<String> = string.lines().map(|s| s.to_string()).collect();
    lines.extend(new_lines);
    join_lines(&lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relative_path() {
        assert_eq!(relative_path("foo", "foo/bar"), Ok(PathBuf::from("bar")));
        assert_eq!(relative_path("foo/bar", "foo"), Ok(PathBuf::from("..")));
        assert_eq!(
            relative_path("foo/bar", "foo/baz"),
            Ok(PathBuf::from("../baz"))
        );
        assert_eq!(
            relative_path("/foo/bar", "/foo/baz"),
            Ok(PathBuf::from("../baz"))
        );
        assert!(matches!(relative_path("/foo/bar", "foo/baz"), Err(..)));
        assert!(matches!(relative_path("foo/bar", "/foo/baz"), Err(..)));
    }

    #[test]
    fn test_parse_worktree_list() {
        let wt_list = &parse_worktree_list(concat!(
            "worktree /home/user/git/repo\n",
            "HEAD a5abe4af040eb3204fe77e16cbe6f5c7042836aa\n",
            "branch refs/heads/add-four\n\n",
            "worktree /home/user/git/wt\n",
            "HEAD a5abe4af040eb3204fe77e16cbe6f5c7042836aa\n",
            "detached\n\n",
        ));
        assert_eq!(
            wt_list[0],
            WorktreeListEntry {
                path: "/home/user/git/repo".to_string(),
                state: WorktreeState::CommittedBranch {
                    head: Commit {
                        sha: "a5abe4af040eb3204fe77e16cbe6f5c7042836aa".to_string()
                    },
                    branch: LocalBranchName {
                        name: "add-four".to_string()
                    }
                },
            }
        );
        assert_eq!(
            wt_list[1],
            WorktreeListEntry {
                path: "/home/user/git/wt".to_string(),
                state: WorktreeState::DetachedHead {
                    head: Commit {
                        sha: "a5abe4af040eb3204fe77e16cbe6f5c7042836aa".to_string()
                    },
                },
            }
        )
    }
    #[test]
    fn test_parse_worktree_list_no_commit() {
        let wt_list = &parse_worktree_list(concat!(
            "worktree /home/abentley/sandbox/asdf2\n",
            "HEAD 0000000000000000000000000000000000000000\n",
            "branch refs/heads/master\n\n",
        ));
        assert_eq!(
            wt_list[0],
            WorktreeListEntry {
                path: "/home/abentley/sandbox/asdf2".to_string(),
                state: WorktreeState::UncommittedBranch {
                    branch: LocalBranchName {
                        name: "master".to_string()
                    }
                },
            }
        )
    }
    #[test]
    fn test_join_lines() {
        let lines = vec!["hello".to_string(), "there".to_string()];
        let string = join_lines(&lines);
        assert_eq!("hello\nthere\n".to_string(), string)
    }

    #[test]
    fn test_append_lines() {
        let contents = "a\nb".to_string();
        let contents2 = append_lines(contents, vec!["c".to_string()]);
        assert_eq!(contents2, "a\nb\nc\n");
    }

    #[test]
    fn test_append_lines_no_terminator() {
        let contents = "a\nb\n".to_string();
        let contents2 = append_lines(contents, vec!["c".to_string()]);
        assert_eq!(contents2, "a\nb\nc\n");
    }

    #[test]
    fn test_parse_unmerged_state() {
        assert_eq!(
            "UU".parse::<UnmergedState>().unwrap(),
            UnmergedState::BothModified
        );
        assert_eq!(
            "AA".parse::<UnmergedState>().unwrap(),
            UnmergedState::Added(Changer::Both)
        );
        assert_eq!(
            "DD".parse::<UnmergedState>().unwrap(),
            UnmergedState::Deleted(Changer::Both)
        );
        assert_eq!(
            "AU".parse::<UnmergedState>().unwrap(),
            UnmergedState::Added(Changer::Us)
        );
        assert_eq!(
            "DU".parse::<UnmergedState>().unwrap(),
            UnmergedState::Deleted(Changer::Us)
        );
        assert_eq!(
            "UA".parse::<UnmergedState>().unwrap(),
            UnmergedState::Added(Changer::Them)
        );
        assert_eq!(
            "UD".parse::<UnmergedState>().unwrap(),
            UnmergedState::Deleted(Changer::Them)
        );
    }

    #[test]
    fn test_rename_entry() {
        let mut iterator = StatusIter {
            raw_entries: concat!(
                "2 R. N... 100644 100644 100644 2bba5d1fa19e1adab8f11aee09fcc46bbb6e58e3 2bba5d1fa19e1adab8f11aee09fcc46bbb6e58e3 R100 README.dm\x00README.md"
            ).split_terminator('\0'),
        };
        assert_eq!(
            iterator.next().unwrap(),
            StatusEntry {
                state: EntryState::Renamed {
                    old_filename: "README.md",
                    staged_status: EntryLocationStatus::Renamed,
                    tree_status: EntryLocationStatus::Unmodified,
                },
                filename: "README.dm",
            }
        );
    }

    #[test]
    fn test_conflict_entry() {
        let mut iterator = StatusIter {
            raw_entries: concat!(
                "u UU N... 100755 100755 100755 100755 bcd098ed9e6b87c18c819847cab1cea07034635a",
                " 9280a3393eeb5f48f43b5d47299f88308275624e",
                " f1823404a82d732e4f6c33d7da256a563da8815a tools/btool.py"
            )
            .split_terminator('\0'),
        };
        assert_eq!(
            iterator.next().unwrap(),
            StatusEntry {
                state: EntryState::Unmerged {
                    state: UnmergedState::BothModified
                },
                filename: "tools/btool.py",
            }
        )
    }

    #[test]
    fn test_make_worktree_head_detached() {
        let info = make_worktree_head(
            ["# branch.oid hello", "# branch.head (detached)"]
                .iter()
                .map(|x| *x),
        );
        if let WorktreeHead::Attached { commit, .. } = &info {
            println!("{:?}", commit);
        }
        assert_eq!(info, WorktreeHead::Detached("hello".to_string(),));
    }
    #[test]
    fn test_make_worktree_head_attached() {
        let info = make_worktree_head(
            ["# branch.oid hello", "# branch.head main"]
                .iter()
                .map(|x| *x),
        );
        assert_eq!(
            info,
            WorktreeHead::Attached {
                commit: BranchCommit::Oid("hello".to_string()),
                head: LocalBranchName {
                    name: "main".to_string()
                },
                upstream: None,
            }
        );
    }
    #[test]
    fn test_make_worktree_head_attached_more() {
        let info = make_worktree_head(
            ["# branch.oid hello", "# branch.head main", "asdf"]
                .iter()
                .map(|x| *x),
        );
        assert_eq!(
            info,
            WorktreeHead::Attached {
                commit: BranchCommit::Oid("hello".to_string()),
                head: LocalBranchName {
                    name: "main".to_string()
                },
                upstream: None,
            }
        );
    }
    #[test]
    fn test_make_worktree_head_upstream() {
        let info = make_worktree_head(
            [
                "# branch.oid hello",
                "# branch.head main",
                "# branch.upstream origin/main",
                "# branch.ab +25 -30",
            ]
            .iter()
            .map(|x| *x),
        );
        assert_eq!(
            info,
            WorktreeHead::Attached {
                commit: BranchCommit::Oid("hello".to_string()),
                head: LocalBranchName {
                    name: "main".to_string()
                },
                upstream: Some(UpstreamInfo {
                    name: "origin/main".to_string(),
                    added: 25,
                    removed: 30,
                }),
            }
        );
    }
}
