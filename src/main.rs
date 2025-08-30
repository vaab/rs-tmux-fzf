use std::{env, fs};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{PathBuf, Path};
use std::process::{Command, Stdio};


macro_rules! cmd {
    ($bin:ident) => {
        pub mod $bin {
            static CACHE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
            #[inline]
            pub fn path() -> &'static std::path::Path {
                CACHE.get_or_init(|| super::depends(stringify!($bin))).as_path()
            }
        }
    };
}

cmd!(tmux);
cmd!(fzf);


fn die(msg: &str) -> ! {
    eprintln!("{}", msg);
    std::process::exit(1)
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let cand = dir.join(cmd);
        if let Ok(m) = fs::metadata(&cand) {
            if m.is_file() && (m.permissions().mode() & 0o111 != 0) {
                return Some(cand);
            }
        }
    }
    None
}

pub fn tmux_session_list(current: &str) -> Vec<String> {
    const FMT: &str = "#{?session_attached,0,1} #{?session_last_attached,,0}#{session_last_attached} #{session_name}";

    let s = wrap(tmux::path(), &["list-sessions", "-F", FMT]);

    // Collect (attached_flag, last_attached_ts, name)
    let mut rows: Vec<(u8, u64, String)> = Vec::new();

    for line in s.lines() {
        let mut it = line.split_whitespace();

        let attached = it.next().and_then(|x| x.parse::<u8>().ok()).unwrap_or(1);
        let last_attached = it.next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
        let name = it.next().and_then(|x| Some(x.to_string())).unwrap_or_default();

        if !name.is_empty() && name != current {
            rows.push((attached, last_attached, name));
        }
    }

    rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    rows.into_iter().map(|(_, _, n)| n).collect()
}

fn fzf_pick(prompt: &str, items: &[String]) -> Option<String> {
    if items.is_empty() {
        return None;
    }

    let mut child = Command::new(&fzf::path())
        .args(["--no-multi", "--prompt", prompt])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;

    {
        let mut sin = child.stdin.take().unwrap();
        // write once; avoid per-line flush overhead
        let mut buf = Vec::with_capacity(items.iter().map(|s| s.len() + 1).sum());
        for it in items {
            buf.extend_from_slice(it.as_bytes());
            buf.push(b'\n');
        }
        let _ = sin.write_all(&buf);
    }

    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    } // Esc / Ctrl-G → none
    let sel = String::from_utf8_lossy(&out.stdout);
    sel.lines()
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn wrap(cmd_path: &Path, args: &[&str]) -> String {
    let out = Command::new(cmd_path)
        .args(args)
        .output()
        .unwrap_or_else(|e| die(&format!("spawn failed: {e}")));
    if !out.status.success() {
        die("command failed");
    }
    String::from_utf8_lossy(&out.stdout).to_string()
}


const USAGE: &str = "usage: rs-tmux-fzf {switch-from|move-window} <current_session> [<current_window>]";

fn depends(name: &str) -> PathBuf {
    if let Some(name) = which(name) {
        return name
    }
    die(&format!(
        "{}: `{}` not found in PATH",
        env!("CARGO_PKG_NAME"),
        name
    ));
}

fn main() {

    // dependencies and path resolution

    tmux::path();
    fzf::path();

    // argument parsing

    let mut args = env::args().skip(1);
    let action = args.next().unwrap_or_else(|| {
        die(USAGE);
    });

    match action.as_str() {
        "switch-from" => {
            let current_session = args.next().unwrap_or_else(|| {
                die("You must provide the current session name as the second argument");
                // wrap(tmux::path(), ["display-message", "-p", "#{client_session}"])
            });
            let list = tmux_session_list(&current_session);
            if let Some(target) = fzf_pick("switch to session> ", &list) {
                let _ = wrap(tmux::path(), &[
                    "switch-client", "-t", &target, ";",
                    "refresh-client", "-S"
                ]);
            }
        }
        "move-window" => {
            let current_session = args.next().unwrap_or_else(|| {
                die("You must provide the current session name as the second argument");
                // wrap(tmux::path(), ["display-message", "-p", "#{client_session}"])
            });
            let current_window = args.next().unwrap_or_else(|| {
                die("You must provide the current window id as the third argument");
                // wrap(tmux::path(), ["display-message", "-p", "#{window_id}"])
            });

            let list = tmux_session_list(&current_session);
            if let Some(target) = fzf_pick("move window to> ", &list) {
                let _ = wrap(tmux::path(), &[
                    "move-window", "-t", &format!("{target}:"), ";",
                    "switch-client", "-t", &target, ";",
                    "select-window", "-t", &current_window,
                ]);
            }
        }
        _ => die(USAGE),
    }
}
