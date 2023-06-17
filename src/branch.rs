// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::git::{
    get_settings, resolve_refname, BranchName, LocalBranchName, RefErr, ReferenceSpec,
    SettingEntry, SettingTarget, UnparsedReference,
};
use super::worktree::{target_branch_setting, Commit, Commitish, ExtantRefName};
use git2::{Error, ErrorClass, ErrorCode, Reference, Repository};
use std::borrow::Cow;
use std::fmt;
use std::fmt::{Display, Formatter};

pub struct PrevRefErr(RefErr);

impl Display for PrevRefErr {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(
            formatter,
            "{}",
            match &self.0 {
                RefErr::NotFound(_) => "No previous branch.",
                RefErr::NotBranch => "Previous entry is not a branch.",
                RefErr::NotUtf8 => "Previous entry is not valid utf-8.",
                RefErr::Other(err) => return err.fmt(formatter),
            }
        )
    }
}

pub struct NextRefErr(pub RefErr);

impl Display for NextRefErr {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(
            formatter,
            "{}",
            match &self.0 {
                RefErr::NotFound(_) => "No next branch.",
                RefErr::NotBranch => "Next entry is not a branch.",
                RefErr::NotUtf8 => "Next entry is not valid utf-8.",
                RefErr::Other(err) => return err.fmt(formatter),
            }
        )
    }
}

impl From<Error> for RefErr {
    fn from(err: Error) -> RefErr {
        if err.class() == ErrorClass::Reference && err.code() == ErrorCode::NotFound {
            return RefErr::NotFound(err);
        }
        RefErr::Other(err)
    }
}

pub fn resolve_symbolic_reference(
    repo: &Repository,
    next_ref: &impl ReferenceSpec,
) -> Result<UnparsedReference, RefErr> {
    let target_ref = repo.find_reference(&next_ref.full())?;
    let target_bytes = target_ref
        .symbolic_target_bytes()
        .ok_or(RefErr::NotBranch)?;
    String::from_utf8(target_bytes.to_owned())
        .map_err(|_| RefErr::NotUtf8)
        .map(|name| UnparsedReference { name })
}

pub trait SiblingBranch: From<LocalBranchName> + ReferenceSpec {
    type BranchError: From<RefErr>;
    type Inverse: SiblingBranch;
    fn inverse(self) -> Self::Inverse;
    fn name(&self) -> &LocalBranchName;
    fn check_link<'repo>(
        &self,
        repo: &'repo Repository,
        new: LocalBranchName,
    ) -> Result<CheckedBranchLinks, LinkFailure<'repo>>;
    fn insert_branch(
        self,
        repo: &Repository,
        new: LocalBranchName,
    ) -> Result<(PipeNext, PipePrev), LinkFailure> {
        self.check_link(repo, new)?.link(repo)
    }
}

impl From<RefErr> for NextRefErr {
    fn from(err: RefErr) -> NextRefErr {
        NextRefErr(err)
    }
}
impl From<RefErr> for PrevRefErr {
    fn from(err: RefErr) -> PrevRefErr {
        PrevRefErr(err)
    }
}

impl PipeNext {
    /**
     * Given a branch name of the format "foo-5", produce the next number in the sequence, e.g.
     * "foo-6".  Given any other branch name, append "-1" to it.
     **/
    pub fn make_name(mut current_name: String) -> String {
        let (num, prefix_len) = current_name
            .rsplit_once('-')
            .and_then(|(stub, num_str)| {
                num_str
                    .parse::<u32>()
                    .ok()
                    .map(|n| (n, stub.len() + "-".len()))
            })
            .unwrap_or_else(|| {
                current_name.push('-');
                (1, current_name.len())
            });
        current_name.truncate(prefix_len);
        current_name.push_str(&(num + 1).to_string());
        current_name
    }
}

impl SiblingBranch for PipeNext {
    type BranchError = NextRefErr;
    type Inverse = PipePrev;
    fn inverse(self) -> Self::Inverse {
        Self::Inverse::from(self.name)
    }
    fn check_link<'repo>(
        &self,
        repo: &'repo Repository,
        new: LocalBranchName,
    ) -> Result<CheckedBranchLinks, LinkFailure<'repo>> {
        check_link_branches(repo, self.clone(), new.into())
    }
    fn name(&self) -> &LocalBranchName {
        &self.name
    }
}

#[derive(Clone, Debug)]
pub struct PipeNext {
    pub name: LocalBranchName,
}

impl From<LocalBranchName> for PipeNext {
    fn from(name: LocalBranchName) -> PipeNext {
        Self { name }
    }
}

impl ReferenceSpec for PipeNext {
    fn full(&self) -> Cow<str> {
        format!("refs/pipe-next/{}", self.name.branch_name()).into()
    }
}

