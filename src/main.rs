use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command, Output};
use std::str::{from_utf8, FromStr};
use structopt::{clap, StructOpt};

#[derive(Debug)]
struct Commit {
    sha: String,
}

impl Commit {
    fn get_tree(self) -> String {
        output_to_string(
            &run_git_command(&["show", "--pretty=format:%T", "-q", &self.sha])
                .expect("Cannot find tree."),
        )
    }
    fn get_tree_reference(self) -> String {
        format!("{}^{{tree}}", self.sha)
    }
}

#[derive(Debug)]
enum CommitErr {
    NoCommit { spec: String },
}

impl fmt::Display for CommitErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CommitErr::NoCommit { spec } => write!(f, "No commit found for \"{}\"", spec),
        }
    }
}

impl FromStr for Commit {
    type Err = CommitErr;
    fn from_str(spec: &str) -> std::result::Result<Self, <Self as FromStr>::Err> {
        match eval_rev_spec(spec) {
            Err(..) => Err(CommitErr::NoCommit {
                spec: spec.to_string(),
            }),
            Ok(sha) => Ok(Commit { sha }),
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt {
    /// Output the contents of a file for a given tree.
    Cat {
        #[structopt(long, short, default_value = "HEAD")]
        tree: String,
        input: String,
    },
    /// Transfer local changes to a remote repository and branch.
    Push,
    /**
    Switch to a branch, stashing and restoring pending changes.

    Outstanding changes are stored as tags in the repo, with the branch's name
    suffixed with ".wip".  For example, pending changes for a branch named
    "foo" would be stored in a tag named "foo.wip".
    */
    Switch {
        /// The branch to switch to.
        branch: String,
        #[structopt(long, short)]
        create: bool,
    },
    /// Disabled to prevent accidentally discarding stashed changes.
    Checkout {
        /// The branch to switch to.
        branch_name: String,
        #[structopt(long, short)]
        branch: bool,
    },
    /// Apply the changes from another branch (or commit) to the current tree.
    Merge { source: Commit },
    /**
    Display a diff predicting the changes that would be merged if you merged your working tree.

    The diff includes uncommitted changes.  It is produced by diffing against
    the merge base of <target> and HEAD.  (nit diff <target>... does not
    include uncommitted changes)
    */
    MergeDiff {
        /// The branch you would merge into.  (Though any commitish will work.)
        target: Commit,
        path: Vec<String>,
    },
    Pull {
        remote: Option<String>,
        source: Option<String>,
    },
    /**
    Perform a fake merge of the specified branch/commit, leaving the local tree unmodified.

    This effectively gives the contents of the latest commit precedence over the contents of the
    source commit.
    */
    FakeMerge {
        /// The source for the fake merge.
        source: Commit,
        /// The message to use for the fake merge.  (Default: "Fake merge.")
        #[structopt(long, short)]
        message: Option<String>,
    },
    Commit {
        #[structopt(long, short)]
        message: Option<String>,
        /// Amend the HEAD commit.
        #[structopt(long)]
        amend: bool,
        #[structopt(long, short)]
        no_verify: bool,
        ///Commit only changes in the index.
        #[structopt(long)]
        no_all: bool,
    },
    /// Produce a log of the commit range.  By default, exclude merged commits.
    Log {
        /// The range of commits to display.  Defaults to all of HEAD.
        #[structopt(long, short)]
        range: Option<String>,
        /// If enabled, show patches for commits.
        #[structopt(long, short)]
        patch: bool,
        /// If enabled, show merged commits.  (Merge commits are always shown.)
        #[structopt(long, short)]
        include_merged: bool,
        /// Show only commits in which these files were modified.  (No filter if none supplied.)
        path: Vec<String>,
    },
}

fn cat_args(input: &str, mut tree: &str) -> Vec<String> {
    if tree == "index" {
        tree = "";
    }
    vec!["show".to_string(), format!("{}:./{}", tree, input)]
}

fn commit_args(message: Option<String>, amend: bool, no_verify: bool, no_all: bool) -> Vec<String> {
    let mut cmd_args = vec!["commit".to_string()];
    if !no_all {
        cmd_args.push("--all".to_string())
    }
    if let Some(message) = message {
        cmd_args.push("--message".to_string());
        cmd_args.push(message);
    }
    if amend {
        cmd_args.push("--amend".to_string());
    }
    if no_verify {
        cmd_args.push("--no-verify".to_string());
    }
    cmd_args
}

fn merge_args(source: Commit) -> Vec<String> {
    vec![
        "merge".to_string(),
        "--no-commit".to_string(),
        "--no-ff".to_string(),
        source.sha,
    ]
}

fn pull_args(remote: Option<String>, source: Option<String>) -> Vec<String> {
    let mut cmd_args: Vec<String> = vec!["pull".to_string(), "--ff-only".to_string()];
    if let Some(remote) = remote {
        cmd_args.push(remote);
    }
    if let Some(source) = source {
        cmd_args.push(source);
    }
    cmd_args
}

fn log_args(range: Option<String>, patch: bool, include_merged: bool,
            path: Vec<String>) -> Vec<String> {
    let mut cmd_args: Vec<String> = vec!["log".to_string()];
    if !include_merged {
        cmd_args.push("--first-parent".to_string());
    }
    if patch {
        cmd_args.push("--patch".to_string());
    }
    if let Some(range) = range {
        cmd_args.push(range);
    }
    if !path.is_empty() {
        cmd_args.push("--".to_string());
        cmd_args.extend(path)
    }
    cmd_args
}

fn output_to_string(output: &Output) -> String {
    from_utf8(&output.stdout)
        .expect("Output is not utf-8")
        .trim()
        .to_string()
}

fn run_for_string(cmd: &mut Command) -> String {
    output_to_string(&cmd.output().expect("Could not run command."))
}

fn get_current_branch() -> String {
    run_for_string(&mut make_git_command(&["branch", "--show-current"]))
}

fn branch_setting(branch: &str, setting: &str) -> String {
    format!("branch.{}.{}", branch, setting)
}

fn setting_exists(setting: &str) -> bool {
    match run_git_command(&["config", "--get", setting]) {
        Ok(..) => true,
        Err(..) => false,
    }
}

fn cmd_push() {
    let branch = get_current_branch();
    if setting_exists(&branch_setting(&branch, "remote")) {
        if !setting_exists(&branch_setting(&branch, "merge")) {
            panic!("Branch in unsupported state");
        }
        make_git_command(&["push"]).exec();
    } else {
        make_git_command(&["push", "-u", "origin", "HEAD"]).exec();
    }
}

fn create_stash() -> Option<String> {
    let oid = run_for_string(&mut make_git_command(&["stash", "create"]));
    if oid.is_empty() {
        return None;
    }
    Some(oid)
}

fn create_branch_stash() -> Option<String> {
    let current_tag = make_wip_tag(&get_current_branch());
    match create_stash() {
        Some(oid) => {
            if let Err(..) = run_git_command(&["tag", "-f", &current_tag, &oid]) {
                panic!("Failed to tag {} to {}", oid, current_tag);
            }
            Some(current_tag)
        }
        None => {
            if let Err(..) = delete_tag(&current_tag) {
                let tag_list = run_for_string(&mut make_git_command(&["tag", "-l", &current_tag]));
                if !tag_list.is_empty() {
                    panic!("Failed to delete tag {}", current_tag);
                }
            }
            None
        }
    }
}

fn run_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Result<Output, Output> {
    let output = make_git_command(args_vec)
        .output()
        .expect("Couldn't run command");
    if !output.status.success() {
        return Err(output);
    }
    Ok(output)
}

fn eval_rev_spec(rev_spec: &str) -> Result<String, Output> {
    Ok(output_to_string(&run_git_command(&[
        "rev-list", "-n1", rev_spec,
    ])?))
}

fn apply_branch_stash(target_branch: &str) -> bool {
    let target_tag = make_wip_tag(target_branch);
    match eval_rev_spec(&format!("refs/tags/{}", target_tag)) {
        Err(..) => false,
        Ok(target_oid) => {
            run_git_command(&["stash", "apply", &target_oid]).unwrap();
            delete_tag(&target_tag).unwrap();
            true
        }
    }
}

fn git_switch(target_branch: &str, create: bool, discard_changes: bool) {
    // Actual "switch" is not broadly deployed yet.
    // let mut switch_cmd = vec!["switch", "--discard-changes"];
    // --force means "discard local changes".
    let mut switch_cmd = vec!["checkout"];
    if discard_changes {
        switch_cmd.push("--force");
    }
    if create {
        if discard_changes {
            if let Err(..) = run_git_command(&["reset", "--hard"]) {
                panic!("Failed to reset tree");
            }
        }
        switch_cmd.push("-b");
    }
    switch_cmd.push(&target_branch);
    if let Err(..) = run_git_command(&switch_cmd) {
        panic!("Failed to switch to {}", target_branch);
    }
}

fn make_wip_tag(branch: &str) -> String {
    format!("{}.wip", branch)
}

fn delete_tag(tag: &str) -> Result<(), Output> {
    run_git_command(&["tag", "-d", tag])?;
    Ok(())
}

fn cmd_switch(target_branch: &str, create: bool) {
    match eval_rev_spec(&format!("refs/heads/{}", target_branch)) {
        Err(..) => {
            if !create {
                if let Err(..) = eval_rev_spec(&format!("refs/remotes/origin/{}", target_branch)) {
                    eprintln!("Branch {} not found", target_branch);
                    exit(1);
                }
            }
        }
        Ok(..) => {
            if create {
                eprintln!("Branch {} already exists", target_branch);
                exit(1);
            }
        }
    };
    if create {
        eprintln!("Retaining any local changes.");
    } else if let Some(current_tag) = create_branch_stash() {
        eprintln!("Stashed WIP changes to {}", current_tag);
    } else {
        eprintln!("No changes to stash");
    }
    git_switch(target_branch, create, !create);
    eprintln!("Switched to {}", target_branch);
    if !create {
        if apply_branch_stash(&target_branch) {
            eprintln!("Applied WIP changes for {}", target_branch);
        } else {
            eprintln!("No WIP changes for {} to restore", target_branch);
        }
    }
}

fn cmd_checkout() {
    eprintln!(
        "Please use \"switch\" to change branches or \"restore\" to restore files to a known state"
    );
}

fn cmd_merge_diff(target: &Commit, paths: Vec<String>) {
    let output = run_git_command(&["merge-base", &target.sha, "HEAD"]);
    let merge_base = output_to_string(&output.expect("Couldn't find merge base."));
    let mut diff_cmd = vec!["diff".to_string(), merge_base];
    if paths.is_empty() {
        diff_cmd.push("--".to_string());
        diff_cmd.extend(paths);
    }
    make_git_command(&diff_cmd).exec();
}

fn set_head(new_head: &str) {
    run_git_command(&["reset", "--soft", new_head]).expect("Failed to update HEAD.");
}

fn cmd_fake_merge(source: &Commit, message: &Option<String>) {
    let head = Commit::from_str("HEAD").expect("HEAD is not a commit.");
    let message = if let Some(msg) = message {
        &msg
    } else {
        "Fake merge."
    };
    let output = run_git_command(&[
        "commit-tree",
        "-p",
        "HEAD",
        "-p",
        &source.sha,
        &head.get_tree_reference(),
        "-m",
        message,
    ])
    .expect("Could not generate commit.");
    let fm_hash = output_to_string(&output);
    set_head(&fm_hash);
}

enum Args {
    NativeCommand(Opt),
    GitCommand(Vec<String>),
}

fn parse_args() -> Args {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec: Vec<String> = args_iter.collect();
    let args_vec2: Vec<String> = env::args().collect();
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "nit" => {
            if args_vec2.len() > 1 {
                let x = Opt::from_iter_safe(&args_vec2[0..2]);
                if let Err(err) = x {
                    if err.kind == clap::ErrorKind::UnknownArgument {
                        return Args::GitCommand(args_vec);
                    }
                    if err.kind == clap::ErrorKind::InvalidSubcommand {
                        return Args::GitCommand(args_vec);
                    }
                }
            }
            Opt::from_args()
        }
        _ => {
            let mut args = vec!["nit".to_string()];
            let mut subcmd_iter = progname.split('-');
            subcmd_iter.next();
            for arg in subcmd_iter {
                args.push(arg.to_string());
            }
            for arg in &args_vec {
                args.push(arg.to_string());
            }
            Opt::from_iter(args)
        }
    };
    match opt {
        Opt::Cat { input, tree } => Args::GitCommand(cat_args(&input, &tree)),
        Opt::Commit {
            message,
            amend,
            no_verify,
            no_all,
        } => Args::GitCommand(commit_args(message, amend, no_verify, no_all)),
        Opt::Merge { source } => Args::GitCommand(merge_args(source)),
        Opt::Pull { remote, source } => Args::GitCommand(pull_args(remote, source)),
        Opt::Log { range, patch, include_merged, path } => {
            Args::GitCommand(log_args(range, patch, include_merged, path))
        }
        _ => Args::NativeCommand(opt),
    }
}

fn make_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args_vec);
    cmd
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::NativeCommand(Opt::Push) => cmd_push(),
        Args::NativeCommand(Opt::Switch { branch, create }) => cmd_switch(&branch, create),
        Args::NativeCommand(Opt::MergeDiff { target, path }) => cmd_merge_diff(&target, path),
        Args::NativeCommand(Opt::FakeMerge { source, message }) => {
            cmd_fake_merge(&source, &message)
        }
        Args::NativeCommand(Opt::Checkout { .. }) => cmd_checkout(),

        // Not implemented here.
        Args::NativeCommand(Opt::Cat { .. }) => (),
        Args::NativeCommand(Opt::Commit { .. }) => (),
        Args::NativeCommand(Opt::Merge { .. }) => (),
        Args::NativeCommand(Opt::Pull { .. }) => (),
        Args::NativeCommand(Opt::Log { .. }) => (),
        Args::GitCommand(args_vec) => {
            make_git_command(&args_vec).exec();
        }
    };
}
