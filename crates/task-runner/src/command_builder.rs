use moon_action::ActionNode;
use moon_action_context::ActionContext;
use moon_app_context::AppContext;
use moon_common::path::PathExt;
use moon_config::TaskOptionAffectedFiles;
use moon_env_var::GlobalEnvBag;
use moon_pdk_api::{Extend, ExtendTaskCommandInput, ExtendTaskScriptInput};
use moon_platform::PlatformManager;
use moon_process::{Command, Shell, ShellType};
use moon_project::Project;
use moon_task::Task;
use moon_toolchain::{get_version_env_key, get_version_env_value, is_using_global_toolchain};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, instrument, trace};

pub struct CommandBuilder<'task> {
    app: &'task AppContext,
    node: &'task ActionNode,
    project: &'task Project,
    task: &'task Task,
    working_dir: &'task Path,
    env_bag: &'task GlobalEnvBag,
    platform_manager: &'task PlatformManager,

    // To be built
    command: Command,
    using_platform: bool,
}

impl<'task> CommandBuilder<'task> {
    pub fn new(
        app: &'task AppContext,
        project: &'task Project,
        task: &'task Task,
        node: &'task ActionNode,
    ) -> Self {
        let working_dir = if task.options.run_from_workspace_root {
            &app.workspace_root
        } else {
            &project.root
        };

        Self {
            app,
            node,
            project,
            task,
            working_dir,
            env_bag: GlobalEnvBag::instance(),
            platform_manager: PlatformManager::read(),
            command: Command::new("noop"),
            using_platform: false,
        }
    }

    pub fn set_env_bar(&mut self, bag: &'task GlobalEnvBag) {
        self.env_bag = bag;
    }

    pub fn set_platform_manager(&mut self, manager: &'task PlatformManager) {
        self.platform_manager = manager;
    }

    #[instrument(name = "build_command", skip_all)]
    pub async fn build(mut self, context: &ActionContext) -> miette::Result<Command> {
        debug!(
            task_target = self.task.target.as_str(),
            working_dir = ?self.working_dir,
            "Creating task child process to execute",
        );

        self.command = self.build_command(context).await?;

        // We need to handle non-zero exit code's manually
        self.command
            .cwd(self.working_dir)
            .set_error_on_nonzero(false);

        // Order is important!
        self.inject_args(context);
        self.inject_env();
        self.inject_shell();
        self.inherit_affected(context)?;
        self.inherit_config();
        self.inherit_proto();

        // Must be last!
        self.command.inherit_path()?;

        Ok(self.command)
    }

    async fn build_command(&mut self, context: &ActionContext) -> miette::Result<Command> {
        let task = self.task;
        let toolchain_ids = self.project.get_enabled_toolchains_for_task(task);

        let mut command = match self
            .platform_manager
            .get_by_toolchains(&self.task.toolchains)
        {
            Ok(platform) => {
                self.using_platform = true;

                platform
                    .create_run_target_command(
                        context,
                        self.project,
                        self.task,
                        self.node.get_runtime(),
                        self.working_dir,
                    )
                    .await?
            }
            Err(_) => {
                // No platform so create a custom command
                let mut cmd = Command::new(&task.command);
                cmd.args(&task.args);
                cmd.envs_if_not_global(&task.env);
                cmd
            }
        };

        match &task.script {
            // If a script, overwrite the binary (command) with the script and reset args,
            // but also inherit all environment variables and paths from the platform
            Some(script) => {
                command.bin = script.into();
                command.args.clear();

                // Scripts should be used as-is
                command.escape_args = false;

                for params in self
                    .app
                    .toolchain_registry
                    .extend_task_script_many(toolchain_ids, |registry, _| ExtendTaskScriptInput {
                        context: registry.create_context(),
                        script: script.clone(),
                        task: task.to_fragment(),
                        ..Default::default()
                    })
                    .await?
                {
                    if let Some(new_script) = params.script {
                        command.bin = new_script.into();
                    }

                    self.extend_with_env(&mut command, params.env, params.env_remove);
                    self.extend_with_paths(&mut command, params.paths);
                }
            }
            None => {
                for params in self
                    .app
                    .toolchain_registry
                    .extend_task_command_many(toolchain_ids, |registry, _| ExtendTaskCommandInput {
                        context: registry.create_context(),
                        command: task.command.clone(),
                        args: task.args.clone(),
                        task: task.to_fragment(),
                        ..Default::default()
                    })
                    .await?
                {
                    if let Some(new_bin) = params.command {
                        command.bin = new_bin.into();
                    }

                    if let Some(new_args) = params.args {
                        self.extend_with_args(&mut command, new_args);
                    }

                    self.extend_with_env(&mut command, params.env, params.env_remove);
                    self.extend_with_paths(&mut command, params.paths);
                }
            }
        };

        Ok(command)
    }

