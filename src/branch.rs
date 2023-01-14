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

enum RefErr {
    NotFound(Error),
    NotBranch,
    NotUtf8,
    Other(Error),
}

impl From<Error> for RefErr {
    fn from(err: Error) -> RefErr {
        if err.class() == ErrorClass::Reference && err.code() == ErrorCode::NotFound {
            return RefErr::NotFound(err);
        }
        RefErr::Other(err)
    }
}

fn resolve_symbolic_reference(
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

impl PipeNext {
    pub fn resolve_symbolic(&self, repo: &Repository) -> Result<String, String> {
        match resolve_symbolic_reference(repo, self) {
            Ok(target) => Ok(target),
            Err(RefErr::NotFound(_)) => Err("No next branch.".into()),
            Err(RefErr::NotBranch) => Err("Next entry is not a branch.".into()),
            Err(RefErr::NotUtf8) => Err("Next entry is not valid utf-8.".into()),
            Err(RefErr::Other(err)) => Err(err.message().into()),
        }
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
