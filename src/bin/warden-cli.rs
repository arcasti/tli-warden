//! The command line: `warden-cli` is the console build, so it prints to the
//! terminal it runs from and the shell waits for its exit code. `warden` is
//! the window.
//!
//!   warden-cli solve <board.json | frame.png> [--rollouts N] [--seed S] [--tools id,id]
//!   warden-cli read <frame.png> [--tools id,id]
//!   warden-cli capture [--frames N] <title text> <out.png>
//!   warden-cli bench [--games N] [--rollouts R] [--seed S] [--threads T]
//!   warden-cli bench --strategy-only [--set name=value,...] [--hold id,id] [--save file] [--against file] [--games N] [--seed S] [--threads T]
//!   warden-cli --version

use std::path::Path;
use std::process::ExitCode;
use warden::board::{Model, build_state};
use warden::engine::data::Tool;
use warden::reader::{read_model, save_png};
use warden::solver::bench::{LOW, Solo, bench, mean_se, spread, strategy_bench, summarize};
use warden::solver::{Params, TUNED};
use warden::solver::solve::{FollowUp, follow_up, instant, solve};

/// println! that ignores a closed pipe (`| head`) instead of aborting.
macro_rules! out {
    () => { out!("") };
    ($($t:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

const USAGE: &str = "usage: warden-cli solve <board.json | frame.png> [--rollouts N] [--seed S] [--tools id,id] | warden-cli read <frame.png> [--tools id,id] | warden-cli capture [--frames N] <title text> <out.png> | warden-cli bench [--games N] [--rollouts R] [--seed S] [--threads T] | warden-cli bench --strategy-only [--set name=value,...] [--hold id,id] [--save file] [--against file] [--games N] [--seed S] [--threads T] | warden-cli --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") if args.len() == 1 => {
            out!("warden {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(cmd @ ("solve" | "read" | "capture" | "bench")) => {
            let run = match cmd {
                "solve" => run_solve(&args[1..]),
                "read" => run_read(&args[1..]),
                "bench" => run_bench(&args[1..]),
                _ => run_capture(&args[1..]),
            };
            run.unwrap_or_else(|message| {
                eprintln!("{message}");
                ExitCode::from(1)
            })
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(1)
        }
    }
}

fn number<T: std::str::FromStr>(flag: &str, value: Option<&String>) -> Result<T, String> {
    value.and_then(|v| v.parse().ok()).ok_or_else(|| format!("{flag} needs a number; {USAGE}"))
}

/// `--tools id,id`: instrument keys such as coin or chains.
fn tools(value: Option<&String>) -> Result<Vec<Tool>, String> {
    let value = value.ok_or_else(|| format!("--tools needs instrument ids; {USAGE}"))?;
    value
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| Tool::from_key(s.trim()).ok_or_else(|| format!("unknown instrument id {s}")))
        .collect()
}

fn is_png(file: &str) -> bool {
    Path::new(file).extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

/// `capture [--frames N] <title text> <out.png>`: saves the client area of the
/// first visible window whose title contains the text, as the reader takes it.
/// With `--frames` it grabs N frames through one capture session, as auto read
/// does, and prints the mean time per frame after the first.
fn run_capture(args: &[String]) -> Result<ExitCode, String> {
    let (mut frames, mut rest) = (None, Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--frames" => frames = Some(number::<usize>("--frames", it.next())?),
            _ => rest.push(arg),
        }
    }
    let [title, out] = rest[..] else {
        return Err(USAGE.to_string());
    };
    let img = match frames {
        None => warden::app::capture::capture(title)?,
        Some(n) => {
            let (img, times) = warden::app::capture::time_frames(title, n)?;
            let ms = |d: &std::time::Duration| d.as_secs_f64() * 1000.0;
            let after = &times[1..];
            out!("first frame {:.1} ms (opens the session)", ms(&times[0]));
            if !after.is_empty() {
                out!("{} more frames, {:.1} ms per frame (grab and fingerprint)", after.len(), after.iter().map(ms).sum::<f64>() / after.len() as f64);
            }
            img
        }
    };
    save_png(Path::new(out), &img)?;
    out!("{}x{} saved to {out}", img.w, img.h);
    Ok(ExitCode::SUCCESS)
}