    #[instrument(skip_all)]
    fn inject_args(&mut self, context: &ActionContext) {
        // Must be first!
        if let ActionNode::RunTask(inner) = &self.node
            && !inner.args.is_empty()
        {
            trace!(
                task_target = self.task.target.as_str(),
                args = ?inner.args,
                "Inheriting args from dependent task"
            );

            self.command.args(&inner.args);
        }

        if self.task.script.is_none()
            && context.should_inherit_args(&self.task.target)
            && !context.passthrough_args.is_empty()
        {
            trace!(
                task_target = self.task.target.as_str(),
                args = ?context.passthrough_args,
                "Inheriting args passed through the command line"
            );

            self.command.args(&context.passthrough_args);
        }
    }

    #[instrument(skip_all)]
    fn inject_env(&mut self) {
        // Must be first!
        if let ActionNode::RunTask(inner) = &self.node
            && !inner.env.is_empty()
        {
            trace!(
                task_target = self.task.target.as_str(),
                env = ?inner.env,
                "Inheriting env from dependent task"
            );

            self.command.envs(&inner.env);
        }

        self.command.env("PWD", self.working_dir);

        // moon
        self.command
            .env("MOON_CACHE_DIR", &self.app.cache_engine.cache_dir);
        self.command
            .env("MOON_PROJECT_ID", self.project.id.as_str());
        self.command.env("MOON_PROJECT_ROOT", &self.project.root);
        self.command
            .env("MOON_PROJECT_SOURCE", self.project.source.as_str());
        self.command.env("MOON_TARGET", self.task.target.as_str());
        self.command
            .env("MOON_WORKSPACE_ROOT", &self.app.workspace_root);
        self.command.env("MOON_WORKING_DIR", &self.app.working_dir);
        self.command.env(
            "MOON_PROJECT_SNAPSHOT",
            self.app
                .cache_engine
                .state
                .get_project_snapshot_path(&self.project.id),
        );

        // proto
        self.command.env("PROTO_AUTO_INSTALL", "false");
        self.command.env("PROTO_IGNORE_MIGRATE_WARNING", "true");
        self.command.env("PROTO_NO_PROGRESS", "true");
        self.command.env(
            "PROTO_VERSION",
            self.app.toolchain_config.proto.version.to_string(),
        );

        for (key, value) in self.app.toolchain_config.get_version_env_vars() {
            // Don't overwrite proto version variables inherited from toolchains
            self.command.env_if_missing(key, value);
        }
    }

    #[instrument(skip_all)]
    fn inject_shell(&mut self) {
        if self.task.options.shell == Some(true) {
            // Process command set's a shell by default!

            #[cfg(unix)]
            if let Some(shell) = &self.task.options.unix_shell {
                use moon_config::TaskUnixShell;

                self.command.with_shell(match shell {
                    TaskUnixShell::Bash => Shell::new(ShellType::Bash),
                    TaskUnixShell::Elvish => Shell::new(ShellType::Elvish),
                    TaskUnixShell::Fish => Shell::new(ShellType::Fish),
                    TaskUnixShell::Ion => Shell::new(ShellType::Ion),
                    TaskUnixShell::Murex => Shell::new(ShellType::Murex),
                    TaskUnixShell::Nu => Shell::new(ShellType::Nu),
                    TaskUnixShell::Pwsh => Shell::new(ShellType::Pwsh),
                    TaskUnixShell::Xonsh => Shell::new(ShellType::Xonsh),
                    TaskUnixShell::Zsh => Shell::new(ShellType::Zsh),
                });
            }

            #[cfg(windows)]
            if let Some(shell) = &self.task.options.windows_shell {
                use moon_config::TaskWindowsShell;

                self.command.with_shell(match shell {
                    TaskWindowsShell::Bash => Shell::new(ShellType::Bash),
                    TaskWindowsShell::Elvish => Shell::new(ShellType::Elvish),
                    TaskWindowsShell::Fish => Shell::new(ShellType::Fish),
                    TaskWindowsShell::Murex => Shell::new(ShellType::Murex),
                    TaskWindowsShell::Nu => Shell::new(ShellType::Nu),
                    TaskWindowsShell::Pwsh => Shell::new(ShellType::Pwsh),
                    TaskWindowsShell::Xonsh => Shell::new(ShellType::Xonsh),
                });
            }
        } else {
            self.command.without_shell();
        }
    }

