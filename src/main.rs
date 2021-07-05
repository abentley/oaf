use std::env;
use std::ffi::OsStr;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output};
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
    run_for_string(&mut make_git_command(vec!["branch", "--show-current"]))
}

fn branch_setting(branch: &str, setting: &str) -> String {
    format!("branch.{}.{}", branch, setting)
}

fn run_for_status(cmd: &mut Command) -> ExitStatus {
    cmd.output().expect("Command could not run.").status
}

fn setting_exists(setting: &str) -> bool {
    let status = run_for_status(&mut make_git_command(vec!["config", "--get", setting]));
    status.success()
}

fn cmd_push() {
    let branch = get_current_branch();
    if setting_exists(&branch_setting(&branch, "remote")) {
        if !setting_exists(&branch_setting(&branch, "merge")) {
            panic!("Branch in unsupported state");
        }
        make_git_command(vec!["push"]).exec();
    } else {
        make_git_command(vec!["push", "-u", "origin", "HEAD"]).exec();
    }
}
fn create_stash() -> Option<String> {
    let oid = run_for_string(&mut make_git_command(vec!["stash", "create"]));
    if oid == "" {
        return None;
    }
    Some(oid)
}

fn apply_branch_stash(target_branch: &str) -> bool {
    let target_tag = make_wip_tag(target_branch);
    let output = &mut make_git_command(vec!["rev-parse", &format!("refs/tags/{}", target_tag)])
        .output()
        .expect("Couldn't run command");
    if !output.status.success() {
        return false;
    }
    let target_oid = output_to_string(&output);
    let status = run_for_status(&mut make_git_command(vec!["stash", "apply", &target_oid]));
    if !status.success() {
        panic!("Failed to apply WIP changes");
    }
    let status = delete_tag(&target_tag);
    if !status.success() {
        panic!("Failed to delete tag {}", target_tag);
    }
    return true;
}

fn make_wip_tag(branch: &str) -> String {
    format!("{}.wip", branch)
}

fn delete_tag(tag: &str) -> ExitStatus {
    run_for_status(&mut make_git_command(vec!["tag", "-d", tag]))
}

fn cmd_switch(target_branch: &str) {
    let current_tag = make_wip_tag(&get_current_branch());
    match create_stash() {
        Some(oid) => {
            let status =
                run_for_status(&mut make_git_command(vec!["tag", "-f", &current_tag, &oid]));
            if !status.success() {
                panic!("Failed to tag {} to {}", oid, current_tag);
            }
            eprintln!("Stashed WIP changes to {}", current_tag);
        }
        None => {
            let status = delete_tag(&current_tag);
            if !status.success() {
                let tag_list =
                    run_for_string(&mut make_git_command(vec!["tag", "-l", &current_tag]));
                if tag_list != "" {
                    panic!("Failed to delete tag {}", current_tag);
                }
            }
            eprintln!("No changes to stash");
        }
    }
    let status = run_for_status(&mut make_git_command(vec![
        "switch",
        "--discard-changes",
        &target_branch,
    ]));
    if !status.success() {
        panic!("Failed to switch to {}", target_branch);
    }
    eprintln!("Switched to {}", target_branch);
    if apply_branch_stash(&target_branch) {
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

fn make_git_command<T: AsRef<OsStr>>(args_vec: Vec<T>) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args_vec);
    cmd
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::NativeCommand(Opt::Push) => cmd_push(),
        Args::NativeCommand(Opt::Switch { branch }) => cmd_switch(&branch),
        // Not implemented here.
        Args::NativeCommand(Opt::Cat { .. }) => (),
        Args::GitCommand(args_vec) => {
            make_git_command(args_vec).exec();
        }
    };
}