#[derive(Clone, Debug)]
pub struct PipePrev {
    pub name: LocalBranchName,
}

impl SiblingBranch for PipePrev {
    type BranchError = PrevRefErr;
    type Inverse = PipeNext;
    fn inverse(self) -> Self::Inverse {
        Self::Inverse::from(self.name)
    }
    fn check_link<'repo>(
        &self,
        repo: &'repo Repository,
        new: LocalBranchName,
    ) -> Result<CheckedBranchLinks, LinkFailure<'repo>> {
        check_link_branches(repo, new.into(), self.clone())
    }

    fn name(&self) -> &LocalBranchName {
        &self.name
    }
}

impl From<LocalBranchName> for PipePrev {
    fn from(name: LocalBranchName) -> PipePrev {
        PipePrev { name }
    }
}

impl ReferenceSpec for PipePrev {
    fn full(&self) -> Cow<str> {
        format!("refs/pipe-prev/{}", self.name.branch_name()).into()
    }
}

/**
 * If a branch is local, convert it to its remote form, using the supplied remote (if any).
 * Note: this is *not* using the own branch's "remote" setting, so it's arguably incorrect.
 * As well as the risk of converting a valid local branch to an invalid (or stale) remote branch
 * there's the risk of converting an newer branch into an older one.
 */
pub fn remotify(branch: BranchName, remote: Option<String>) -> BranchName {
    let x = (remote, branch);
    let (Some(remote), BranchName::Local(local_branch)) = x else {
        return x.1
    };
    local_branch.with_remote(remote).into()
}

pub struct BranchAndCommit {
    name: BranchName,
    commit: Commit,
}

impl BranchAndCommit {
    pub fn factory(name: BranchName, commit: Commit) -> Self {
        Self { name, commit }
    }
    pub fn resolve(name: BranchName) -> Option<Self> {
        let Some((_, sha)) = resolve_refname(&name.full()) else {
            return None
        };
        Some(Self {
            name,
            commit: Commit { sha },
        })
    }
    pub fn extract_branch_name(self) -> BranchName {
        self.name
    }
}

fn select_latest(first: BranchAndCommit, second: BranchName) -> BranchAndCommit {
    let Some(second) = BranchAndCommit::resolve(second) else {
        return first
    };
    let base = first.commit.find_merge_base(&second.commit);
    if base == second.commit {
        first
    } else {
        second
    }
}

pub fn find_target_branchname(
    branch_name: LocalBranchName,
) -> Result<Option<BranchName>, UnparsedReference> {
    let target_setting = target_branch_setting(&branch_name);
    let remote_setting = branch_name.setting_name("remote");
    let mut remote = None;
    let mut target_branch = None;
    for entry in get_settings(&branch_name, &["oaf-target-branch", "remote"]) {
        if let SettingEntry::Valid { key, value } = entry {
            if target_setting.matches(&key) {
                target_branch = Some(value);
            } else if *key == remote_setting {
                remote = Some(value);
            }
        }
    }
    let Some(target_branch) = target_branch else {
        return Ok(None);
    };
    let Some(refname) = ExtantRefName::resolve(&target_branch) else {
        eprintln!("Remembered branch {} does not exist", target_branch);
        return Ok(None);
    };
    let refname = BranchAndCommit::try_from(refname)?;
    let remote_target_branch = remotify(refname.name.clone(), remote);
    if ExtantRefName::resolve(&remote_target_branch.full()).is_none() {
        return Ok(Some(refname.name));
    }
    Ok(Some(
        select_latest(refname, remote_target_branch).extract_branch_name(),
    ))
}

#[derive(Debug, PartialEq)]
pub enum LinkFailure<'repo> {
    BranchValidationError(BranchValidationError<'repo>),
    PrevReferenceExists,
    NextReferenceExists,
    SameReference,
    Git2Error(git2::Error),
}

impl From<git2::Error> for LinkFailure<'_> {
    fn from(err: git2::Error) -> LinkFailure<'static> {
        LinkFailure::Git2Error(err)
    }
}

impl Display for LinkFailure<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(
            formatter,
            "{}",
            match &self {
                LinkFailure::BranchValidationError(err) => {
                    return write!(formatter, "{:?}", err);
                }
                LinkFailure::PrevReferenceExists => "Previous reference exists",
                LinkFailure::NextReferenceExists => "NextReferenceExists",
                LinkFailure::SameReference => "Previous and next are the same.",
                LinkFailure::Git2Error(err) => return err.fmt(formatter),
            }
        )
    }
}

