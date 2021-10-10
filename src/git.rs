use std::ffi::{OsStr, OsString};
use std::path::{PathBuf};
use std::process::{Command, Output};
use std::os::unix::ffi::OsStringExt;
use std::str::from_utf8;

pub fn run_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Result<Output, Output> {
    let output = make_git_command(args_vec)
        .output()
        .expect("Couldn't run command");
    if !output.status.success() {
        return Err(output);
    }
    Ok(output)
}

pub fn output_to_string(output: &Output) -> String {
    from_utf8(&output.stdout)
        .expect("Output is not utf-8")
        .trim()
        .to_string()
}

pub fn make_git_command<T: AsRef<OsStr>>(args_vec: &[T]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args_vec);
    cmd
}

pub fn run_for_string(cmd: &mut Command) -> String {
    output_to_string(&cmd.output().expect("Could not run command."))
}

pub fn git_switch(
    target_branch: &str,
    create: bool,
    discard_changes: bool,
) -> Result<Output, Output> {
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
    switch_cmd.push(target_branch);
    run_git_command(&switch_cmd)
}

pub fn get_current_branch() -> String {
    run_for_string(&mut make_git_command(&["branch", "--show-current"]))
}

pub fn branch_setting(branch: &str, setting: &str) -> String {
    format!("branch.{}.{}", branch, setting)
}

pub fn setting_exists(setting: &str) -> bool {
    match run_git_command(&["config", "--get", setting]) {
        Ok(..) => true,
        Err(..) => false,
    }
}

pub fn full_branch(branch: String) -> String {
    if branch.starts_with("refs/heads/") {
        return branch;
    }
    return format!("refs/heads/{}", branch);
}

pub fn eval_rev_spec(rev_spec: &str) -> Result<String, Output> {
    Ok(output_to_string(&run_git_command(&[
        "rev-list", "-n1", rev_spec,
    ])?))
}

pub fn upsert_ref(git_ref: &str, value: &str) -> Result<(), Output> {
    run_git_command(&["update-ref", git_ref, value])?;
    Ok(())
}

pub fn delete_ref(git_ref: &str) -> Result<(), Output> {
    run_git_command(&["update-ref", "-d", git_ref])?;
    Ok(())
}

pub fn set_head(new_head: &str) {
    run_git_command(&["reset", "--soft", new_head]).expect("Failed to update HEAD.");
}

pub fn create_stash() -> Option<String> {
    let oid = run_for_string(&mut make_git_command(&["stash", "create"]));
    if oid.is_empty() {
        return None;
    }
    Some(oid)
}

pub fn get_toplevel() -> String {
    output_to_string(&run_git_command(&["rev-parse", "--show-toplevel"]).expect("Can't find top"))
}

fn one_liner(mut output: Output) -> OsString{
    output.stdout.pop();
    OsStringExt::from_vec(output.stdout)
}

pub fn get_git_path<T: AsRef<OsStr>> (sub_path: T) -> PathBuf {
    let string = one_liner(run_git_command(&["rev-parse".as_ref(), "--git-path".as_ref(), sub_path.as_ref()]).expect("Cannot find path location"));
    PathBuf::from(&string)
}
