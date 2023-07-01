use std::collections::BTreeSet;

use clap::builder::NonEmptyStringValueParser;
use itertools::Itertools;
use jujutsu_lib::backend::{CommitId, ObjectId};
use jujutsu_lib::git::git_tracking_branches;
use jujutsu_lib::op_store::RefTarget;
use jujutsu_lib::repo::Repo;
use jujutsu_lib::revset;
use jujutsu_lib::view::View;

use crate::cli_util::{user_error, user_error_with_hint, CommandError, CommandHelper, RevisionArg};
use crate::commands::make_branch_term;
use crate::formatter::Formatter;
use crate::ui::Ui;

/// Manage branches.
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum BranchSubcommand {
    #[command(visible_alias("c"))]
    Create(BranchCreateArgs),
    #[command(visible_alias("d"))]
    Delete(BranchDeleteArgs),
    #[command(visible_alias("f"))]
    Forget(BranchForgetArgs),
    #[command(visible_alias("l"))]
    List(BranchListArgs),
    #[command(visible_alias("s"))]
    Set(BranchSetArgs),
}

/// Create a new branch.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchCreateArgs {
    /// The branch's target revision.
    #[arg(long, short)]
    revision: Option<RevisionArg>,

    /// The branches to create.
    #[arg(required = true, value_parser=NonEmptyStringValueParser::new())]
    names: Vec<String>,
}

/// Delete an existing branch and propagate the deletion to remotes on the
/// next push.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchDeleteArgs {
    /// The branches to delete.
    #[arg(required_unless_present_any(& ["glob"]))]
    names: Vec<String>,

    /// A glob pattern indicating branches to delete.
    #[arg(long)]
    pub glob: Vec<String>,
}

/// List branches and their targets
///
/// A remote branch will be included only if its target is different from
/// the local target. For a conflicted branch (both local and remote), old
/// target revisions are preceded by a "-" and new target revisions are
/// preceded by a "+". For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchListArgs;

/// Forget everything about a branch, including its local and remote
/// targets.
///
/// A forgotten branch will not impact remotes on future pushes. It will be
/// recreated on future pulls if it still exists in the remote.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchForgetArgs {
    /// The branches to forget.
    #[arg(required_unless_present_any(& ["glob"]))]
    pub names: Vec<String>,

    /// A glob pattern indicating branches to forget.
    #[arg(long)]
    pub glob: Vec<String>,
}

/// Update a given branch to point to a certain commit.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchSetArgs {
    /// The branch's target revision.
    #[arg(long, short)]
    pub revision: Option<RevisionArg>,

    /// Allow moving the branch backwards or sideways.
    #[arg(long, short = 'B')]
    pub allow_backwards: bool,

    /// The branches to update.
    #[arg(required = true)]
    pub names: Vec<String>,
}

pub fn cmd_branch(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BranchSubcommand,
) -> Result<(), CommandError> {
    match subcommand {
        BranchSubcommand::Create(sub_args) => cmd_branch_create(ui, command, sub_args),
        BranchSubcommand::Set(sub_args) => cmd_branch_set(ui, command, sub_args),
        BranchSubcommand::Delete(sub_args) => cmd_branch_delete(ui, command, sub_args),
        BranchSubcommand::Forget(sub_args) => cmd_branch_forget(ui, command, sub_args),
        BranchSubcommand::List(sub_args) => cmd_branch_list(ui, command, sub_args),
    }
}

fn cmd_branch_create(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchCreateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    let branch_names: Vec<&str> = args
        .names
        .iter()
        .map(|branch_name| match view.get_local_branch(branch_name) {
            Some(_) => Err(user_error_with_hint(
                format!("Branch already exists: {branch_name}"),
                "Use `jj branch set` to update it.",
            )),
            None => Ok(branch_name.as_str()),
        })
        .try_collect()?;

    if branch_names.len() > 1 {
        writeln!(
            ui.warning(),
            "warning: Creating multiple branches ({}).",
            branch_names.len()
        )?;
    }

    let target_commit =
        workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
    workspace_command.check_rewritable(&target_commit)?;
    let mut tx = workspace_command.start_transaction(&format!(
        "create {} pointing to commit {}",
        make_branch_term(&branch_names),
        target_commit.id().hex()
    ));
    for branch_name in branch_names {
        tx.mut_repo().set_local_branch(
            branch_name.to_string(),
            RefTarget::Normal(target_commit.id().clone()),
        );
    }
    tx.finish(ui)?;
    Ok(())
}

