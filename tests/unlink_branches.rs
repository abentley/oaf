use git2::{Reference, Repository};

use oaf::branch::{
    check_link_branches, resolve_symbolic_reference, unlink_branch, PipeNext, PipePrev,
};
use oaf::git::{LocalBranchName, ReferenceSpec};

mod common;

fn find_sibling<'repo, T: From<LocalBranchName> + ReferenceSpec>(
    branch: &LocalBranchName,
    repo: &'repo Repository,
) -> Result<Reference<'repo>, git2::Error> {
    T::from(branch.clone()).find_reference(&repo)
}

fn make_two_pipeline(repo: &Repository) -> (LocalBranchName, LocalBranchName) {
    let foo = LocalBranchName::from("foo".to_string());
    let bar = LocalBranchName::from("bar".to_string());
    check_link_branches(&repo, foo.clone().into(), bar.clone().into())
        .unwrap()
        .link(&repo)
        .unwrap();
    (foo, bar)
}

fn make_three_pipeline(repo: &Repository) -> (LocalBranchName, LocalBranchName, LocalBranchName) {
    let (foo, bar) = make_two_pipeline(repo);
    let baz = LocalBranchName::from("baz".to_string());
    check_link_branches(&repo, bar.clone().into(), baz.clone().into())
        .unwrap()
        .link(&repo)
        .unwrap();
    (foo, bar, baz)
}

#[test]
fn unlink_two_first() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let (foo, bar) = make_two_pipeline(&repo);
    unlink_branch(&repo, &foo);
    assert!(find_sibling::<PipeNext>(&foo, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&foo, &repo).is_err());
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_err());
}

#[test]
fn unlink_two_last() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let (foo, bar) = make_two_pipeline(&repo);
    unlink_branch(&repo, &bar);
    assert!(find_sibling::<PipeNext>(&foo, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&foo, &repo).is_err());
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_err());
}

#[test]
fn unlink_three_first() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let (foo, bar, baz) = make_three_pipeline(&repo);
    unlink_branch(&repo, &foo);
    assert!(find_sibling::<PipeNext>(&foo, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&foo, &repo).is_err());
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_ok());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_err());
    assert!(find_sibling::<PipeNext>(&baz, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&baz, &repo).is_ok());
}

#[test]
fn unlink_three_last() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let (foo, bar, baz) = make_three_pipeline(&repo);
    unlink_branch(&repo, &baz);
    assert!(find_sibling::<PipeNext>(&foo, &repo).is_ok());
    assert!(find_sibling::<PipePrev>(&foo, &repo).is_err());
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_ok());
    assert!(find_sibling::<PipeNext>(&baz, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&baz, &repo).is_err());
}

#[test]
fn unlink_three_middle() {
    let work_dir = common::init_blank_repo();
    let repo = Repository::open(work_dir).unwrap();
    let (foo, bar, baz) = make_three_pipeline(&repo);
    unlink_branch(&repo, &bar);
    assert!(find_sibling::<PipeNext>(&bar, &repo).is_err());
    assert!(find_sibling::<PipePrev>(&bar, &repo).is_err());
    let foo_next = resolve_symbolic_reference(&repo, &PipeNext::from(foo.clone())).unwrap();
    assert!(foo_next == baz.full());
    let baz_prev = resolve_symbolic_reference(&repo, &PipePrev::from(baz.clone())).unwrap();
    assert!(baz_prev == foo.full());
}