    #[instrument(skip_all)]
    fn inherit_affected(&mut self, context: &ActionContext) -> miette::Result<()> {
        let Some(check_affected) = &self.task.options.affected_files else {
            return Ok(());
        };

        // Only get files when `--affected` is passed
        let mut abs_files = if context.affected.is_some() {
            self.task.get_affected_files(
                &self.app.workspace_root,
                &context.touched_files,
                &self.project.source,
            )?
        } else {
            Vec::with_capacity(0)
        };

        // If we have no files, use the task's inputs instead
        if abs_files.is_empty() && self.task.options.affected_pass_inputs {
            abs_files = self.task.get_input_files(&self.app.workspace_root)?;
        }

        abs_files.sort();

        // Convert to project relative paths
        let rel_files = abs_files
            .into_iter()
            .filter_map(|abs_file| {
                if abs_file.starts_with(&self.project.root) {
                    abs_file.relative_to(self.working_dir).ok()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // Set an environment variable
        if matches!(
            check_affected,
            TaskOptionAffectedFiles::Env | TaskOptionAffectedFiles::Enabled(true)
        ) {
            self.command.env(
                "MOON_AFFECTED_FILES",
                if rel_files.is_empty() {
                    ".".into()
                } else {
                    rel_files
                        .iter()
                        .map(|file| file.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                },
            );
        }

        // Pass an argument
        if matches!(
            check_affected,
            TaskOptionAffectedFiles::Args | TaskOptionAffectedFiles::Enabled(true)
        ) {
            if rel_files.is_empty() {
                self.command.arg_if_missing(".");
            } else {
                // Mimic relative from ("./")
                self.command.args(rel_files.iter().map(|file| {
                    let arg = format!("./{file}");

                    // Escape files with special characters
                    if arg.contains(['*', '$', '+', '[', ']']) {
                        format!("\"{arg}\"")
                    } else {
                        arg
                    }
                }));
            }
        }

        Ok(())
    }

    fn inherit_config(&mut self) {
        // Terminal colors
        if self
            .app
            .workspace_config
            .pipeline
            .inherit_colors_for_piped_tasks
        {
            self.command.inherit_colors();
        }
    }

    fn inherit_proto(&mut self) {
        // The values below were inherited by the platform already
        if self.using_platform {
            return;
        }

        // Inherit common parameters
        self.app
            .toolchain_registry
            .prepare_process_command(&mut self.command, self.env_bag);

        // Inherit project overrides
        for (id, config) in &self.project.config.toolchain.plugins {
            if is_using_global_toolchain(self.env_bag, id) {
                continue;
            }

            if let Some(version) = config.get_version() {
                self.command
                    .env(get_version_env_key(id), get_version_env_value(version));
            }
        }
    }

    fn extend_with_args(&self, command: &mut Command, args: Extend<Vec<String>>) {
        match args {
            Extend::Empty => {
                command.args.clear();
            }
            Extend::Append(next) => {
                command.args(next);
            }
            Extend::Prepend(next) => {
                let prev = std::mem::take(&mut command.args);
                command.args(next);
                command.args(prev);
            }
            Extend::Replace(next) => {
                command.args.clear();
                command.args(next);
            }
        }
    }

    fn extend_with_env(
        &self,
        command: &mut Command,
        env: FxHashMap<String, String>,
        env_remove: Vec<String>,
    ) {
        command.envs_if_not_global(env);

        for key in env_remove {
            command.env_remove(key);
        }
    }

    fn extend_with_paths(&self, command: &mut Command, next_paths: Vec<PathBuf>) {
        if next_paths.is_empty() {
            return;
        }

        // Normalize separators since WASM always uses forward slashes
        #[cfg(windows)]
        {
            command.prepend_paths(next_paths.into_iter().map(|path| {
                PathBuf::from(moon_common::path::normalize_separators(
                    path.to_string_lossy(),
                ))
            }));
        }

        #[cfg(unix)]
        {
            command.prepend_paths(next_paths);
        }
    }
}
