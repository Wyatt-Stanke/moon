use crate::action::{Action, ActionStatus};
use crate::context::ActionContext;
use crate::errors::ActionError;
use moon_config::{tsconfig::TsConfigJson, TypeScriptConfig};
use moon_logger::{color, debug};
use moon_project::Project;
use moon_utils::{is_ci, path};
use moon_workspace::Workspace;
use std::sync::Arc;
use tokio::sync::RwLock;

const LOG_TARGET: &str = "moon:action:sync-node-project";

// Sync projects references to the root `tsconfig.json`.
fn sync_root_tsconfig(
    tsconfig: &mut TsConfigJson,
    typescript_config: &TypeScriptConfig,
    project: &Project,
) -> bool {
    if project
        .root
        .join(&typescript_config.project_config_file_name)
        .exists()
        && tsconfig.add_project_ref(&project.source, &typescript_config.project_config_file_name)
    {
        debug!(
            target: LOG_TARGET,
            "Syncing {} as a project reference to the root {}",
            color::id(&project.id),
            color::file(&typescript_config.root_config_file_name)
        );

        return true;
    }

    false
}

pub async fn sync_node_project(
    _action: &mut Action,
    _context: &ActionContext,
    workspace: Arc<RwLock<Workspace>>,
    project_id: &str,
) -> Result<ActionStatus, ActionError> {
    let mut mutated_files = false;
    let mut typescript_config;

    // Read only
    {
        let workspace = workspace.read().await;
        let mut project = workspace.projects.load(project_id)?;
        let node_config = &workspace.config.node;

        // Copy values outside of this block
        typescript_config = workspace.config.typescript.clone();

        // Load project configs
        project.load_package_json().await?;

        let has_tsconfig = project.load_tsconfig_json(&typescript_config).await?;

        if !has_tsconfig
            && typescript_config.create_missing_config
            && typescript_config.sync_project_references
        {
            project
                .create_tsconfig_json(&typescript_config, &workspace.root)
                .await?;
        }

        // Sync each dependency to `tsconfig.json` and `package.json`
        let dep_version_range = workspace
            .toolchain
            .get_node()
            .get_package_manager()
            .get_workspace_dependency_range();

        for dep_id in project.get_dependencies() {
            let dep_project = workspace.projects.load(&dep_id)?;

            // Update `dependencies` within this project's `package.json`
            if node_config.sync_project_workspace_dependencies {
                if let Some(package_json) = project.package_json.get_mut() {
                    let dep_package_name =
                        dep_project.get_package_name().await?.unwrap_or_default();

                    // Only add if the dependent project has a `package.json`,
                    // and this `package.json` has not already declared the dep.
                    if !dep_package_name.is_empty()
                        && package_json.add_dependency(&dep_package_name, &dep_version_range, true)
                    {
                        debug!(
                            target: LOG_TARGET,
                            "Syncing {} as a dependency to {}'s {}",
                            color::id(&dep_id),
                            color::id(project_id),
                            color::file("package.json")
                        );

                        package_json.save().await?;
                        mutated_files = true;
                    }
                }
            }

            // Update `references` within this project's `tsconfig.json`
            if typescript_config.sync_project_references {
                if let Some(tsconfig_json) = project.tsconfig_json.get_mut() {
                    let tsconfig_branch_name = &typescript_config.project_config_file_name;
                    let dep_ref_path = path::to_string(
                        &path::relative_from(&dep_project.root, &project.root).unwrap_or_default(),
                    )?;

                    // Only add if the dependent project has a `tsconfig.json`,
                    // and this `tsconfig.json` has not already declared the dep.
                    if dep_project.root.join(tsconfig_branch_name).exists()
                        && tsconfig_json.add_project_ref(&dep_ref_path, tsconfig_branch_name)
                    {
                        debug!(
                            target: LOG_TARGET,
                            "Syncing {} as a project reference to {}'s {}",
                            color::id(&dep_id),
                            color::id(project_id),
                            color::file(tsconfig_branch_name)
                        );

                        tsconfig_json.save().await?;
                        mutated_files = true;
                    }
                } else {
                    // Project doesnt have a `tsconfig.json`
                    typescript_config.sync_project_references = false;
                }
            }
        }
    }

    // Writes root `tsconfig.json`
    {
        // Sync a project reference
        if typescript_config.sync_project_references {
            let mut workspace = workspace.write().await;
            let project = workspace.projects.load(project_id)?;

            if let Some(tsconfig) = &mut workspace.tsconfig_json {
                if sync_root_tsconfig(tsconfig, &typescript_config, &project) {
                    tsconfig.save().await?;
                    mutated_files = true;
                }
            }
        }
    }

    if mutated_files {
        // If files have been modified in CI, we should update the status to warning,
        // as these modifications should be committed to the repo.
        if is_ci() {
            return Ok(ActionStatus::Invalid);
        } else {
            return Ok(ActionStatus::Passed);
        }
    }

    Ok(ActionStatus::Skipped)
}