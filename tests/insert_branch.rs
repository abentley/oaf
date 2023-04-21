use git2::Repository;

use oaf::branch::{PipeNext, PipePrev, SiblingBranch};
use oaf::git::{LocalBranchName, ReferenceSpec};

mod common;

#[test]
fn insert_next() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = LocalBranchName::from("foo".to_string());
    let bar = LocalBranchName::from("bar".to_string());
    let (foo, bar) = PipeNext::from(foo.clone())
        .insert_branch(&repo, bar.clone())
        .unwrap();
    assert!(foo.find_reference(&repo).is_ok());
    assert!(foo.inverse().find_reference(&repo).is_err());
    assert!(bar.find_reference(&repo).is_ok());
    assert!(bar.inverse().find_reference(&repo).is_err());
}

#[test]
fn insert_prev() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = LocalBranchName::from("foo".to_string());
    let bar = LocalBranchName::from("bar".to_string());
    let (bar, foo) = PipePrev::from(foo.clone())
        .insert_branch(&repo, bar.clone())
        .unwrap();
    assert!(foo.find_reference(&repo).is_ok());
    assert!(foo.inverse().find_reference(&repo).is_err());
    assert!(bar.find_reference(&repo).is_ok());
    assert!(bar.inverse().find_reference(&repo).is_err());
}
