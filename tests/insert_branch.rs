use git2::Repository;

use oaf::branch::{PipeNext, PipePrev, SiblingBranch};
use oaf::git::{LocalBranchName, ReferenceSpec};

mod common;

#[test]
fn insert_next() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = PipeNext::from(LocalBranchName::from("foo".to_string()));
    let bar = LocalBranchName::from("bar".to_string());
    let (foo, bar) = foo.insert_branch(&repo, bar).unwrap();
    assert!(foo.find_reference(&repo).is_ok());
    assert!(foo.inverse().find_reference(&repo).is_err());
    assert!(bar.find_reference(&repo).is_ok());
    assert!(bar.inverse().find_reference(&repo).is_err());
}

#[test]
fn insert_prev() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = PipePrev::from(LocalBranchName::from("foo".to_string()));
    let bar = LocalBranchName::from("bar".to_string());
    let (bar, foo) = foo.insert_branch(&repo, bar).unwrap();
    assert!(foo.find_reference(&repo).is_ok());
    assert!(foo.inverse().find_reference(&repo).is_err());
    assert!(bar.find_reference(&repo).is_ok());
    assert!(bar.inverse().find_reference(&repo).is_err());
}
