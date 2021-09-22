// Copyright 2021 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::git::{
    create_stash, delete_ref, eval_rev_spec, full_branch, get_toplevel, git_switch,
    output_to_string, run_git_command, set_head, upsert_ref,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf, StripPrefixError};
use std::process::Output;
use std::str::FromStr;

#[derive(Debug, Clone, Copy)]
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

fn relative_path<T: AsRef<OsStr>, U: AsRef<OsStr>>(
    from: T,
    to: U,
) -> Result<PathBuf, StripPrefixError> {
    let mut result = PathBuf::from("");
    let to = Path::new(&to);
    let from = Path::new(&from);
    if from.has_root() == to.has_root() {
        for ancestor in from.ancestors() {
            if let Ok(relpath) = to.strip_prefix(ancestor) {
                result.push(relpath);
                return Ok(result);
            }
            result.push("..");
        }
    }
    return Ok(PathBuf::from(to.strip_prefix(from)?));
}

#[derive(Debug, Copy, Clone)]
pub struct StatusEntry<'a> {
    pub state: EntryState<'a>,
    pub filename: &'a str,
}

impl StatusEntry<'_> {
    pub fn format_entry<T: AsRef<OsStr>>(&self, current_dir: &T) -> String {
        let track_char = match self.state {
            EntryState::Untracked => "?",
            EntryState::Ignored => "!",
            EntryState::Changed { staged_status, .. } => match staged_status {
                EntryLocationStatus::Added => "+",
                EntryLocationStatus::Deleted => "-",
                _ => " ",
            },
            EntryState::Renamed { .. } => "R",
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

#[derive(Debug, Copy, Clone)]
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
}

/// Represents `git status` output
pub struct GitStatus {
    outstr: String,
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
    pub fn new() -> GitStatus {
        let output =
            run_git_command(&["status", "--porcelain=v2", "-z"]).expect("Couldn't list directory");
        let outstr = output_to_string(&output);
        GitStatus { outstr }
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

#[derive(Clone, Debug, PartialEq)]
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
    pub fn get_tree_reference(&self) -> String {
        format!("{}^{{tree}}", self.sha)
    }
    pub fn find_merge_base(&self, commit: &str) -> Commit {
        let output = run_git_command(&["merge-base", &self.sha, commit]);
        Commit {
            sha: output_to_string(&output.expect("Couldn't find merge base.")),
        }
    }
    pub fn set_wt_head(&self) {
        set_head(&self.sha);
    }
}

#[derive(Debug)]
pub enum CommitErr {
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

pub fn base_tree() -> String {
    match Commit::from_str("HEAD") {
        Ok(commit) => commit.get_tree_reference(),
        Err(..) => "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorktreeState {
    DetachedHead { head: Commit },
    UncommittedBranch { branch: String },
    CommittedBranch { branch: String, head: Commit },
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeListEntry {
    pub path: String,
    pub state: WorktreeState,
}

fn parse_worktree_list(lines: &str) -> Vec<WorktreeListEntry> {
    let mut line_iter = lines.lines();
    let mut result: Vec<WorktreeListEntry> = vec![];
    loop {
        let line = line_iter.next();
        let path = if let Some(line) = line {
            &line[9..]
        } else {
            break;
        };
        let line = line_iter.next().unwrap();
        let head = match &line[5..] {
            "0000000000000000000000000000000000000000" => None,
            _ => Some(Commit {
                sha: line[5..].to_string(),
            }),
        };
        let line = line_iter.next().unwrap();
        let wt_state = if &line[..6] == "branch" {
            let branch = line[7..].to_string();
            if let Some(head) = head {
                WorktreeState::CommittedBranch { branch, head }
            } else {
                WorktreeState::UncommittedBranch { branch }
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
                state: WorktreeState {
                    head: Some(Commit {
                        sha: "a5abe4af040eb3204fe77e16cbe6f5c7042836aa".to_string()
                    }),
                    branch: Some("refs/heads/add-four".to_string())
                },
            }
        );
        assert_eq!(
            wt_list[1],
            WorktreeListEntry {
                path: "/home/user/git/wt".to_string(),
                state: WorktreeState {
                    head: Some(Commit {
                        sha: "a5abe4af040eb3204fe77e16cbe6f5c7042836aa".to_string()
                    }),
                    branch: None
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
                state: WorktreeState {
                    head: None,
                    branch: Some("refs/heads/master".to_string())
                },
            }
        )
    }
}

pub fn create_wip_stash(wt: &WorktreeState) -> Option<String> {
    let current_ref = make_wip_ref(wt);
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
    let target_ref = make_wip_ref(target_wt);
    match eval_rev_spec(&target_ref) {
        Err(..) => false,
        Ok(target_oid) => {
            run_git_command(&["stash", "apply", &target_oid]).unwrap();
            delete_ref(&target_ref).unwrap();
            true
        }
    }
}

pub fn make_wip_ref(wt: &WorktreeState) -> String {
    let branch = match wt {
        WorktreeState::DetachedHead { head } => return format!("refs/commits-wip/{}", head.sha),
        WorktreeState::UncommittedBranch { branch } => branch,
        WorktreeState::CommittedBranch { branch, .. } => branch,
    };
    let splitted: Vec<&str> = branch.split("refs/heads/").collect();
    if splitted.len() != 2 {
        panic!("Branch {} does not start with refs/heads", branch);
    }
    format!("refs/branch-wip/{}", splitted[1])
}

fn check_switch_branch(top: &str, branch: &str) -> Result<WorktreeListEntry, SwitchErr> {
    let mut self_wt = None;
    let full_branch = full_branch(branch.to_string());
    for wt in list_worktree() {
        if wt.path == top {
            self_wt = Some(wt);
            continue;
        }
        let target_branch = match wt.state {
            WorktreeState::UncommittedBranch { branch } => branch,
            WorktreeState::CommittedBranch { branch, .. } => branch,
            WorktreeState::DetachedHead { .. } => continue,
        };
        if target_branch == full_branch {
            return Err(SwitchErr::BranchInUse { path: wt.path });
        }
    }
    Ok(self_wt.expect("Could not find self in worktree list."))
}

pub enum SwitchErr {
    AlreadyExists,
    NotFound,
    BranchInUse { path: String },
}

pub fn determine_switch_target(
    branch: String,
    create: bool,
    current_head: Option<&Commit>,
) -> Result<WorktreeState, SwitchErr> {
    Ok(
        if let Ok(commit_id) = eval_rev_spec(&format!("refs/heads/{}", branch)) {
            if create {
                return Err(SwitchErr::AlreadyExists);
            }
            WorktreeState::CommittedBranch {
                branch,
                head: Commit { sha: commit_id },
            }
        } else if create {
            if let Some(current_head) = current_head {
                WorktreeState::CommittedBranch {
                    branch,
                    head: current_head.to_owned(),
                }
            } else {
                WorktreeState::UncommittedBranch { branch }
            }
        } else if let Ok(commit_id) = eval_rev_spec(&format!("refs/remotes/origin/{}", branch)) {
            WorktreeState::CommittedBranch {
                branch,
                head: Commit { sha: commit_id },
            }
        } else if let Ok(commit_id) = eval_rev_spec(&branch) {
            WorktreeState::DetachedHead {
                head: Commit { sha: commit_id },
            }
        } else {
            return Err(SwitchErr::NotFound);
        },
    )
}

pub fn stash_switch(branch: &str, create: bool) -> Result<(), SwitchErr> {
    let top = get_toplevel();
    let self_wt = check_switch_branch(&top, &branch)?.state;
    let self_head = match &self_wt {
        WorktreeState::DetachedHead { head } => Some(head),
        WorktreeState::CommittedBranch { head, .. } => Some(head),
        WorktreeState::UncommittedBranch { .. } => None,
    };
    let target_wt = determine_switch_target(branch.to_string(), create, self_head)?;
    if create {
        eprintln!("Retaining any local changes.");
    } else if let Some(current_ref) = create_wip_stash(&self_wt) {
        eprintln!("Stashed WIP changes to {}", current_ref);
    } else {
        eprintln!("No changes to stash");
    }
    if let Err(..) = git_switch(&branch, create, !create) {
        panic!("Failed to switch to {}", branch);
    }
    eprintln!("Switched to {}", branch);
    if !create {
        if apply_wip_stash(&target_wt) {
            eprintln!("Applied WIP changes for {}", branch);
        } else {
            eprintln!("No WIP changes for {} to restore", branch);
        }
    }
    Ok(())
}

/// Use the commit-tree command to generate a fake-merge commit.
pub fn commit_tree(merge_parent: &Commit, tree: &Commit, message: &str) -> Result<Commit, Output> {
    let output = run_git_command(&[
        "commit-tree",
        "-p",
        "HEAD",
        "-p",
        &merge_parent.sha,
        &tree.get_tree_reference(),
        "-m",
        message,
    ])?;
    Ok(Commit {
        sha: output_to_string(&output),
    })
}
