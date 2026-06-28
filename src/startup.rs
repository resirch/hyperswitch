use std::io;
use std::path::PathBuf;
use std::process::Command;

const TASK_NAME: &str = "Hyperswitch";

/// Create or remove the logon scheduled task so it matches `enabled`.
pub fn sync(enabled: bool) -> io::Result<()> {
    if enabled {
        if !is_enabled()? {
            enable()?;
        } else {
            // Refresh the task path if the exe moved.
            enable()?;
        }
    } else if is_enabled()? {
        disable()?;
    }
    Ok(())
}

pub fn is_enabled() -> io::Result<bool> {
    Ok(Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME])
        .status()?
        .success())
}

fn enable() -> io::Result<()> {
    let exe = current_exe()?;
    let tr = format!("\"{}\"", exe.display());
    let ok = Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            TASK_NAME,
            "/TR",
            &tr,
            "/SC",
            "ONLOGON",
            "/RL",
            "HIGHEST",
            "/F",
        ])
        .status()?
        .success();
    if ok {
        Ok(())
    } else {
        Err(io::Error::other("schtasks /Create failed"))
    }
}

fn disable() -> io::Result<()> {
    let ok = Command::new("schtasks")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .status()?
        .success();
    if ok {
        Ok(())
    } else {
        Err(io::Error::other("schtasks /Delete failed"))
    }
}

fn current_exe() -> io::Result<PathBuf> {
    std::env::current_exe()
}
