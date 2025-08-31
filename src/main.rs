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

pub mod current_exe {
    static CACHE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

    #[inline]
    pub fn path() -> &'static std::path::Path {
        CACHE.get_or_init(|| {
            std::env::current_exe().unwrap_or_else(|e| {
                eprintln!("{}: cannot get current exe path: {e}", env!("CARGO_PKG_NAME"));
                std::process::exit(1)
            })
        }).as_path()
    }
}


fn tmux_message(msg: &str) {
    wrap(tmux::path(), &["display-message", "-p", &msg], None);
}

macro_rules! wrap {
    // 2 mandatory args, optional 3rd
    ($cmd:ident, $args:expr $(, $on:expr)? $(,)?) => {
        $crate::wrap($cmd::path(), $args, wrap!(@opt $($on)?))
    };
    (@opt) => { Some(&mut tmux_message) };
    (@opt $on:expr) => { $on };
}



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

    let s = wrap!(tmux, &["list-sessions", "-F", FMT]);

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

enum Choice {
    FromSelection(String),
    New(String)
}

fn fzf_pick(prompt: &str, current_session: &str) -> Option<Choice> {
    let items = tmux_session_list(&current_session);

    let mut child = Command::new(&fzf::path())
        .args([
            "--no-multi", "--print-query",
            "--bind=alt-enter:print-query",
            format!(
                concat!(
                    "--bind=ctrl-k:",
                    "execute(tmux kill-session -t {{1}})+",
                    "reload({} ls-switch-from {})"
                ),
                current_exe::path().to_string_lossy(),
                current_session).as_str(),
            "--prompt", prompt])
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
    let output = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = output.lines();
    let query = lines.next()
        .unwrap_or_else(|| die(&format!("output is missing first line")));
    let selected = lines.next();
    if out.status.success() && selected.is_some() {
        Some(Choice::FromSelection(selected.unwrap().to_string()))
    } else {  // no selection was matched
        if query.is_empty() {
            tmux_message("canceled");
            return None;
        }
        Some(Choice::New(query.to_string()))
    }
}

fn wrap(cmd_path: &Path, args: &[&str], err_display_cb: Option<&mut dyn FnMut(&str)>) -> String {
    let out = Command::new(cmd_path)
        .args(args)
        .output()
        .unwrap_or_else(|e| die(&format!("spawn failed: {e}")));
    if !out.status.success() {
        if let Some(cb) = err_display_cb {
            cb(&String::from_utf8_lossy(&out.stderr));
        }
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
        "ls-switch-from" => {
            let current_session = args.next().unwrap_or_else(|| {
                die("You must provide the current session name as the second argument");
                // wrap!(tmux, ["display-message", "-p", "#{client_session}"])
            });
            let list = tmux_session_list(&current_session);
            // output list to stdout, one per line
            for item in list {
                println!("{}", item);
            }
        }
        "switch-from" => {
            let current_session = args.next().unwrap_or_else(|| {
                die("You must provide the current session name as the second argument");
                // wrap!(tmux, ["display-message", "-p", "#{client_session}"])
            });
            if let Some(choice) = fzf_pick("switch to session> ", &current_session) {
                match choice {
                    Choice::FromSelection(target) => {
                        wrap!(tmux, &[
                            "switch-client", "-t", &target, ";",
                            "refresh-client", "-S"
                        ]);

                    }
                    Choice::New(target) => {
                        wrap!(tmux, &[
                            "new-session", "-d", "-s", &target, ";",
                            "switch-client", "-t", &target, ";",
                            "refresh-client", "-S"
                        ]);
                    }
                }
            }
        }
        "move-window" => {
            let current_session = args.next().unwrap_or_else(|| {
                die("You must provide the current session name as the second argument");
                // wrap!(tmux, ["display-message", "-p", "#{client_session}"])
            });
            let current_window = args.next().unwrap_or_else(|| {
                die("You must provide the current window id as the third argument");
                // wrap!(tmux, ["display-message", "-p", "#{window_id}"])
            });

            if let Some(choice) = fzf_pick("move window to> ", &current_session) {
                match choice {
                    Choice::FromSelection(target) => {
                        wrap!(tmux, &[
                            "move-window", "-t", &format!("{target}:"), ";",
                            "switch-client", "-t", &target, ";",
                            "select-window", "-t", &current_window,
                        ]);
                    }
                    Choice::New(target) => {
                        wrap!(tmux, &[
                            "new-session", "-d", "-s", &target, ";",
                            "move-window", "-s", &current_window, "-t", &format!("{target}:"), ";",
                            "switch-client", "-t", &target, ";",
                            "kill-window", "-t", &format!("{target}:!"),
                        ]);
                    }
                }
            }
        }
        _ => die(USAGE),
    }
}
