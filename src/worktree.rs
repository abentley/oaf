// Copyright 2021 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf, StripPrefixError};
use std::process::{Command, Output};
use std::str::{from_utf8, FromStr};

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

pub fn run_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Result<Output, Output> {
    let output = make_git_command(args_vec)
        .output()
        .expect("Couldn't run command");
    if !output.status.success() {
        return Err(output);
    }
    Ok(output)
}

pub fn output_to_string(output: &Output) -> String {
    from_utf8(&output.stdout)
        .expect("Output is not utf-8")
        .trim()
        .to_string()
}

pub fn make_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args_vec);
    cmd
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

pub struct GitStatus {
    outstr: String,
}

impl GitStatus {
    pub fn iter(&self) -> StatusIter {
        StatusIter {
            // Note: there is an extra entry for each rename entry, consisting of the original
            // filename.  This is an inevitable consequence of splitting using a terminator instead
            // of performing the entry iteration in StatusIter::next()
            raw_entries: self.outstr.split_terminator('\0'),
        }
    }

    pub fn new() -> GitStatus {
        let output =
            run_git_command(&["status", "--porcelain=v2", "-z"]).expect("Couldn't list directory");
        let outstr = output_to_string(&output);
        GitStatus { outstr }
    }
}
