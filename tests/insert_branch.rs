use git2::{Reference, Repository};

use oaf::branch::{PipeNext, PipePrev, SiblingBranch};
use oaf::git::{LocalBranchName, ReferenceSpec};

mod common;

fn find_sibling<'repo, T: From<LocalBranchName> + ReferenceSpec>(
    branch: &LocalBranchName,
    repo: &'repo Repository,
) -> Result<Reference<'repo>, git2::Error> {
    T::from(branch.clone()).find_reference(&repo)
}

#[test]
fn insert_next() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = LocalBranchName::from("foo".to_string());
    let bar = LocalBranchName::from("bar".to_string());
    PipeNext::insert_branch(&repo, &foo, &bar).unwrap();
    assert!(find_sibling::<PipeNext>(&foo, &repo).is_ok());
    assert!(find_sibling::<PipePrev>(&foo, &repo).is_err());
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_ok());
}

#[test]
fn insert_prev() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = LocalBranchName::from("foo".to_string());
    let bar = LocalBranchName::from("bar".to_string());
    PipePrev::insert_branch(&repo, &foo, &bar).unwrap();
    assert!(find_sibling::<PipeNext>(&foo, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&foo, &repo).is_ok());
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_ok());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_err());
}
