// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::git::{LocalBranchName, ReferenceSpec};
use git2::{Error, ErrorClass, ErrorCode, Repository};
use std::borrow::Cow;
use std::fmt::{Display, Formatter};

pub enum RefErr {
    NotFound(Error),
    NotBranch,
    NotUtf8,
    Other(Error),
}

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

pub struct NextRefErr(RefErr);

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
    let Some(target_bytes) = target_ref.symbolic_target_bytes() else {
        return Err(RefErr::NotBranch);
    };
    let Ok(target) = String::from_utf8(target_bytes.to_owned()) else {
        return Err(RefErr::NotUtf8);
    };
    Ok(target)
}

#[derive(Debug)]
pub struct PipeNext {
    pub name: LocalBranchName,
}

pub trait SiblingBranch {
    type BranchError;
    fn wrap(err: RefErr) -> Self::BranchError;
}

impl SiblingBranch for PipeNext {
    type BranchError = NextRefErr;
    fn wrap(err: RefErr) -> NextRefErr {
        NextRefErr(err)
    }
}

impl From<LocalBranchName> for PipeNext {
    fn from(name: LocalBranchName) -> PipeNext {
        PipeNext { name }
    }
}

impl ReferenceSpec for PipeNext {
    fn full(&self) -> Cow<str> {
        format!("refs/pipe-next/{}", self.name.short()).into()
    }
    fn short(&self) -> Cow<str> {
        self.full()
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
}

impl From<LocalBranchName> for PipePrev {
    fn from(name: LocalBranchName) -> PipePrev {
        PipePrev { name }
    }
}

impl ReferenceSpec for PipePrev {
    fn full(&self) -> Cow<str> {
        format!("refs/pipe-prev/{}", self.name.short()).into()
    }
    fn short(&self) -> Cow<str> {
        self.full()
    }
}
