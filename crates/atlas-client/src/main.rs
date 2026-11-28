//! atlasd: keeps one login's harnesses in step with the owner's signed releases.
//!
//! Runs as the login (a systemd user service and timer), never as root, and never runs
//! anything it receives.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use atlas_client::config::{Config, Paths, State};
use atlas_client::pass::{self, Fetched};
use atlas_client::{http, release};
use atlas_core::distribution::PairRequest;

const USAGE: &str = "\
usage:
  atlasd pair <hub-url> <code> [--machine NAME] [--adopt DIR]... [--memory-store DIR]
                                                  pair this login with the hub; --adopt lets
                                                  atlasd take over links an older script made
                                                  into DIR (such as ~/Knowledge);
                                                  --memory-store names your own checkout of
                                                  the knowledge repo for memory links
  atlasd run [--dry-run]                          apply the latest signed release
  atlasd status                                   show the pairing and the pending plan";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = home().and_then(|home| {
        let paths = Paths::new(&home);
        match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["pair", hub, code, rest @ ..] => {
                let mut machine = None;
                let mut adopt = Vec::new();
                let mut memory_store = None;
                let mut opts = rest.iter();
                loop {
                    match (opts.next(), opts.next()) {
                        (None, _) => break,
                        (Some(&"--machine"), Some(name)) => machine = Some(*name),
                        (Some(&"--adopt"), Some(dir)) => adopt.push((*dir).to_owned()),
                        (Some(&"--memory-store"), Some(dir)) => {
                            memory_store = Some((*dir).to_owned());
                        }
                        _ => return Err(USAGE.to_owned()),
                    }
                }
                pair(&paths, hub, code, machine, adopt, memory_store)
            }
            ["run"] => run(&paths, false),
            ["run", "--dry-run"] => run(&paths, true),
            ["status"] => status(&paths),
            _ => Err(USAGE.to_owned()),
        }
    });
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("atlasd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| "HOME isn't set to an absolute path".to_owned())
}

fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|f| {
            std::fs::read_to_string(f)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn pair(
    paths: &Paths,
    hub: &str,
    code: &str,
    machine: Option<&str>,
    adopt_dirs: Vec<String>,
    memory_store: Option<String>,
) -> Result<ExitCode, String> {
    let (origin, _) = release::origin_and_rp(hub)?;
    let request = PairRequest {
        code: code.to_owned(),
        hub: origin.clone(),
        machine: machine.map_or_else(hostname, str::to_owned),
        user: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
    };
    let resp = http::pair(&origin, &request)?;
    if release::origin_and_rp(&resp.hub)?.0 != origin {
        return Err(format!(
            "the hub calls itself {}, not {origin}; pair using that address",
            resp.hub
        ));
    }
    if resp.owner_keys.is_empty() {
        return Err("the hub has no owner passkey yet, so no release could be trusted".to_owned());
    }
    // Folders to take over: those given now, plus any from an earlier pairing, whose
    // project roots and memory store are kept too.
    let earlier = Config::load(&paths.config).ok();
    let mut adopt = earlier.as_ref().map_or_else(Vec::new, |c| c.adopt.clone());
    for dir in adopt_dirs {
        if !adopt.contains(&dir) {
            adopt.push(dir);
        }
    }
    Config {
        hub: resp.hub,
        client_id: resp.client_id,
        token: resp.token,
        machine: request.machine.clone(),
        adopt,
        project_roots: earlier
            .as_ref()
            .map_or_else(atlas_client::config::default_project_roots, |c| {
                c.project_roots.clone()
            }),
        memory_store: memory_store.or_else(|| earlier.and_then(|c| c.memory_store)),
    }
    .save(&paths.config)?;
    State {
        owner_keys: resp.owner_keys,
        last_applied: None,
        last_digest: None,
    }
    .save(&paths.state)?;
    println!(
        "Paired {} ({}) with {origin}. Run `atlasd run --dry-run` to see what it would change.",
        request.machine, request.user
    );
    Ok(ExitCode::SUCCESS)
}

fn fetch(config: &Config) -> Fetched {
    match http::release(&config.hub, &config.token) {
        Ok(Some(r)) => Fetched::Release(Box::new(r)),
        Ok(None) => Fetched::Nothing,
        Err(e) => Fetched::Unreachable(e),
    }
}

fn run(paths: &Paths, dry_run: bool) -> Result<ExitCode, String> {
    let config = Config::load(&paths.config)?;
    let state = State::load(&paths.state)?;
    let fetched = fetch(&config);
    let reachable = !matches!(fetched, Fetched::Unreachable(_));
    let pass = pass::run(paths, &config, &state, fetched, dry_run, now());
    for line in &pass.lines {
        println!("{line}");
    }
    for p in &pass.problems {
        eprintln!("problem: {p}");
    }
    if !dry_run {
        if pass.state != state {
            pass.state.save(&paths.state)?;
        }
        if reachable && let Err(e) = http::report(&config.hub, &config.token, &pass.report) {
            eprintln!("problem: couldn't send the report: {e}");
        }
    }
    Ok(if pass.failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn status(paths: &Paths) -> Result<ExitCode, String> {
    let config = Config::load(&paths.config)?;
    let state = State::load(&paths.state)?;
    println!("Hub: {}", config.hub);
    println!("Machine: {}", config.machine);
    println!(
        "Last applied release: {}",
        state
            .last_applied
            .map_or_else(|| "none".to_owned(), |n| n.to_string())
    );
    let names: Vec<&str> = state.owner_keys.iter().map(|k| k.name.as_str()).collect();
    println!("Trusted passkeys: {}", names.join(", "));
    println!("Plan for the hub's latest release:");
    let pass = pass::run(paths, &config, &state, fetch(&config), true, now());
    for line in &pass.lines {
        println!("  {line}");
    }
    for p in &pass.problems {
        println!("  problem: {p}");
    }
    Ok(ExitCode::SUCCESS)
}
