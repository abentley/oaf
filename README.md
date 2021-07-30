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
* The Git Index (cache) is an implementation detail that is not useful to most
  users.
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

Note: all new commands can also be used as Git external commands, as long as
the nit binary can be accessed via that name prefixed with 'git-'.  e.g. by
running `ln -s ~/.local/bin/nit ~/.local/bin/git-merge-diff` you can then run
`git merge-diff`.  (This assumes that ~/.local/bin is in your path, and you have nit installed there.)

## Changed behaviour
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
* `commit` defaults to `-a` because the Git Index isn't useful to most.  To
  commit only some changes, consider using `nit stash [-p]` to temporarily
  remove unwanted changes.  This gives you an opportunity to test that version
  before committing it.

## Obsolete commands
* `checkout` is superseded by `switch` or `restore`.

## Unchanged commands
All commands not listed by `nit help` will automatically fall through to `git`.
So `nit write-tree -h` is the same as `git write-tree -h`.

# Interoperability
Nit is a front-end for Git, so all of its operations on repositories are
performed by invoking Git commands.  Everything it does could be accomplished
by a series of Git commands.

Some Git users embrace treating the current branch's commits as special, but this is not a default in Git, resulting in:

* some animosity towards merges, since they mess up the default logs
* Git users messing up the first-parent ancestry through:
  * fast-forward "merges"
  * [foxtrot](https://blog.developer.atlassian.com/stop-foxtrots-now/) "merges"

Users interoperating with Git users may wish to reduce their use of `merge`.
Note that using `rebase` in place of `merge` can also hamper interoperability,
so this a catch-22, but one that Git users have long accepted.

# History
Nit draws some inspiration from my previous work on the
[Bazaar](https://bazaar.canonical.com/en/) VCS and
[bzrtools](http://wiki.bazaar.canonical.com/BzrTools) plugins.  While the Git
repository format won out over Bazaar, many concepts from the Bazaar user model
can be applied to Git.  Maybe one day we'll get robust rename support :-).  Or
empty directory support.  A boy can dream.