fn cmd_branch_set(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchSetArgs,
) -> Result<(), CommandError> {
    let branch_names = &args.names;
    let mut workspace_command = command.workspace_helper(ui)?;
    if branch_names.len() > 1 {
        writeln!(
            ui.warning(),
            "warning: Updating multiple branches ({}).",
            branch_names.len()
        )?;
    }

    let target_commit =
        workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
    workspace_command.check_rewritable(&target_commit)?;
    if !args.allow_backwards
        && !branch_names.iter().all(|branch_name| {
            is_fast_forward(
                workspace_command.repo().as_ref(),
                branch_name,
                target_commit.id(),
            )
        })
    {
        return Err(user_error_with_hint(
            "Refusing to move branch backwards or sideways.",
            "Use --allow-backwards to allow it.",
        ));
    }
    let mut tx = workspace_command.start_transaction(&format!(
        "point {} to commit {}",
        make_branch_term(branch_names),
        target_commit.id().hex()
    ));
    for branch_name in branch_names {
        tx.mut_repo().set_local_branch(
            branch_name.to_string(),
            RefTarget::Normal(target_commit.id().clone()),
        );
    }
    tx.finish(ui)?;
    Ok(())
}

/// This function may return the same branch more than once
fn find_globs(
    view: &View,
    globs: &[String],
    allow_deleted: bool,
) -> Result<Vec<String>, CommandError> {
    let mut matching_branches: Vec<String> = vec![];
    let mut failed_globs = vec![];
    for glob_str in globs {
        let glob = glob::Pattern::new(glob_str)?;
        let names = view
            .branches()
            .iter()
            .filter_map(|(branch_name, branch_target)| {
                if glob.matches(branch_name)
                    && (allow_deleted || branch_target.local_target.is_some())
                {
                    Some(branch_name)
                } else {
                    None
                }
            })
            .cloned()
            .collect_vec();
        if names.is_empty() {
            failed_globs.push(glob);
        }
        matching_branches.extend(names.into_iter());
    }
    match &failed_globs[..] {
        [] => { /* No problem */ }
        [glob] => {
            return Err(user_error(format!(
                "The provided glob '{glob}' did not match any branches"
            )))
        }
        globs => {
            return Err(user_error(format!(
                "The provided globs '{}' did not match any branches",
                globs.iter().join("', '")
            )))
        }
    };
    Ok(matching_branches)
}

fn cmd_branch_delete(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchDeleteArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    for branch_name in &args.names {
        if workspace_command
            .repo()
            .view()
            .get_local_branch(branch_name)
            .is_none()
        {
            return Err(user_error(format!("No such branch: {branch_name}")));
        }
    }
    let globbed_names = find_globs(view, &args.glob, false)?;
    let names: BTreeSet<String> = args.names.iter().cloned().chain(globbed_names).collect();
    let branch_term = make_branch_term(names.iter().collect_vec().as_slice());
    let mut tx = workspace_command.start_transaction(&format!("delete {branch_term}"));
    for branch_name in names.iter() {
        tx.mut_repo().remove_local_branch(branch_name);
    }
    tx.finish(ui)?;
    if names.len() > 1 {
        writeln!(ui, "Deleted {} branches.", names.len())?;
    }
    Ok(())
}

fn cmd_branch_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    for branch_name in args.names.iter() {
        if view.get_branch(branch_name).is_none() {
            return Err(user_error(format!("No such branch: {branch_name}")));
        }
    }
    let globbed_names = find_globs(view, &args.glob, true)?;
    let names: BTreeSet<String> = args.names.iter().cloned().chain(globbed_names).collect();
    let branch_term = make_branch_term(names.iter().collect_vec().as_slice());
    let mut tx = workspace_command.start_transaction(&format!("forget {branch_term}"));
    for branch_name in names.iter() {
        tx.mut_repo().remove_branch(branch_name);
    }
    tx.finish(ui)?;
    if names.len() > 1 {
        writeln!(ui, "Forgot {} branches.", names.len())?;
    }
    Ok(())
}