/// `bench`: Chromatics and loot per run for the strategy alone and following the
/// advice, on the same seeds; see `warden::solver::bench`. With
/// `--strategy-only`, the strategy alone; see `run_strategy_bench`.
fn run_bench(args: &[String]) -> Result<ExitCode, String> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let (mut games, mut rollouts, mut seed, mut threads) = (None, 400usize, 1u32, threads);
    let (mut strategy_only, mut changed) = (false, None::<Params>);
    let (mut held, mut save, mut against) = (Vec::new(), None::<&String>, None::<&String>);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--games" => games = Some(number("--games", it.next())?),
            "--rollouts" => rollouts = number("--rollouts", it.next())?,
            "--seed" => seed = number("--seed", it.next())?,
            "--threads" => threads = number("--threads", it.next())?,
            "--strategy-only" => strategy_only = true,
            "--set" => set_weights(changed.get_or_insert_with(|| TUNED.clone()), it.next())?,
            "--hold" => held = tools(it.next())?,
            "--save" => save = Some(it.next().ok_or_else(|| format!("--save needs a file; {USAGE}"))?),
            "--against" => against = Some(it.next().ok_or_else(|| format!("--against needs a file; {USAGE}"))?),
            _ => return Err(format!("unexpected argument {arg}; {USAGE}")),
        }
    }
    if !strategy_only && (changed.is_some() || !held.is_empty() || save.is_some() || against.is_some()) {
        return Err(format!("--set, --hold, --save and --against need --strategy-only; {USAGE}"));
    }
    let games = games.unwrap_or(if strategy_only { 2000 } else { 100 });
    if games == 0 || rollouts == 0 || threads == 0 {
        return Err(format!("--games, --rollouts and --threads must be at least 1; {USAGE}"));
    }
    if strategy_only {
        return run_strategy_bench(games, seed, threads, &held, changed.as_ref(), save, against);
    }
    let clock = std::time::Instant::now();
    let s = summarize(&bench(games, rollouts, seed, threads));
    let secs = clock.elapsed().as_secs_f64();
    out!(
        "strategy  {:.3} +- {:.3} Chromatics, {:.3} +- {:.3} loot per run",
        s.strategy.0,
        s.strategy.1,
        s.strategy_loot.0,
        s.strategy_loot.1
    );
    out!("advice    {:.3} +- {:.3}, {:.3} +- {:.3}", s.advice.0, s.advice.1, s.advice_loot.0, s.advice_loot.1);
    out!("advice - strategy  {:+.3} +- {:.3} loot (paired)", s.gain.0, s.gain.1);
    out!("advice overruled the strategy at {} of {} decisions with 2+ options", s.overruled, s.decisions);
    out!("{games} games, {rollouts} playouts per option, seed {seed}, {threads} threads, {secs:.1} s");
    Ok(ExitCode::SUCCESS)
}

/// `--set name=value,...`: changes strategy weights, by `Params` field name.
fn set_weights(p: &mut Params, value: Option<&String>) -> Result<(), String> {
    let value = value.ok_or_else(|| format!("--set needs name=value pairs; {USAGE}"))?;
    for pair in value.split(',').filter(|s| !s.trim().is_empty()) {
        let (name, x) = pair.split_once('=').ok_or_else(|| format!("--set {pair}: expected name=value"))?;
        let x: f64 = x.trim().parse().map_err(|_| format!("--set {pair}: {x} is not a number"))?;
        *p.weight_mut(name.trim()).ok_or_else(|| format!("--set {pair}: no strategy weight {name}"))? = x;
    }
    Ok(())
}

