//! The forest's commands: each one's observe and execute steps. Commands return result
//! structs, defined with their renderers in `git-forest-core`, instead of printing directly;
//! src/lib.rs formats them as human-readable or JSON based on --json.
//! See docs/decisions/0002-functional-core-imperative-shell.md.
mod branch_state;
mod exec;
mod init;
mod ls;
mod new;
mod reset;
mod rm;
mod status;

pub use exec::{cmd_exec, format_exec_human, ExecFailureCode, ExecProgress, ExecResult};
pub use init::{cmd_init, format_init_human, InitInputs, RepoInput};
pub use ls::{cmd_ls, format_ls_human};
pub use new::{cmd_new_at, format_new_human, NewInputs};
pub use reset::{cmd_reset, format_reset_human, format_reset_summary, ResetProgress, ResetResult};
pub use rm::{
    cmd_rm_all_with_options, cmd_rm_with_options, format_repo_done, format_rm_all_human,
    format_rm_all_summary, format_rm_human, format_rm_summary, RmAllProgress, RmAllResult,
    RmOptions, RmProgress, RmResult,
};
pub use status::{cmd_status, format_status_human};

#[cfg(test)]
pub(crate) use new::cmd_new;
