// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use super::git::{LocalBranchName, ReferenceSpec};
use std::borrow::Cow;

#[derive(Debug)]
pub struct PipeNext {
    pub name: LocalBranchName,
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