/// `bench --strategy-only`: Chromatics, loot and moves per run for the strategy on
/// the tuned weights and, given `changed`, on those too, with the paired
/// difference by game. Every game starts holding `held`. `save` writes the
/// tuned Chromatics per game, one per line, and `against` reads such a file
/// from another build and prints the paired difference to it.
fn run_strategy_bench(
    games: u32,
    seed: u32,
    threads: usize,
    held: &[Tool],
    changed: Option<&Params>,
    save: Option<&String>,
    against: Option<&String>,
) -> Result<ExitCode, String> {
    let saved = match against {
        None => None,
        Some(file) => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("--against {file}: {e}"))?;
            let runs = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().parse::<usize>().map_err(|_| format!("--against {file}: {l} is not a Chromatics count")))
                .collect::<Result<Vec<_>, _>>()?;
            if runs.len() != games as usize {
                return Err(format!("--against {file} has {} games, this run {games}", runs.len()));
            }
            Some(runs)
        }
    };
    let clock = std::time::Instant::now();
    let tuned = strategy_bench(games, seed, threads, held, &TUNED);
    let row = |name: &str, runs: &[Solo]| {
        let (mean, se) = mean_se(&runs.iter().map(|r| r.chromatics as f64).collect::<Vec<_>>());
        let (loot, loot_se) = mean_se(&runs.iter().map(|r| r.loot).collect::<Vec<_>>());
        let per_run = |f: fn(&Solo) -> u32| runs.iter().map(|r| f64::from(f(r))).sum::<f64>() / runs.len() as f64;
        out!(
            "{name:<8}  {mean:.3} +- {se:.3} Chromatics, {loot:.3} +- {loot_se:.3} loot per run; per run {:.2} Consumes, {:.2} chain bites, {:.2} Replaces, {:.2} doors opened",
            per_run(|r| r.stats.devours),
            per_run(|r| r.stats.chains),
            per_run(|r| r.stats.swaps),
            per_run(|r| r.stats.opened)
        );
        let s = spread(&runs.iter().map(|r| r.chromatics).collect::<Vec<_>>());
        let [p10, p25, p50, p75, p90] = s.percentiles;
        let done = 100.0 * runs.iter().filter(|r| r.objective).count() as f64 / runs.len().max(1) as f64;
        out!(
            "{:<8}  {:.2}% runs with <= {LOW} Chromatics; p10/p25/p50/p75/p90 {p10}/{p25}/{p50}/{p75}/{p90}, max {}; objective done in {done:.2}%",
            "",
            s.low,
            s.max
        );
    };
    row("tuned", &tuned);
    if let Some(file) = save {
        let text: String = tuned.iter().map(|r| format!("{}\n", r.chromatics)).collect();
        std::fs::write(file, text).map_err(|e| format!("--save {file}: {e}"))?;
    }
    if let Some(saved) = &saved {
        let gain = mean_se(&tuned.iter().zip(saved).map(|(t, &s)| t.chromatics as f64 - s as f64).collect::<Vec<_>>());
        out!("tuned - saved  {:+.3} +- {:.3} (paired)", gain.0, gain.1);
    }
    if let Some(p) = changed {
        let runs = strategy_bench(games, seed, threads, held, p);
        row("changed", &runs);
        let gain = mean_se(&runs.iter().zip(&tuned).map(|(c, t)| c.chromatics as f64 - t.chromatics as f64).collect::<Vec<_>>());
        out!("changed - tuned  {:+.3} +- {:.3} (paired)", gain.0, gain.1);
        let low = |r: &Solo| if r.chromatics <= LOW { 100.0 } else { 0.0 };
        let fewer = mean_se(&runs.iter().zip(&tuned).map(|(c, t)| low(c) - low(t)).collect::<Vec<_>>());
        out!("changed - tuned  {:+.2} +- {:.2} points of runs with <= {LOW} Chromatics (paired)", fewer.0, fewer.1);
        out!("changed weights: {p:?}");
    }
    let holding = if held.is_empty() {
        String::new()
    } else {
        format!(", holding {}", held.iter().map(|t| t.key()).collect::<Vec<_>>().join(","))
    };
    out!("{games} games, seed {seed}, {threads} threads{holding}, {:.1} s", clock.elapsed().as_secs_f64());
    Ok(ExitCode::SUCCESS)
}

