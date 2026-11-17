use crate::cli::timebase;
use crate::commands::Verbosity;
use crate::fuzz::SandboxedFuzzChunk;
use crate::plugin::index::SandboxedScanLibrary;
use crate::validator::SandboxedValidation;
use clap::{Args, ValueEnum};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::process::ExitStatus;
use std::time::Duration;
use wait_timeout::ChildExt;

#[derive(Debug)]
pub struct SandboxConfig {
    pub hide_output: bool,
    pub verbosity: Verbosity,
    pub timeout: Option<Duration>,
}

#[derive(Debug)]
pub enum SandboxError {
    Timeout(Duration),
    Crashed(ExitStatus),
}

#[derive(Serialize, Deserialize, Args)]
pub struct SandboxPayload {
    sandbox_id: String,
    sandbox_data: String,
    output_file: String,
}

pub trait SandboxOperation: Serialize + DeserializeOwned {
    const ID: &'static str;

    type Result: Serialize + DeserializeOwned;

    fn run(&self) -> Self::Result;

    fn run_sandboxed(&self, config: SandboxConfig) -> Result<Self::Result, SandboxError> {
        let output_file = tempfile::Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("Could not create a temporary file path")
            .into_temp_path();

        let mut command = std::process::Command::new(
            std::env::current_exe().expect("Could not get the path to the current executable"),
        );

        command.env(
            "CLAP_VALIDATOR_TIMEBASE",
            timebase()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                .to_string(),
        );
        command.env(
            "CLAP_VALIDATOR_VERBOSITY",
            config.verbosity.to_possible_value().unwrap().get_name(),
        );

        command.arg("sandbox");
        command.arg(Self::ID);
        command.arg(serde_json::to_string(&self).expect("Failed to serialize sandbox data"));
        command.arg(output_file.to_str().unwrap());

        if config.hide_output {
            command.stdout(std::process::Stdio::null());
            command.stderr(std::process::Stdio::null());
        }

        let mut child = command.spawn().expect("Failed to spawn a child process");
        let status = wait_child(&mut child, config.timeout)?;

        if !status.success() {
            return Err(SandboxError::Crashed(status));
        }

        let output =
            std::fs::read_to_string(&output_file).expect("Failed to read the output file from the sandboxed operation");
        let result: Self::Result =
            serde_json::from_str(&output).expect("Failed to deserialize the output from the sandboxed operation");

        Ok(result)
    }
}

fn wait_child(child: &mut std::process::Child, timeout: Option<Duration>) -> Result<ExitStatus, SandboxError> {
    match timeout {
        None => Ok(child.wait().expect("Failed to wait for the child process")),
        Some(timeout) => match child
            .wait_timeout(timeout)
            .expect("Failed to wait for the child process")
        {
            Some(status) => Ok(status),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Err(SandboxError::Timeout(timeout))
            }
        },
    }
}

impl SandboxPayload {
    pub fn dispatch(self) {
        fn dispatch<T: SandboxOperation>(payload: &SandboxPayload) {
            let operation: T =
                serde_json::from_str(&payload.sandbox_data).expect("Failed to deserialize the sandbox data");
            let result = operation.run();
            std::fs::write(
                &payload.output_file,
                serde_json::to_string(&result).expect("Failed to serialize the sandbox result"),
            )
            .expect("Failed to write the sandbox result to the output file");
        }

        match self.sandbox_id.as_str() {
            SandboxedScanLibrary::ID => dispatch::<SandboxedScanLibrary>(&self),
            SandboxedValidation::ID => dispatch::<SandboxedValidation>(&self),
            SandboxedFuzzChunk::ID => dispatch::<SandboxedFuzzChunk>(&self),
            _ => panic!("Unknown sandbox ID"),
        };
    }
}

impl std::error::Error for SandboxError {}
impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SandboxError::Timeout(duration) => write!(f, "Timed out after {} seconds", duration.as_secs()),
            SandboxError::Crashed(status) => write!(f, "{}", status),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn spawn_hung() -> std::process::Child {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("ping");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        command.spawn().expect("failed to spawn hung child")
    }

    fn process_alive(pid: u32) -> bool {
        if cfg!(windows) {
            let output = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                .expect("tasklist failed");
            String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
        } else {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .is_ok_and(|s| s.success())
        }
    }

    #[test]
    fn timeout_kills_hung_child() {
        let mut child = spawn_hung();
        let pid = child.id();
        let result = wait_child(&mut child, Some(Duration::from_millis(200)));
        assert!(matches!(result, Err(SandboxError::Timeout(_))));
        assert!(!process_alive(pid), "hung child {pid} is still running after timeout");
    }
}
