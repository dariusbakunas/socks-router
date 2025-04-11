use log::{debug, error};
use std::collections::HashMap;
use std::process::ExitStatus;
use std::sync::Arc;
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

#[derive(Debug)]
pub struct CommandProcessTracker {
    running_processes: RwLock<HashMap<String, Arc<Mutex<CommandProcess>>>>,
}

#[derive(Debug)]
pub struct CommandProcess {
    pub command: String,
    pub child: Option<Child>,
}

impl Default for CommandProcessTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandProcessTracker {
    pub fn new() -> Self {
        CommandProcessTracker {
            running_processes: RwLock::new(HashMap::new()),
        }
    }

    // Checks if a process is already running for the given command
    pub async fn is_running(&self, command: &str) -> bool {
        let processes = self.running_processes.read().await;
        if let Some(process) = processes.get(command) {
            if let Some(child) = &process.lock().await.child {
                return child.id().is_some(); // Check process ID
            }
        }
        false
    }

    // Adds process to running processes list
    pub async fn add_process(&self, command: String, child: Child) {
        let mut processes = self.running_processes.write().await;
        processes.insert(
            command.clone(),
            Arc::new(Mutex::new(CommandProcess {
                command,
                child: Some(child), // Attach process handle
            })),
        );
    }

    // Removes a completed or terminated process
    pub async fn remove_process(&self, command: &str) {
        let mut processes = self.running_processes.write().await;
        processes.remove(command);
    }

    // Checks the exit status of a process for a command
    pub async fn check_process_status(&self, command: &str) -> Option<ExitStatus> {
        let processes = self.running_processes.read().await;
        if let Some(process) = processes.get(command) {
            let mut process_lock = process.lock().await;
            if let Some(ref mut child) = process_lock.child {
                // Try to check the exit status if the process has completed
                return child.try_wait().ok().flatten();
            }
        }
        None
    }

    // Periodically clean up terminated processes
    pub async fn cleanup(&self) {
        let mut to_remove = Vec::new();
        {
            let processes = self.running_processes.read().await;
            for (command, process) in processes.iter() {
                let status = {
                    let mut process_lock = process.lock().await;
                    if let Some(ref mut child) = process_lock.child {
                        child.try_wait().ok().flatten()
                    } else {
                        None
                    }
                };

                if status.is_some() {
                    debug!("Process for command `{}` has exited.", command);
                    to_remove.push(command.clone());
                }
            }
        }

        // Remove terminated processes
        if !to_remove.is_empty() {
            let mut processes = self.running_processes.write().await;
            for command in to_remove {
                processes.remove(&command);
            }
        }
    }

    /// Stops all running child processes
    pub async fn stop_all_processes(&self) {
        let mut tasks = vec![];

        // Iterate over all running processes and terminate them
        let processes = self.running_processes.read().await;
        for (command, process) in processes.iter() {
            let command = command.clone();
            let process = process.clone();

            let task = tokio::spawn(async move {
                let mut process_lock = process.lock().await;
                if let Some(ref mut child) = process_lock.child {
                    debug!(
                        "Stopping process for command `{}` (PID {}).",
                        command,
                        child.id().unwrap_or(0)
                    );
                    if let Err(err) = child.kill().await {
                        error!(
                            "Failed to kill process for command `{}` (PID {}): {:?}",
                            command,
                            child.id().unwrap_or(0),
                            err
                        );
                    }
                }
            });

            tasks.push(task);
        }

        // Await on all kill tasks
        for task in tasks {
            let _ = task.await; // Handle task errors if needed
        }

        debug!("All processes have been stopped.");
    }
}
