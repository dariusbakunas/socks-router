use chrono::DateTime;
use std::io::Write;
use std::process::Command;

fn run_git_command(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn parse_commit_timestamp_to_date(timestamp: &str) -> String {
    timestamp
        .parse::<i64>()
        .ok()
        .and_then(|ts| {
            let native = DateTime::from_timestamp(ts, 0)?;
            Some(native.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get the current Git SHA using `git rev-parse`
    let git_sha = run_git_command(&["rev-parse", "HEAD"]);
    let commit_timestamp = run_git_command(&["log", "-1", "--format=%ct"]);
    let commit_date = parse_commit_timestamp_to_date(&commit_timestamp);

    let app_version = format!(
        "{} (Git SHA: {}, Commit Date: {})",
        env!("CARGO_PKG_VERSION"),
        git_sha,
        commit_date
    );

    writeln!(
        std::io::stdout(),
        "cargo:rustc-env=APP_VERSION={}",
        app_version
    )?;

    Ok(())
}
