//! `cargo targone schedule` — set-and-forget recurrence via the OS scheduler
//! (spike 0.6): per-user Task Scheduler on Windows (daily + only-if-idle,
//! non-elevated), systemd user timer on Linux, launchd agent on macOS.
//! One fixed identity per platform; registration is overwrite-style so
//! re-running `install` is always safe; `uninstall` is the exact inverse.

use std::path::PathBuf;
use std::process::Command;

pub const TASK_NAME: &str = "Targone";

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot resolve own executable: {e}"))
}

#[cfg(windows)]
fn powershell(script: &str) -> Result<String, String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| format!("cannot run powershell: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[cfg(windows)]
pub fn install() -> Result<String, String> {
    let exe = current_exe()?;
    // Spike 0.6: the cmdlet path (Task Scheduler COM API) accepts
    // daily + RunOnlyIfIdle non-elevated — schtasks.exe cannot express it.
    // Defaults are battery-hostile, so battery settings are set explicitly;
    // StartWhenAvailable catches up missed runs.
    let script = format!(
        "$a = New-ScheduledTaskAction -Execute '{exe}' -Argument 'targone schedule run'; \
         $t = New-ScheduledTaskTrigger -Daily -At 3am; \
         $s = New-ScheduledTaskSettingsSet -RunOnlyIfIdle -IdleDuration 00:10:00 -IdleWaitTimeout 02:00:00 \
              -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable; \
         Register-ScheduledTask -TaskName '{name}' -Action $a -Trigger $t -Settings $s -Force | Out-Null; \
         (Get-ScheduledTask -TaskName '{name}').State",
        exe = exe.display(),
        name = TASK_NAME
    );
    let state = powershell(&script)?;
    Ok(format!(
        "registered per-user task '{TASK_NAME}' (daily 03:00, only-if-idle, catch-up on missed runs): {}",
        state.trim()
    ))
}

#[cfg(windows)]
pub fn uninstall() -> Result<String, String> {
    let script = format!(
        "Unregister-ScheduledTask -TaskName '{TASK_NAME}' -Confirm:$false -ErrorAction SilentlyContinue; \
         if (Get-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue) {{ 'still present' }} else {{ 'removed' }}"
    );
    let out = powershell(&script)?;
    Ok(format!("task '{TASK_NAME}': {}", out.trim()))
}

#[cfg(windows)]
pub fn status() -> Result<String, String> {
    let script = format!(
        "$t = Get-ScheduledTask -TaskName '{TASK_NAME}' -ErrorAction SilentlyContinue; \
         if ($t) {{ $i = $t | Get-ScheduledTaskInfo; \
           \"state={{0}} last-run={{1}} last-result={{2}} next-run={{3}}\" -f $t.State, $i.LastRunTime, $i.LastTaskResult, $i.NextRunTime }} \
         else {{ 'not installed' }}"
    );
    powershell(&script).map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
fn systemd_user_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

#[cfg(target_os = "linux")]
pub fn install() -> Result<String, String> {
    let exe = current_exe()?;
    let dir = systemd_user_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let service = format!(
        "[Unit]\nDescription=Targone sweep\n\n[Service]\nType=oneshot\n\
         ExecStart={} targone schedule run\nNice=19\nIOSchedulingClass=idle\nCPUSchedulingPolicy=idle\n",
        exe.display()
    );
    let timer = "[Unit]\nDescription=Daily Targone sweep\n\n[Timer]\nOnCalendar=daily\n\
         RandomizedDelaySec=1h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n";
    std::fs::write(dir.join("targone-sweep.service"), service).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("targone-sweep.timer"), timer).map_err(|e| e.to_string())?;
    let ok = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .and_then(|_| {
            Command::new("systemctl")
                .args(["--user", "enable", "--now", "targone-sweep.timer"])
                .status()
        })
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok("systemd user timer 'targone-sweep.timer' enabled (daily, persistent)".into())
    } else {
        Ok(
            "unit files written, but `systemctl --user` failed — no user systemd? \
            Targone will still sweep opportunistically on manual runs"
                .into(),
        )
    }
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> Result<String, String> {
    let dir = systemd_user_dir()?;
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "targone-sweep.timer"])
        .status();
    let _ = std::fs::remove_file(dir.join("targone-sweep.timer"));
    let _ = std::fs::remove_file(dir.join("targone-sweep.service"));
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    Ok("systemd user units removed".into())
}

#[cfg(target_os = "linux")]
pub fn status() -> Result<String, String> {
    let out = Command::new("systemctl")
        .args(["--user", "is-enabled", "targone-sweep.timer"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn plist_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents/dev.targone.sweep.plist"))
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<String, String> {
    let exe = current_exe()?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>dev.targone.sweep</string>
  <key>ProgramArguments</key><array>
    <string>{exe}</string><string>targone</string><string>schedule</string><string>run</string>
  </array>
  <key>StartCalendarInterval</key><dict><key>Hour</key><integer>3</integer><key>Minute</key><integer>0</integer></dict>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>19</integer>
</dict></plist>
"#,
        exe = exe.display()
    );
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, plist).map_err(|e| e.to_string())?;
    let uid = Command::new("id").arg("-u").output().ok();
    let uid = uid
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // bootout-then-bootstrap: bootstrap of an already-loaded service errors.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/dev.targone.sweep")])
        .status();
    let ok = Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{uid}"),
            path.to_str().unwrap_or_default(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    Ok(if ok {
        "launchd agent dev.targone.sweep loaded (daily 03:00)".into()
    } else {
        "plist written, but launchctl bootstrap failed — Targone will still \
         sweep opportunistically on manual runs"
            .into()
    })
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<String, String> {
    let uid = Command::new("id").arg("-u").output().ok();
    let uid = uid
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/dev.targone.sweep")])
        .status();
    let _ = std::fs::remove_file(plist_path()?);
    Ok("launchd agent removed".into())
}

#[cfg(target_os = "macos")]
pub fn status() -> Result<String, String> {
    Ok(if plist_path()?.exists() {
        "plist installed".into()
    } else {
        "not installed".into()
    })
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn install() -> Result<String, String> {
    Err("no scheduler integration for this platform — run `cargo targone gc --apply` manually or from cron".into())
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn uninstall() -> Result<String, String> {
    Ok("nothing installed".into())
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn status() -> Result<String, String> {
    Ok("not supported on this platform".into())
}

/// True when a scheduled run must be a hard no-op (F-062.10).
pub fn disabled() -> Option<&'static str> {
    if std::env::var_os("TARGONE_DISABLE").is_some_and(|v| v == "1") {
        return Some("TARGONE_DISABLE=1");
    }
    if std::env::var_os("CI").is_some() {
        return Some("CI environment");
    }
    None
}
