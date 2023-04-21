// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::git::{
    get_settings, BranchName, LocalBranchName, RefErr, ReferenceSpec, SettingEntry, SettingTarget,
    UnparsedReference,
};
use super::worktree::{target_branch_setting, ExtantRefName};
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
) -> Result<String, RefErr> {
    let target_ref = repo.find_reference(&next_ref.full())?;
    let target_bytes = target_ref
        .symbolic_target_bytes()
        .ok_or(RefErr::NotBranch)?;
    String::from_utf8(target_bytes.to_owned()).map_err(|_| RefErr::NotUtf8)
}

pub trait SiblingBranch {
    type BranchError;
    fn wrap(err: RefErr) -> Self::BranchError;
    fn check_link<'repo, 'n>(
        repo: &'repo Repository,
        existing: &'n LocalBranchName,
        new: &'n LocalBranchName,
    ) -> Result<CheckedBranchLinks<'n>, LinkFailure<'repo>>;
    fn insert_branch<'repo, 'n>(
        repo: &'repo Repository,
        existing: &'n LocalBranchName,
        new: &'n LocalBranchName,
    ) -> Result<(), LinkFailure<'repo>> {
        Self::check_link(repo, existing, new)?.link(repo)
    }
}

impl SiblingBranch for PipeNext {
    type BranchError = NextRefErr;
    fn wrap(err: RefErr) -> NextRefErr {
        NextRefErr(err)
    }
    fn check_link<'repo, 'n>(
        repo: &'repo Repository,
        existing: &'n LocalBranchName,
        new: &'n LocalBranchName,
    ) -> Result<CheckedBranchLinks<'n>, LinkFailure<'repo>> {
        check_link_branches(repo, existing, new)
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct PipePrev {
    pub name: LocalBranchName,
}

impl SiblingBranch for PipePrev {
    type BranchError = PrevRefErr;
    fn wrap(err: RefErr) -> PrevRefErr {
        PrevRefErr(err)
    }
    fn check_link<'repo, 'n>(
        repo: &'repo Repository,
        existing: &'n LocalBranchName,
        new: &'n LocalBranchName,
    ) -> Result<CheckedBranchLinks<'n>, LinkFailure<'repo>> {
        check_link_branches(repo, new, existing)
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

fn target_from_settings(
    target_branch: String,
    remote: Option<String>,
) -> Result<BranchName, UnparsedReference> {
    let refname = ExtantRefName::resolve(&target_branch).unwrap();
    let (remote, Ok(BranchName::Local(local_branch))) = (remote, &refname.name) else {
        return refname.name;
    };
    let Some(remote) = remote else {
        return Ok(BranchName::Local(local_branch.clone()));
    };
    Ok(BranchName::Remote(local_branch.clone().with_remote(remote)))
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
    let target_branch = { target_from_settings(target_branch, remote)? };
    Ok(Some(target_branch))
}

#[derive(Debug)]
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

pub struct CheckedBranchLinks<'n> {
    prev_name: &'n LocalBranchName,
    next_name: &'n LocalBranchName,
    next_reference: PipeNext,
    prev_reference: PipePrev,
}

/**
 * Check whether it is safe to link two branches in a pipeline.
 * This fails if the links exist, or if the branches are the same.
 * On success, return the next and previous branches as (PipeNext / PipePrev).
 */
pub fn check_link_branches<'repo, 'n>(
    repo: &'repo Repository,
    prev_name: &'n LocalBranchName,
    next_name: &'n LocalBranchName,
) -> Result<CheckedBranchLinks<'n>, LinkFailure<'repo>> {
    if *prev_name == *next_name {
        return Err(LinkFailure::SameReference);
    }
    let prev_reference = PipePrev::from(next_name.clone());
    if repo.find_reference(&prev_reference.full()).is_ok() {
        return Err(LinkFailure::PrevReferenceExists);
    }
    let next_reference = PipeNext::from(prev_name.clone());
    if repo.find_reference(&next_reference.full()).is_ok() {
        return Err(LinkFailure::NextReferenceExists);
    }
    Ok(CheckedBranchLinks {
        prev_name,
        next_name,
        next_reference,
        prev_reference,
    })
}

impl CheckedBranchLinks<'_> {
    pub fn link(self, repo: &'_ Repository) -> Result<(), LinkFailure<'_>> {
        repo.reference_symbolic(
            &self.next_reference.full(),
            &self.next_name.full(),
            false,
            "Connecting branches",
        )?;
        repo.reference_symbolic(
            &self.prev_reference.full(),
            &self.prev_name.full(),
            false,
            "Connecting branches",
        )?;
        Ok(())
    }
}

fn unlink_siblings<
    T: SiblingBranch + ReferenceSpec + From<LocalBranchName>,
    U: SiblingBranch + ReferenceSpec + From<LocalBranchName>,
>(
    repo: &Repository,
    branch: &LocalBranchName,
) -> Option<LocalBranchName> {
    let next = T::from(branch.clone());
    if let Ok(mut next_reference) = next.find_reference(repo) {
        let next_target = next_reference.symbolic_target();
        let resolved = next_target.expect("Next link is not utf-8 symbolic");
        let next_branch = LocalBranchName::from_long(resolved.to_string(), None).unwrap();
        let back_sibling: U = next_branch.clone().into();
        back_sibling
            .find_reference(repo)
            .expect("Back reference is missing")
            .delete()
            .unwrap();
        next_reference.delete().unwrap();
        Some(next_branch)
    } else {
        None
    }
}

pub fn unlink_branch(repo: &Repository, branch: &LocalBranchName) {
    let next = unlink_siblings::<PipeNext, PipePrev>(repo, branch);
    let prev = unlink_siblings::<PipePrev, PipeNext>(repo, branch);
    if let (Some(next), Some(prev)) = (next, prev) {
        check_link_branches(repo, &prev, &next)
            .expect("Could not re-link branches.")
            .link(repo)
            .expect("Could not re-link branches.");
    }
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
}
