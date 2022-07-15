use enum_dispatch::enum_dispatch;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::str::{from_utf8, FromStr};

pub fn run_git_command(args_vec: &[impl AsRef<OsStr>]) -> Result<Output, Output> {
    let process_output = make_git_command(args_vec)
        .output()
        .expect("Couldn't run command");
    if !process_output.status.success() {
        return Err(process_output);
    }
    Ok(process_output)
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

pub fn get_current_branch() -> Result<LocalBranchName, UnhandledNameType> {
    run_for_string(&mut make_git_command(&["branch", "--show-current"])).parse()
}

pub fn setting_exists(setting: &str) -> bool {
    match run_config(&["--get", setting]) {
        Ok(..) => true,
        Err(ConfigErr::SectionKeyInvalid) => false,
        Err(e) => panic!("{:?}", e),
    }
}

pub enum SettingLocation {
    Local,
}

/**
 * Set a setting to a specific value.
 */
pub fn set_setting(
    _location: SettingLocation,
    setting: &str,
    value: &str,
) -> Result<(), ConfigErr> {
    run_config(&["--replace", "--local", setting, value])?;
    Ok(())
}

#[enum_dispatch(BranchName)]
pub trait ReferenceSpec {
    fn full(&self) -> String;
    fn short(&self) -> String;
    fn eval(&self) -> Result<String, Output> {
        eval_rev_spec(&self.full())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalBranchName {
    pub name: String,
}

#[derive(Debug, PartialEq)]
pub struct UnhandledNameType {
    pub name: String,
}
impl fmt::Display for UnhandledNameType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Unhandled name type: {}", self.name)
    }
}

impl ReferenceSpec for UnhandledNameType {
    fn full(&self) -> String {
        self.name.clone()
    }
    fn short(&self) -> String {
        self.name.clone()
    }
}

impl LocalBranchName {
    pub fn with_repo(self, repo: String) -> RemoteBranchName {
        RemoteBranchName {
            repo,
            name: self.name,
        }
    }
    pub fn setting_name(&self, setting_name: &str) -> String {
        format!("branch.{}.{}", self.name, setting_name)
    }
    pub fn is_valid(&self) -> bool {
        run_git_command(&["check-ref-format", "--allow-onelevel", &self.name]).is_ok()
    }
}

impl FromStr for LocalBranchName {
    type Err = UnhandledNameType;
    fn from_str(name: &str) -> Result<Self, UnhandledNameType> {
        let short_name = match name.split_once("refs/heads/") {
            Some((_prefix, name)) => name,
            None => {
                if name.contains('/') {
                    return Err(UnhandledNameType {
                        name: name.to_string(),
                    });
                }
                name
            }
        };
        Ok(Self {
            name: short_name.to_string(),
        })
    }
}

impl ReferenceSpec for LocalBranchName {
    fn full(&self) -> String {
        format!("refs/heads/{}", self.name)
    }
    fn short(&self) -> String {
        self.name.clone()
    }
}

#[enum_dispatch]
#[derive(Debug, PartialEq)]
pub enum BranchName {
    Local(LocalBranchName),
    Remote(RemoteBranchName),
}

impl FromStr for BranchName {
    type Err = UnhandledNameType;
    fn from_str(name: &str) -> Result<Self, UnhandledNameType> {
        if let Ok(lb) = LocalBranchName::from_str(name) {
            Ok(BranchName::Local(lb))
        } else {
            Ok(BranchName::Remote(RemoteBranchName::from_str(name)?))
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RemoteBranchName {
    pub repo: String,
    pub name: String,
}

impl FromStr for RemoteBranchName {
    type Err = UnhandledNameType;
    fn from_str(name: &str) -> Result<Self, UnhandledNameType> {
        let short_name = match name.split_once("refs/remotes/") {
            Some((_prefix, name)) => name,
            None => {
                if name.starts_with("refs/") {
                    return Err(UnhandledNameType {
                        name: name.to_string(),
                    });
                }
                name
            }
        };
        if let Some((repo, name)) = short_name.split_once('/') {
            Ok(Self {
                repo: repo.into(),
                name: name.into(),
            })
        } else {
            Err(UnhandledNameType {
                name: name.to_string(),
            })
        }
    }
}

impl ReferenceSpec for RemoteBranchName {
    fn full(&self) -> String {
        format!("refs/remotes/{}/{}", self.repo, self.name)
    }
    fn short(&self) -> String {
        format!("{}/{}", self.repo, self.name)
    }
}

pub fn eval_rev_spec(rev_spec: &str) -> Result<String, Output> {
    Ok(output_to_string(&run_git_command(&[
        "rev-list", "-n1", rev_spec,
    ])?))
}

#[derive(Debug)]
pub enum GitError {
    NotAGitRepository,
    NotAWorkTree,
    UnknownError(OsString),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GitError::NotAGitRepository => {
                write!(f, "Not in a Git repository")
            }
            GitError::NotAWorkTree => {
                write!(f, "Not in a Git work tree")
            }
            GitError::UnknownError(stderr) => {
                write!(f, "Unknown Error {}", stderr.to_string_lossy())
            }
        }
    }
}

impl GitError {
    pub fn from(stderr: OsString) -> Self {
        let stderr_str = stderr.to_string_lossy();
        if stderr_str.starts_with("fatal: not a git repository") {
            GitError::NotAGitRepository
        } else if stderr_str.starts_with("fatal: this operation must be run in a work tree") {
            GitError::NotAWorkTree
        } else {
            GitError::UnknownError(stderr)
        }
    }
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

pub fn get_toplevel() -> Result<String, GitError> {
    Ok(output_to_string(
        &match run_git_command(&["rev-parse", "--show-toplevel"]) {
            Ok(output) => output,
            Err(output) => return Err(GitError::from(OsString::from_vec(output.stderr))),
        },
    ))
}

fn one_liner(mut output: Output) -> OsString {
    output.stdout.pop();
    OsStringExt::from_vec(output.stdout)
}

pub fn get_git_path<T: AsRef<OsStr>>(sub_path: T) -> PathBuf {
    let string = one_liner(
        run_git_command(&[
            "rev-parse".as_ref(),
            "--git-path".as_ref(),
            sub_path.as_ref(),
        ])
        .expect("Cannot find path location"),
    );
    PathBuf::from(&string)
}

/**
 * Escape characters that can appear in a git-compatible regex
 */
fn escape_re<T: AsRef<str>>(input: T) -> String {
    input
        .as_ref()
        .chars()
        .map(|x| match x {
            '^' | '$' | '.' | '\\' | '|' | '[' | ']' | '(' | ')' | '{' | '}' | '?' | '*' | '+' => {
                format!("\\{}", x)
            }
            _ => x.to_string(),
        })
        .collect()
}

/**
 * Generate a regex for a list of settings, which can be used with --get-regexp
 */
fn settings_re<P: AsRef<str>, S: AsRef<str>>(prefix: P, settings: &[S]) -> String {
    let mut output = format!("{}(", escape_re(prefix));
    for (i, setting) in settings.iter().enumerate() {
        if i != 0 {
            output.push('|')
        };
        output.push_str(&escape_re(setting));
    }
    output.push(')');
    output
}

/// We don't want to puke if we can't parse the settings, so provide an enum that supports invalid
/// entries.
#[derive(Debug, PartialEq)]
pub enum SettingEntry {
    Valid { key: String, value: String },
    Invalid(String),
}

/**
 * Parse a string containing 0-terminated settings entries.  (Key and value are separated by \n).
 */
fn parse_settings(setting_text: &str) -> Vec<SettingEntry> {
    let mut output = Vec::<SettingEntry>::new();
    for entry in setting_text.split_terminator('\0') {
        output.push(if let Some((key, value)) = entry.split_once('\n') {
            SettingEntry::Valid {
                key: key.to_string(),
                value: value.to_string(),
            }
        } else {
            SettingEntry::Invalid(String::from(entry))
        });
    }
    output
}

#[derive(Debug)]
pub enum ConfigErr {
    SectionKeyInvalid,
    SectionKeyMissing,
    ConfigInvalid,
    ConfigUnwritable,
    UnsetMissing,
    InvalidRegex,
    Other(Output),
}

/**
 * Convert the error output of `git config`
 */
impl From<Output> for ConfigErr {
    fn from(output: Output) -> ConfigErr {
        match output.status.code().expect("Failed to call config") {
            1 => ConfigErr::SectionKeyInvalid,
            2 => ConfigErr::SectionKeyMissing,
            3 => ConfigErr::ConfigInvalid,
            4 => ConfigErr::ConfigUnwritable,
            5 => ConfigErr::UnsetMissing,
            6 => ConfigErr::InvalidRegex,
            _ => ConfigErr::Other(output),
        }
    }
}

/**
 * Run 'git config' with supplied arguments
 */
pub fn run_config(args: &[impl AsRef<OsStr>]) -> Result<Output, ConfigErr> {
    let mut args_vec: Vec<OsString> = vec!["config".into()];
    args_vec.extend(args.iter().map(|a| a.into()));
    let result = run_git_command(&args_vec);
    match result {
        Ok(output) => Ok(output),
        Err(output) => Err(output.into()),
    }
}

/**
 * Get a Vec of SettingsEntry items for the supplied settings and prefix.
 */
pub fn get_settings<P: AsRef<str>, S: AsRef<str>>(prefix: P, settings: &[S]) -> Vec<SettingEntry> {
    let regex = settings_re(prefix, settings);
    let result = run_config(&["--null", "--get-regexp", &regex]);
    match result {
        Ok(output) => parse_settings(&output_to_string(&output)),
        Err(ConfigErr::SectionKeyInvalid) => vec![],
        Err(e) => {
            panic!("Failed to get settings: {:?}", e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_re() {
        assert_eq!(
            escape_re("^a.\\|[](){}?*+b$"),
            "\\^a\\.\\\\\\|\\[\\]\\(\\)\\{\\}\\?\\*\\+b\\$"
        );
    }

    #[test]
    fn test_settings_re() {
        assert_eq!(
            settings_re("a.b.", &["b$rk", "b|te"]),
            "a\\.b\\.(b\\$rk|b\\|te)"
        );
    }
    #[test]
    fn test_parse_settings() {
        assert_eq!(
            parse_settings(concat!(
                "branch.main.merge\nrefs/heads/main\0",
                "branch.main.remote\norigin\0",
                "branch.main.oaf-target-branch\norigin/develop\0",
                "inv"
            )),
            vec![
                SettingEntry::Valid {
                    key: "branch.main.merge".to_string(),
                    value: "refs/heads/main".to_string(),
                },
                SettingEntry::Valid {
                    key: "branch.main.remote".to_string(),
                    value: "origin".to_string(),
                },
                SettingEntry::Valid {
                    key: "branch.main.oaf-target-branch".to_string(),
                    value: "origin/develop".to_string(),
                },
                SettingEntry::Invalid(String::from("inv")),
            ]
        );
    }
    #[test]
    fn test_parse_branch_name() {
        let x = "refs/heads/foo".parse::<BranchName>();
        assert_eq!(
            x,
            Ok(BranchName::Local(LocalBranchName { name: "foo".into() }))
        );
        let y = "refs/remotes/origin/foo".parse::<BranchName>();
        assert_eq!(
            y,
            Ok(BranchName::Remote(RemoteBranchName {
                repo: "origin".into(),
                name: "foo".into(),
            }))
        );
        let y2 = "origin/foo".parse::<BranchName>();
        assert_eq!(
            y2,
            Ok(BranchName::Remote(RemoteBranchName {
                repo: "origin".into(),
                name: "foo".into(),
            }))
        );
        let z = "refs/baz/origin/foo".parse::<BranchName>();
        assert_eq!(
            z,
            Err(UnhandledNameType {
                name: "refs/baz/origin/foo".into()
            })
        );
    }
}
