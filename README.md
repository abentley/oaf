# Nit, a nicer Git

Nit is a Git client that brings a more user-friendly CLI to Git.

It's based on the following ideas:

1.  Merge is used to apply the changes introduced in another branch to the
    current branch.
2.  The commits done in a branch are different from the commits merged into a
    branch.
3.  Merges are a good thing that should be encouraged.  They prevent conflicts
    from happening later, by establishing a clear precedence between two sets of
    changes.
4.  Merges can introduce logical conflicts, so the user should have the
    opportunity to test a merge before committing it.
5.  Push and Pull are operations to synchronize two copies of *the same branch*.
6.  `--` is used to separate options from inputs that might *look* like options,
    not to separate file inputs from other kinds of inputs.
7.  Working on several branches at once should be as easy as possible.
8.  Users should have the opportunity to see what changes would be introduced by
    merging their current work into its eventual target.
9.  It is usually a bad idea to commit a version of the code that has never
    existed on disk.
10. The Git index (staging area/cache) is an implementation detail that is not
    useful to most users.
11. History is valuable, and should usually be preserved.  It allows
    later readers to understand the precise context in which a change was
    introduced.

# Differences from Git
## New commands
* `merge-diff` command to show what would happen if you committed and and
  merged your changes into a branch.
* `cat` command to retrieve old versions of files.
* `fake-merge` to pretend to merge a branch, while actually making no changes
  to your local contents.

### New commands as Git external commands
All new commands can also be used as Git external commands, as long as the nit
binary can be accessed via that name prefixed with 'git-'.  e.g. by running `ln
-s ~/.local/bin/nit ~/.local/bin/git-merge-diff` you can then run
`git merge-diff`.  (This assumes that ~/.local/bin is in your path, and you have nit installed there.)

## Commands with changed behaviour
* `merge` defaults to `merge --no-ff --no-commit`.  `--no-commit` (1.).
  `--no-ff` is because `merge` and `pull` are distinct.
* `pull` uses `--ff-only` (5.).
* `log` shows only commits from the current branch (and its ancestors) by
  default. (2.)
* For a branch that has never been pushed before, `push` will automatically
  push to `origin` with the current branch's name.
* `switch` allows you to pick up where you left off, without committing or
  explicitly stashing your pending changes. (7.)
* `commit` defaults to `-a` (10.).  To commit only some changes, consider using
  `nit stash [-p]` to temporarily remove unwanted changes.  This gives you an
  opportunity to test that version before committing it (8.).
* `diff` defaults to HEAD for its source (10.).  It provides source and target
  as options (6.).  It defaults to patience diff to prefer contiguous matches
  over longer, broken-up matches.
* `restore` defaults to HEAD for its source (10.).

Note: if you just want the new commands, not the changed behaviour, see "New
commands as Git external commands" above.

## Obsolete commands
* `checkout` is superseded by `switch` or `restore`.

## Unchanged commands
All commands not listed by `nit help` will automatically fall through to `git`.
So `nit write-tree -h` is the same as `git write-tree -h`.

# Extensions
Because `nit` falls through to `git`, `nit` will also fall through to external
git commands.  So `git-lfs` can also be invoked as `nit lfs`.  Currently, Nit
does not have native support for extension.

# Interoperability
## File-format compatibility
Nit is a front-end for Git, so all of its operations on repositories are
performed by invoking Git commands.  Everything it does could be accomplished
by a series of Git commands, meaning everything is completely compatible with
Git.

## Interchange with other users
The use of `merge` improves mechanical interoperability, but may cause friction
with some Git users and tools.  Most developers would agree that the changes
introduced on a branch are special in the context of that branch, but some do
not wish to use the first-parent mechanism to distinguish between branch
commits and merged-in commits.  Because of this, they consider all merges to
hamper readability.

Since maintaining first-parent ancestry is not a priority, they may mess it up
through fast-forward "merges", especially
[foxtrot](https://blog.developer.atlassian.com/stop-foxtrots-now/) "merges".

Note that using `rebase` in place of `merge` can also hamper interoperability,
so this a catch-22, but one that Git users have long accepted.

# Installation
Nit is in its early days, so binaries are provided for only Ubuntu 20.04 for
x86-64.

It is written in the Rust language, so you'll need a copy of the Rust
toolchain to install from source.  Download the source and run `cargo build
--release`.  The resulting binary will be stored as `target/release/nit`.

Git must be installed for Nit to function.  Nit is typically tested with Git 2.25.x

# History
Nit draws some inspiration from my previous work on

* [Bazaar](https://bazaar.canonical.com/en/) VCS
* the [bzrtools](http://wiki.bazaar.canonical.com/BzrTools) plugins
* Fai, the Friendly [Arch](https://www.gnu.org/software/gnu-arch/) Interface
* aba, an Arch I wrote in shell to add support for Git-style external
  commands.

While the Git repository format won out over Bazaar, many concepts from the
Bazaar user model can be applied to Git.  Nit is my attempt to begin to do
that.  There is also [Breezy](https://www.breezy-vcs.org/), which is a fork of
Bazaar with Git support built-in.