fn cmd_branch_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &BranchListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();

    let mut all_branches = repo.view().branches().clone();
    for (branch_name, git_tracking_target) in git_tracking_branches(repo.view()) {
        let branch_target = all_branches.entry(branch_name.to_owned()).or_default();
        if branch_target.remote_targets.contains_key("git") {
            // TODO(#1690): There should be a mechanism to prevent importing a
            // remote named "git" in `jj git import`.
            // TODO: This is not currently tested
            writeln!(
                ui.warning(),
                "WARNING: Branch {branch_name} has a remote-tracking branch for a remote named \
                 `git`. Local-git tracking branches for it will not be shown.\nIt is recommended \
                 to rename that remote, as jj normally reserves the `@git` suffix to denote \
                 local-git tracking branches."
            )?;
        } else {
            // TODO: `BTreeMap::try_insert` could be used here once that's stabilized.
            branch_target
                .remote_targets
                .insert("git".to_string(), git_tracking_target.clone());
        }
    }

    let print_branch_target =
        |formatter: &mut dyn Formatter, target: &RefTarget| -> Result<(), CommandError> {
            match target {
                RefTarget::Normal(id) => {
                    write!(formatter, ": ")?;
                    let commit = repo.store().get_commit(id)?;
                    workspace_command.write_commit_summary(formatter, &commit)?;
                    writeln!(formatter)?;
                }
                RefTarget::Conflict { removes, adds } => {
                    write!(formatter, " ")?;
                    write!(formatter.labeled("conflict"), "(conflicted)")?;
                    writeln!(formatter, ":")?;
                    for id in removes {
                        let commit = repo.store().get_commit(id)?;
                        write!(formatter, "  - ")?;
                        workspace_command.write_commit_summary(formatter, &commit)?;
                        writeln!(formatter)?;
                    }
                    for id in adds {
                        let commit = repo.store().get_commit(id)?;
                        write!(formatter, "  + ")?;
                        workspace_command.write_commit_summary(formatter, &commit)?;
                        writeln!(formatter)?;
                    }
                }
            }
            Ok(())
        };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    for (name, branch_target) in all_branches {
        let found_non_git_remote = {
            let pseudo_remote_count = branch_target.remote_targets.contains_key("git") as usize;
            branch_target.remote_targets.len() - pseudo_remote_count > 0
        };

        write!(formatter.labeled("branch"), "{name}")?;
        if let Some(target) = branch_target.local_target.as_ref() {
            print_branch_target(formatter, target)?;
        } else if found_non_git_remote {
            writeln!(formatter, " (deleted)")?;
        } else {
            writeln!(formatter, " (forgotten)")?;
        }

        for (remote, remote_target) in branch_target.remote_targets.iter() {
            if Some(remote_target) == branch_target.local_target.as_ref() {
                continue;
            }
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "@{remote}")?;
            if let Some(local_target) = branch_target.local_target.as_ref() {
                let remote_ahead_count =
                    revset::walk_revs(repo.as_ref(), remote_target.adds(), local_target.adds())?
                        .count();
                let local_ahead_count =
                    revset::walk_revs(repo.as_ref(), local_target.adds(), remote_target.adds())?
                        .count();
                if remote_ahead_count != 0 && local_ahead_count == 0 {
                    write!(formatter, " (ahead by {remote_ahead_count} commits)")?;
                } else if remote_ahead_count == 0 && local_ahead_count != 0 {
                    write!(formatter, " (behind by {local_ahead_count} commits)")?;
                } else if remote_ahead_count != 0 && local_ahead_count != 0 {
                    write!(
                        formatter,
                        " (ahead by {remote_ahead_count} commits, behind by {local_ahead_count} \
                         commits)"
                    )?;
                }
            }
            print_branch_target(formatter, remote_target)?;
        }

        if branch_target.local_target.is_none() {
            if found_non_git_remote {
                writeln!(
                    formatter,
                    "  (this branch will be *deleted permanently* on the remote on the\n   next \
                     `jj git push`. Use `jj branch forget` to prevent this)"
                )?;
            } else {
                writeln!(
                    formatter,
                    "  (this branch will be deleted from the underlying Git repo on the next `jj \
                     git export`)"
                )?;
            }
        }
    }

    Ok(())
}

fn is_fast_forward(repo: &dyn Repo, branch_name: &str, new_target_id: &CommitId) -> bool {
    if let Some(current_target) = repo.view().get_local_branch(branch_name) {
        current_target
            .adds()
            .iter()
            .any(|add| repo.index().is_ancestor(add, new_target_id))
    } else {
        true
    }
}
