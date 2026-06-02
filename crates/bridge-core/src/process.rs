// supervisor.rs — subprocess supervisor with process-group SIGTERM/SIGKILL on terminate().
// Spec §9 + S3, Codex finding 6: SIGKILL targets the whole group so TERM-ignoring children
// and their descendants cannot survive.

use std::process::Stdio;
use tokio::process::{Child, Command};

pub struct Supervised {
    child: Child,
    pid: u32,
}

impl Supervised {
    pub fn spawn(
        prog: &str,
        args: &[&str],
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0) // child becomes its own group leader (pgid == child pid)
            .kill_on_drop(true);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id().expect("child has a pid before wait");
        // Drain the child's stderr on a detached task. stderr is piped (so it never
        // interleaves with our own logs) but NOTHING else reads it: an agent that
        // writes past the ~64KB pipe buffer blocks on its next stderr write and
        // deadlocks its entire turn (observed live with a chatty ACP agent — the
        // turn hung, then the process died as AgentCrashed). Reading to EOF keeps
        // the pipe drained; lines surface at debug under `agent_stderr` so
        // `RUST_LOG=agent_stderr=debug` shows agent diagnostics on demand.
        if let Some(stderr) = child.stderr.take() {
            let agent = prog.to_string();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "agent_stderr", %agent, "{line}");
                }
            });
        }
        Ok(Self { child, pid })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Task 8 reads stdout/stdin via this.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// SIGTERM the group, wait `grace`, SIGKILL the group, reap.
    pub async fn terminate(mut self, grace: std::time::Duration) {
        let pgid = self.pid as i32;
        // SAFETY: kill(-pgid, sig) sends signal to a process group we own.
        // pgid is the child's pid, which equals the group leader pid because
        // we spawned with process_group(0).
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        if tokio::time::timeout(grace, self.child.wait())
            .await
            .is_err()
        {
            // Grace period elapsed — escalate to SIGKILL on the whole group.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        // Always reap to prevent zombies.
        let _ = self.child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // returns the `stat`/`state` column for a pid, or None if the pid is gone/reaped.
    fn proc_state(pid: u32) -> Option<String> {
        let out = std::process::Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    #[tokio::test]
    async fn chatty_stderr_does_not_deadlock_the_turn() {
        // Regression: stderr was piped but never drained, so a child writing past
        // the ~64KB pipe buffer blocked on its next stderr write and never produced
        // stdout (a live ACP agent hung this way, then died as AgentCrashed). The
        // child below writes ~320KB to stderr, THEN a sentinel to stdout. If stderr
        // is undrained the stdout read hangs; with draining it returns "DONE".
        use tokio::io::AsyncReadExt;
        let mut sup = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -lt 8000 ]; do \
                 echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx 1>&2; \
                 i=$((i+1)); done; echo DONE",
            ],
            None,
        )
        .unwrap();
        let mut out = sup.child_mut().stdout.take().unwrap();
        let mut buf = String::new();
        let read =
            tokio::time::timeout(Duration::from_secs(10), out.read_to_string(&mut buf)).await;
        assert!(
            read.is_ok(),
            "stdout read timed out — child blocked on an undrained stderr pipe"
        );
        assert!(
            buf.contains("DONE"),
            "child never finished; stdout was {buf:?}"
        );
    }

    #[tokio::test]
    async fn terminate_reaps_child_no_zombie() {
        let child = Supervised::spawn("/bin/sh", &["-c", "sleep 30"], None).unwrap();
        let pid = child.pid();
        assert!(proc_state(pid).is_some()); // alive
        child.terminate(Duration::from_millis(300)).await;
        // after terminate+reap, pid is gone (None) — NOT a zombie ('Z' state)
        match proc_state(pid) {
            None => {} // reaped, good
            Some(s) => assert!(!s.starts_with('Z'), "left a zombie: {s}"),
        }
    }

    #[tokio::test]
    async fn term_ignoring_child_with_descendant_is_group_killed() {
        // parent ignores TERM and has a child that sleeps; only a GROUP kill gets both.
        let child = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "trap '' TERM; sleep 30 & echo $! > /tmp/a2a_desc_pid.$$; wait",
            ],
            None,
        )
        .unwrap();
        let pid = child.pid();
        tokio::time::sleep(Duration::from_millis(200)).await; // let descendant spawn
        let desc = std::fs::read_to_string(format!("/tmp/a2a_desc_pid.{pid}"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        child.terminate(Duration::from_millis(300)).await; // grace then SIGKILL group
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            proc_state(pid).is_none_or(|s| s.starts_with('Z')),
            "leader still running"
        );
        if let Some(d) = desc {
            assert!(
                proc_state(d).is_none(),
                "descendant {d} survived group kill"
            );
        }
        let _ = std::fs::remove_file(format!("/tmp/a2a_desc_pid.{pid}"));
    }

    #[tokio::test]
    async fn term_ignoring_loop_forces_group_sigkill() {
        // sh ignores TERM and re-spawns short sleeps in a loop: group-SIGTERM kills the
        // current `sleep` but the loop (sh ignores TERM) survives the grace window, so only
        // the SIGKILL group escalation can reap it.
        let child = Supervised::spawn(
            "/bin/sh",
            &["-c", "trap '' TERM; while :; do sleep 0.2; done"],
            None,
        )
        .unwrap();
        let pid = child.pid();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            proc_state(pid).is_some(),
            "leader should be alive before terminate"
        );
        // short grace -> SIGTERM ignored by the loop -> timeout -> SIGKILL group
        child.terminate(Duration::from_millis(250)).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            proc_state(pid).is_none(),
            "group SIGKILL should have reaped the TERM-ignoring leader"
        );
    }
}
