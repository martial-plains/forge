// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

pub(super) fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
pub(super) fn env() -> &'static Env {
    static ENV: OnceLock<Env> = OnceLock::new();
    ENV.get_or_init(|| {
        let tz = run("date", &["+%z"])
            .and_then(|z| {
                let sign = if z.starts_with('-') { -1 } else { 1 };
                let digits = z.trim_start_matches(['+', '-']);
                let (h, m) = (
                    digits.get(0..2)?.parse::<i64>().ok()?,
                    digits.get(2..4)?.parse::<i64>().ok()?,
                );
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0);
        #[cfg(unix)]
        let (users, groups) = {
            let mut users = HashMap::new();
            // macOS keeps ordinary users out of /etc/passwd, so ask `id` too.
            if let Ok(passwd) = fs::read_to_string("/etc/passwd") {
                for line in passwd.lines() {
                    let mut f = line.split(':');
                    if let (Some(name), _, Some(uid)) = (f.next(), f.next(), f.next())
                        && let Ok(uid) = uid.parse()
                    {
                        users.entry(uid).or_insert_with(|| name.to_string());
                    }
                }
            }
            if let (Some(uid), Some(name)) = (run("id", &["-u"]), run("id", &["-un"]))
                && let Ok(uid) = uid.parse()
            {
                users.insert(uid, name);
            }
            let mut groups = HashMap::new();
            if let Ok(text) = fs::read_to_string("/etc/group") {
                for line in text.lines() {
                    let mut f = line.split(':');
                    if let (Some(name), _, Some(gid)) = (f.next(), f.next(), f.next())
                        && let Ok(gid) = gid.parse()
                    {
                        groups.entry(gid).or_insert_with(|| name.to_string());
                    }
                }
            }
            // macOS keeps its groups in the directory service too.
            if let (Some(gid), Some(name)) = (run("id", &["-g"]), run("id", &["-gn"]))
                && let Ok(gid) = gid.parse()
            {
                groups.entry(gid).or_insert(name);
            }
            (users, groups)
        };
        #[cfg(unix)]
        {
            Env { tz, users, groups }
        }
        #[cfg(not(unix))]
        {
            Env { tz }
        }
    })
}
pub(super) fn icons_enabled() -> bool {
    !std::env::var("FORGE_DIRED_ICONS")
        .is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "no" | "off"))
}

// ------------------------------------------------------------ formatting
