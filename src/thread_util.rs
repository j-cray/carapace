use std::io;
use std::thread;

use thiserror::Error;

pub(crate) type NamedThreadRoutine = Box<dyn FnOnce() + Send + 'static>;
pub(crate) type NamedThreadSpawner =
    fn(thread::Builder, NamedThreadRoutine) -> io::Result<thread::JoinHandle<()>>;

pub(crate) fn spawn_named_thread(
    builder: thread::Builder,
    routine: NamedThreadRoutine,
) -> io::Result<thread::JoinHandle<()>> {
    builder.spawn(routine)
}

#[derive(Debug, Error)]
#[error("failed to spawn startup thread '{thread_name}': {source}")]
pub struct StartupThreadSpawnError {
    pub(crate) thread_name: &'static str,
    #[source]
    pub(crate) source: io::Error,
}

impl StartupThreadSpawnError {
    pub(crate) fn new(thread_name: &'static str, source: io::Error) -> Self {
        Self {
            thread_name,
            source,
        }
    }
}

pub(crate) fn spawn_startup_named_thread(
    thread_name: &'static str,
    routine: NamedThreadRoutine,
) -> Result<thread::JoinHandle<()>, StartupThreadSpawnError> {
    spawn_startup_named_thread_with_spawner(thread_name, routine, spawn_named_thread)
}

pub(crate) fn spawn_startup_named_thread_with_spawner(
    thread_name: &'static str,
    routine: NamedThreadRoutine,
    spawner: NamedThreadSpawner,
) -> Result<thread::JoinHandle<()>, StartupThreadSpawnError> {
    spawner(
        thread::Builder::new().name(thread_name.to_string()),
        routine,
    )
    .map_err(|source| StartupThreadSpawnError::new(thread_name, source))
}