fn run_read(args: &[String]) -> Result<ExitCode, String> {
    let (mut file, mut held) = (None, Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tools" => held = tools(it.next())?,
            _ if file.is_none() && !arg.starts_with("--") => file = Some(arg),
            _ => return Err(format!("unexpected argument {arg}; {USAGE}")),
        }
    }
    let (model, warnings) = read_model(Path::new(file.ok_or(USAGE)?), &held)?;
    for w in &warnings {
        eprintln!("{w}");
    }
    out!("{}", serde_json::to_string_pretty(&model).map_err(|e| e.to_string())?);
    Ok(ExitCode::SUCCESS)
}

fn run_solve(args: &[String]) -> Result<ExitCode, String> {
    let (mut file, mut rollouts, mut seed, mut held) = (None, 400usize, 1u32, Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rollouts" => rollouts = number("--rollouts", it.next())?,
            "--seed" => seed = number("--seed", it.next())?,
            "--tools" => held = tools(it.next())?,
            _ if file.is_none() && !arg.starts_with("--") => file = Some(arg),
            _ => return Err(format!("unexpected argument {arg}; {USAGE}")),
        }
    }
    let file = file.ok_or(USAGE)?;
    if rollouts == 0 {
        return Err(format!("--rollouts must be at least 1; {USAGE}"));
    }
    let model: Model = if is_png(file) {
        let (model, warnings) = read_model(Path::new(file), &held)?;
        for w in &warnings {
            out!("{w}");
        }
        if !warnings.is_empty() {
            out!();
        }
        model
    } else {
        let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
        let mut model: Model = serde_json::from_str(&text).map_err(|e| format!("{file} is not JSON: {e}"))?;
        if !held.is_empty() {
            model.tools = held;
        }
        model
    };
    let (st, problems) = build_state(&model, seed);
    if !problems.is_empty() {
        for p in &problems {
            out!("{p}");
        }
        return Ok(ExitCode::from(2));
    }

    match instant(&st) {
        Some(pick) => {
            out!("Pick: {}\n  {}", pick.label, pick.why);
            if let Some(next) = follow_up(&st, &pick.action) {
                match next {
                    FollowUp::Move(next) => out!("Then: {}\n  {}", next.label, next.why),
                    FollowUp::Depends => out!("Then: depends on what this move reveals; read the board again."),
                }
            }
        }
        None => out!("No move: the run is over."),
    }
    let options = solve(&st, rollouts, seed, &TUNED);
    if options.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let width = options.iter().map(|o| o.label.chars().count()).max().unwrap_or(0).max(6);
    out!("\n{:>4}  {:<width$}  {:>6}  {:>6}  {:>5}", "rank", "option", "mean", "gap", "best");
    for (i, o) in options.iter().enumerate() {
        let gap = if i == 0 {
            "-".to_string()
        } else if o.gap_se.is_finite() {
            format!("{:.3}", o.gap_se)
        } else {
            "inf".to_string()
        };
        let mark = if o.policy { " *" } else { "" };
        out!("{:>4}  {:<width$}  {:>6.3}  {:>6}  {:>5.2}{mark}", i + 1, o.label, o.mean, gap, o.best);
    }
    out!("\n{rollouts} playouts per option; gap is the paired standard error of the margin to the leader; * is the strategy's pick.");
    Ok(ExitCode::SUCCESS)
}
