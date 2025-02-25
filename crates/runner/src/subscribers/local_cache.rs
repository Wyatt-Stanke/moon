use crate::emitter::Event;
use moon_contract::EventFlow;
use moon_error::MoonError;
use moon_utils::path;
use moon_workspace::Workspace;

/// The local cache subscriber is in charge of managing archives
/// (task output's archived as tarballs), by reading and writing them
/// to the `.moon/cache/{out,hashes}` directories.
///
/// This is the last subscriber amongst all subscribers, as local
/// cache is the last line of defense. However, other subscribers
/// will piggyback off of it, like remote cache.
pub struct LocalCacheSubscriber {}

impl LocalCacheSubscriber {
    pub fn new() -> Self {
        LocalCacheSubscriber {}
    }

    pub async fn on_emit<'a>(
        &mut self,
        event: &Event<'a>,
        workspace: &Workspace,
    ) -> Result<EventFlow, MoonError> {
        match event {
            // Check if the archive exists on the local file system
            Event::TargetOutputCacheCheck { hash, .. } => {
                if workspace.cache.is_hash_cached(hash) {
                    return Ok(EventFlow::Return("local-cache".into()));
                }
            }

            // Archive the task's outputs into the cache
            Event::TargetOutputArchiving {
                hash,
                project,
                task,
            } => {
                let archive_path = workspace
                    .cache
                    .create_hash_archive(hash, &project.root, &task.outputs)
                    .await?;

                return Ok(EventFlow::Return(path::to_string(archive_path)?));
            }

            // Hydrate the cached archive into the task's outputs
            Event::TargetOutputHydrating { hash, project, .. } => {
                let archive_path = workspace
                    .cache
                    .hydrate_from_hash_archive(hash, &project.root)
                    .await?;

                return Ok(EventFlow::Return(path::to_string(archive_path)?));
            }

            // After the run has finished, clean any stale archives
            Event::RunFinished { .. } => {
                workspace
                    .cache
                    .clean_stale_cache(&workspace.config.runner.cache_lifetime)
                    .await?;
            }
            _ => {}
        }

        Ok(EventFlow::Continue)
    }
}
