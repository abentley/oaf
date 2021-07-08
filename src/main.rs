use std::env;
use std::ffi::OsStr;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command, Output};
use std::str::from_utf8;
use structopt::{clap, StructOpt};

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt {
    Cat {
        #[structopt(long, short, default_value = "HEAD")]
        tree: String,
        input: String,
    },
    Push,
    /**
    Switch to a branch, stashing any outstanding changes, and restoring any
    outstanding changes for that branch.

    Outstanding changes are stored as tags in the repo, with the branch's name
    suffixed with ".wip".  For example, outstanding changes for a branch named
    "foo" would be stored in a tag named "foo.wip".
    */
    Switch {
        /// The branch to switch to
        branch: String,
        #[structopt(long, short)]
        create: bool,
    },
}

fn cat_args(input: &str, mut tree: &str) -> Vec<String> {
    if tree == "index" {
        tree = "";
    }
    vec!["show".to_string(), format!("{}:./{}", tree, input)]
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
    if oid == "" {
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
            return Some(current_tag);
        }
        None => {
            if let Err(..) = delete_tag(&current_tag) {
                let tag_list = run_for_string(&mut make_git_command(&["tag", "-l", &current_tag]));
                if tag_list != "" {
                    panic!("Failed to delete tag {}", current_tag);
                }
            }
            return None;
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
        Err(..) => {
            return false;
        }
        Ok(target_oid) => {
            run_git_command(&["stash", "apply", &target_oid]).unwrap();
            delete_tag(&target_tag).unwrap();
            return true;
        }
    }
}

fn git_switch(target_branch: &str, create: bool) {
    // Actual "switch" is not broadly deployed yet.
    // let mut switch_cmd = vec!["switch", "--discard-changes"];
    // --force means "discard local changes".
    let mut switch_cmd = vec!["checkout", "--force"];
    if create {
        if let Err(..) = run_git_command(&["reset", "--hard"]) {
            panic!("Failed to reset tree");
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
                eprintln!("Branch {} not found", target_branch);
                exit(1);
            }
        }
        Ok(..) => {
            if create {
                eprintln!("Branch {} already exists", target_branch);
                exit(1);
            }
        }
    };
    if let Some(current_tag) = create_branch_stash() {
        eprintln!("Stashed WIP changes to {}", current_tag);
    } else {
        eprintln!("No changes to stash");
    }
    git_switch(target_branch, create);
    eprintln!("Switched to {}", target_branch);
    if !create && apply_branch_stash(&target_branch) {
        eprintln!("Applied WIP changes for {}", target_branch);
    } else {
        eprintln!("No WIP changes for {} to restore", target_branch);
    }
}

enum Args {
    NativeCommand(Opt),
    GitCommand(Vec<String>),
}

fn parse_args() -> Args {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec: Vec<String> = args_iter.collect();
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "nit" => Opt::from_args_safe(),
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
            Ok(Opt::from_iter(args))
        }
    };
    match opt {
        Ok(Opt::Cat { input, tree }) => Args::GitCommand(cat_args(&input, &tree)),
        Ok(opt) => Args::NativeCommand(opt),
        Err(err) => {
            if err.kind != clap::ErrorKind::UnknownArgument {
                err.exit();
            }
            Args::GitCommand(args_vec)
        }
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
        // Not implemented here.
        Args::NativeCommand(Opt::Cat { .. }) => (),
        Args::GitCommand(args_vec) => {
            make_git_command(&args_vec).exec();
        }
    };
}