#[derive(PartialEq)]
pub enum BranchValidationError<'repo> {
    NotLocalBranch(&'repo Reference<'repo>),
    NotUtf8(&'repo Reference<'repo>),
}

impl fmt::Debug for BranchValidationError<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match &self {
            BranchValidationError::NotLocalBranch(_) => write!(formatter, "Not local branch"),
            BranchValidationError::NotUtf8(_) => write!(formatter, "Not UTF-8"),
        }
    }
}

impl<'repo> From<BranchValidationError<'repo>> for LinkFailure<'repo> {
    fn from(err: BranchValidationError<'repo>) -> LinkFailure<'repo> {
        LinkFailure::BranchValidationError(err)
    }
}

impl<'repo> TryFrom<&'repo Reference<'repo>> for LocalBranchName {
    type Error = BranchValidationError<'repo>;
    fn try_from(reference: &'repo Reference) -> Result<Self, BranchValidationError<'repo>> {
        if !reference.is_branch() {
            return Err(BranchValidationError::NotLocalBranch(reference));
        }

        reference
            .shorthand()
            .ok_or(BranchValidationError::NotUtf8(reference))
            .map(|n| Ok(LocalBranchName::from(n.to_owned())))?
    }
}

pub struct CheckedBranchLinks {
    next_reference: PipeNext,
    prev_reference: PipePrev,
}

/**
 * Check whether it is safe to link two branches in a pipeline.
 * This fails if the links exist, or if the branches are the same.
 * On success, return the next and previous branches as (PipeNext / PipePrev).
 */
pub fn check_link_branches(
    repo: &Repository,
    next_reference: PipeNext,
    prev_reference: PipePrev,
) -> Result<CheckedBranchLinks, LinkFailure> {
    if prev_reference.name() == next_reference.name() {
        return Err(LinkFailure::SameReference);
    }
    if repo.find_reference(&prev_reference.full()).is_ok() {
        return Err(LinkFailure::PrevReferenceExists);
    }
    if repo.find_reference(&next_reference.full()).is_ok() {
        return Err(LinkFailure::NextReferenceExists);
    }
    Ok(CheckedBranchLinks {
        next_reference,
        prev_reference,
    })
}

impl CheckedBranchLinks {
    pub fn link(self, repo: &Repository) -> Result<(PipeNext, PipePrev), LinkFailure<'_>> {
        repo.reference_symbolic(
            &self.next_reference.full(),
            &self.prev_reference.name().full(),
            false,
            "Connecting branches",
        )?;
        repo.reference_symbolic(
            &self.prev_reference.full(),
            &self.next_reference.name.full(),
            false,
            "Connecting branches",
        )?;
        Ok((self.next_reference, self.prev_reference))
    }
}

fn unlink_siblings<T: SiblingBranch>(repo: &Repository, next: T) -> Option<LocalBranchName> {
    let mut next_reference = next.find_reference(repo).ok()?;
    let next_target = next_reference.symbolic_target();
    let resolved = next_target.expect("Next link is not utf-8 symbolic");
    let next_branch = LocalBranchName::from_long(resolved.to_string(), None).unwrap();
    let back_sibling: <T as SiblingBranch>::Inverse = next_branch.clone().into();
    back_sibling
        .find_reference(repo)
        .expect("Back reference is missing")
        .delete()
        .unwrap();
    next_reference.delete().unwrap();
    Some(next_branch)
}

#[derive(Debug)]
pub enum UnlinkBranchError {
    NoSuchBranch,
}

pub fn unlink_branch(repo: &Repository, branch: &LocalBranchName) -> Result<(), UnlinkBranchError> {
    let next = unlink_siblings(repo, PipeNext::from(branch.clone()));
    let prev = unlink_siblings(repo, PipePrev::from(branch.clone()));
    if next.is_none() && prev.is_none() && ExtantRefName::resolve(&branch.full()).is_none() {
        return Err(UnlinkBranchError::NoSuchBranch);
    }
    if let Some((next, prev)) = next.zip(prev) {
        check_link_branches(repo, prev.into(), PipePrev::from(next))
            .expect("Could not re-link branches.")
            .link(repo)
            .expect("Could not re-link branches.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_branch_setting() {
        assert_eq!(
            target_branch_setting(&LocalBranchName::from("my-branch".to_string()))
                .to_setting_string(),
            "branch.my-branch.oaf-target-branch"
        );
    }
    #[test]
    fn test_make_name() {
        assert_eq!(PipeNext::make_name("bar/foo-2".to_string()), "bar/foo-3");
        assert_eq!(PipeNext::make_name("bar/foo".to_string()), "bar/foo-2");
        assert_eq!(PipeNext::make_name("bar/foo-a".to_string()), "bar/foo-a-2");
    }
}
