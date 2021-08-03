# Nit, a nicer Git

Nit is a Git client that brings a more user-friendly CLI to Git.

It's based on the following ideas:

* Merge is used to apply the changes introduced in another branch to the
  current branch.
* Merges are a good thing that should be encouraged.  They prevent conflicts
  from happening later, by establishing a clear precedence between two sets of
  changes.
* Merges can introduce logical conflicts, so the user should have the
  opportunity to test a merge before committing it.
* The commits done in a branch are different from the commits merged into a
  branch.
* Push and Pull are operations to synchronize two copies of *the same branch*.
* `--` is used to separate options from inputs that might *look* like options,
   not to separate file inputs from other kinds of inputs.
* Users may want to work on several branches, so supporting that workflow is
  helpful.
* Users should have the opportunity to see what changes would be introduced by
  merging their current work into its eventual target.
* It is usually a bad idea to commit a version of the code that has never
  existed on disk.
* The Git index (staging area/cache) is an implementation detail that is not
  useful to most users.
* History is valuable, and should usually be preserved.  It allows
  later coders to understand the precise context in which a change was
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
* `merge` defaults to `merge --no-ff --no-commit`.  `--no-commit` gives the user
  a chance to review and test the result of the merge before committing it.
  `--no-ff` is because `merge` and `pull` are distinct.
* `pull` uses `--ff-only` because synchronizing two copies of a branch should
  not typically require a merge.
* `log` shows only commits from the current branch (and its ancestors) by
  default.
* For a branch that has never been pushed before, `push` will automatically
  push to `origin` with the current branch's name.
* `switch` allows you to pick up where you left off, without committing or
  explicitly stashing your pending changes.
* `commit` defaults to `-a` because the Git index isn't useful to most.  To
  commit only some changes, consider using `nit stash [-p]` to temporarily
  remove unwanted changes.  This gives you an opportunity to test that version
  before committing it.
* `diff` defaults to HEAD for its source because the Git index isn't useful to
  most.  It provides source and target as plugins because `--` should not be
  used to separate filenames from other inputs.  It defaults to patience diff
  to prefer contiguous matches over longer, broken-up matches.
* `restore` defaults to HEAD for its source because the Git index isn't useful to most.

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
Nit is a front-end for Git, so all of its operations on repositories are
performed by invoking Git commands.  Everything it does could be accomplished
by a series of Git commands, meaning everything is completely compatible with
Git.

The use of `merge` improves mechanical interoperability, but may cause friction
with some Git users and tools.  Few would disagree that the changes introduced
on a branch are special in the context of that branch, but some do not wish to
use the first-parent mechanism to distinguish between branch commits and
merged-in commits.  Because of this, they consider all merges to hamper
readability.

Since maintaining first-parent ancestry is not a priority, they may mess it up
through fast-forward "merges", especially
[foxtrot](https://blog.developer.atlassian.com/stop-foxtrots-now/) "merges".

Note that using `rebase` in place of `merge` can also hamper interoperability,
so this a catch-22, but one that Git users have long accepted.

# Installation
Nit is in its early days, and so requires installing from source.  It is
written in the Rust language, so you'll want a copy of the Rust toolchain.
Download the source and run `cargo build --release`.  The resulting binary will
be stored as `target/release/nit`.

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
