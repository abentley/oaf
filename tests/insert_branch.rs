use git2::Repository;

use oaf::branch::{LinkFailure, PipeNext, PipePrev, SiblingBranch};
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

#[test]
fn insert_next_twice() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let foo = PipeNext::from(LocalBranchName::from("foo".to_string()));
    let bar = LocalBranchName::from("bar".to_string());
    let (foo, bar) = foo.insert_branch(&repo, bar).unwrap();
    let baz = LocalBranchName::from("baz".to_string());
    let (bar, baz) = bar.inverse().insert_branch(&repo, baz).unwrap();
    assert!(foo.find_reference(&repo).is_ok());
    assert!(foo.inverse().find_reference(&repo).is_err());
    assert!(bar.find_reference(&repo).is_ok());
    assert!(bar.inverse().find_reference(&repo).is_ok());
    assert!(baz.find_reference(&repo).is_ok());
    assert!(baz.inverse().find_reference(&repo).is_err());
}

#[test]
fn corrupt_insertion() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();

    let foo = PipeNext::from(LocalBranchName::from("foo".to_string()));
    let bar = LocalBranchName::from("bar".to_string());
    let (foo, _) = foo.insert_branch(&repo, bar).unwrap();

    let baz = PipeNext::from(LocalBranchName::from("baz".to_string()));
    let qux = LocalBranchName::from("qux".to_string());
    let (_, qux) = baz.insert_branch(&repo, qux).unwrap();

    // qux cannot become next to foo, because that leaves baz dangling.
    assert!(
        foo.insert_branch(&repo, qux.name().clone()).unwrap_err()
            == LinkFailure::PrevReferenceExists
    )
}
