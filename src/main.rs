mod config;
mod headless;
mod herdr;
mod pipeline;
mod stack;
mod util;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;

use config::{Backend, Config};
use stack::{Entry, Stacks};

#[derive(Parser)]
#[command(
    name = "rigg",
    version,
    about = "Pipeline runner for AI coding agents on Herdr"
)]
struct Cli {
    /// Path to a rigg.toml (defaults to the repo root).
    #[arg(long, global = true)]
    config: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a starter rigg.toml into this repo.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Start a new stack and run a pipeline on it in one go.
    New {
        /// Branch and stack name.
        name: String,
        /// The task. Prompted for if omitted.
        prompt: Option<String>,
        #[arg(long)]
        pipeline: Option<String>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        headless: bool,
    },
    /// Append the next branch to a stack and run a pipeline on it.
    Add {
        /// Stack to extend. The new branch is named <stack>-<n>.
        stack: String,
        /// The task. Prompted for if omitted.
        prompt: Option<String>,
        #[arg(long)]
        pipeline: Option<String>,
        /// Name the new branch explicitly instead of <stack>-<n>.
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        headless: bool,
    },
    /// Print where a stack lives, or focus its session under herdr.
    Attach {
        /// Stack or branch name (default: whatever the cwd is in).
        target: Option<String>,
        /// Print only the path, for `cd "$(rigg attach --path billing)"`.
        #[arg(long)]
        path: bool,
    },
    /// Send a follow-up prompt to a stack's agent session.
    Say {
        /// Stack or branch to talk to.
        target: String,
        message: String,
        /// Agent role (default: the first one configured).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Run the pipeline.
    Run {
        /// The task handed to the first agent step.
        #[arg(long)]
        task: Option<String>,
        /// Named pipeline to run (default: the top-level steps).
        #[arg(long)]
        pipeline: Option<String>,
        /// Branch to diff against (defaults to the stack base or trunk).
        #[arg(long)]
        base: Option<String>,
        /// Start from this step id, skipping earlier ones.
        #[arg(long)]
        from: Option<String>,
        /// Run only these step ids.
        #[arg(long)]
        only: Vec<String>,
        /// Print what would happen without touching any agent.
        #[arg(long)]
        dry_run: bool,
        /// Drive every agent step headlessly, whatever the config says.
        #[arg(long)]
        headless: bool,
    },
    /// Show agents, branch, and stack state.
    Status,
    /// Check that the environment can actually drive agents.
    Doctor {
        /// Check the headless backend, whatever the config says.
        #[arg(long)]
        headless: bool,
    },
    /// Entry point for the Herdr plugin action: opens a pane and runs the pipeline.
    #[command(hide = true)]
    PluginRun,
    /// Manage a stack of dependent branches.
    Stack {
        #[command(subcommand)]
        cmd: StackCmd,
    },
}

#[derive(Subcommand)]
enum StackCmd {
    /// Branch off a stack's tip in a new worktree. With no stack to continue,
    /// this starts a new one, so parallel stacks off the trunk just work.
    Push {
        branch: String,
        /// Root this branch here instead, starting a new stack.
        #[arg(long)]
        base: Option<String>,
        /// Stack to push onto. Defaults to the stack holding the current
        /// branch, or a new stack named after `branch`.
        #[arg(long)]
        stack: Option<String>,
    },
    /// List the stack, bottom to top.
    List,
    /// Print stack names, one per line, for shell completion.
    Names,
    /// Remove worktrees whose branch has already landed on the trunk.
    Prune {
        /// Ref to test against (default: origin/<trunk>, else <trunk>).
        #[arg(long)]
        base: Option<String>,
        /// Actually remove them. Without this it only lists candidates.
        #[arg(long)]
        yes: bool,
    },
    /// Open a PR for a stack entry against its own base.
    Pr {
        #[arg(long)]
        branch: Option<String>,
    },
}

/// Subcommands that can be prefixed with a pipeline name.
const PIPELINE_VERBS: [&str; 3] = ["new", "add", "run"];

/// Accept `rigg <pipeline> new <name> "<prompt>"` by rewriting it into
/// `rigg new <name> "<prompt>" --pipeline <pipeline>`.
fn rewrite_pipeline_prefix(mut argv: Vec<String>) -> Vec<String> {
    if argv.len() < 3 {
        return argv;
    }
    let first = argv[1].clone();
    if first.starts_with('-') || !PIPELINE_VERBS.contains(&argv[2].as_str()) {
        return argv;
    }
    argv.remove(1);
    argv.push("--pipeline".into());
    argv.push(first);
    argv
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("rigg: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse_from(rewrite_pipeline_prefix(std::env::args().collect()));

    if matches!(cli.cmd, Cmd::PluginRun) {
        return plugin_run();
    }

    let root = util::repo_root()?;

    match cli.cmd {
        Cmd::PluginRun => unreachable!("handled above"),
        Cmd::Init { force } => {
            let path = Config::path_for(&root);
            if path.exists() && !force {
                bail!("{} already exists (use --force)", path.display());
            }
            std::fs::write(&path, config::TEMPLATE)?;
            println!("wrote {}", path.display());
            println!("Edit the steps, then: rigg run --task \"...\"");
        }

        Cmd::Doctor { headless } => doctor(&root, cli.config.as_deref(), headless)?,

        Cmd::Status => {
            let branch = util::current_branch(&root)?;
            println!("repo   {}", root.display());
            println!("branch {branch}");
            let st = Stacks::load(&root)?;
            if st.stacks.is_empty() {
                println!("stacks (none)");
            } else {
                for (name, entries) in &st.stacks {
                    println!("stack  {name}");
                    for e in entries {
                        let marker = if e.branch == branch { "*" } else { " " };
                        let pr = e.pr.map(|n| format!("  #{n}")).unwrap_or_default();
                        println!("    {marker} {} <- {}{}", e.branch, e.base, pr);
                    }
                }
            }
            match herdr::agents() {
                Ok(agents) => {
                    let root_str = root.to_string_lossy().to_string();
                    println!("agents");
                    let mine: Vec<_> = agents.iter().filter(|a| a.cwd == root_str).collect();
                    if mine.is_empty() {
                        println!("  (none in this repo)");
                    }
                    for a in mine {
                        println!("  {} {} [{}]", a.pane_id, a.agent, a.agent_status);
                    }
                }
                Err(e) => println!("agents unavailable: {e}"),
            }
        }

        Cmd::Run {
            task,
            pipeline,
            base,
            from,
            only,
            dry_run,
            headless,
        } => execute(
            &root,
            cli.config.as_deref(),
            RunOpts { pipeline, task, base, from, only, dry_run, headless },
        )?,

        Cmd::New { name, prompt, pipeline, base, headless } => {
            let path = push_stack(&root, cli.config.as_deref(), &name, base, None, true)?;
            execute(
                &path,
                None,
                RunOpts { pipeline, task: prompt, headless, ..Default::default() },
            )?;
        }

        Cmd::Add {
            stack,
            prompt,
            pipeline,
            branch,
            headless,
        } => {
            let st = Stacks::load(&root)?;
            if !st.stacks.contains_key(&stack) {
                bail!(
                    "no stack `{stack}`. Known stacks: {}. Use `rigg new {stack} \"...\"` \
                     to start one.",
                    stack_names(&st)
                );
            }
            let n = st.stacks[&stack].len() + 1;
            let branch = branch.unwrap_or_else(|| format!("{stack}-{n}"));
            let path = push_stack(
                &root,
                cli.config.as_deref(),
                &branch,
                None,
                Some(stack),
                false,
            )?;
            execute(
                &path,
                None,
                RunOpts { pipeline, task: prompt, headless, ..Default::default() },
            )?;
        }

        Cmd::Attach { target, path: path_only } => {
            let st = Stacks::load(&root)?;
            let entry = match target {
                Some(t) => resolve_target(&st, &t)?,
                None => {
                    let b = util::current_branch(&root)?;
                    resolve_target(&st, &b)?
                }
            };
            let dir = entry_path(&root, &entry)?;
            if path_only {
                println!("{dir}");
                return Ok(());
            }
            let focused = herdr::workspaces()
                .unwrap_or_default()
                .into_iter()
                .find(|(_, p)| p == &dir)
                .map(|(id, _)| herdr::workspace_focus(&id));
            match focused {
                Some(Ok(())) => println!("focused workspace for {}", entry.branch),
                _ => {
                    println!("{}", entry.branch);
                    println!("{dir}");
                    println!("cd \"$(rigg attach --path {})\"", entry.branch);
                }
            }
        }

        Cmd::Say {
            target,
            message,
            agent,
        } => {
            let st = Stacks::load(&root)?;
            let entry = resolve_target(&st, &target)?;
            let dir = std::path::PathBuf::from(entry_path(&root, &entry)?);
            let cfg = Config::load(&dir, None)?;
            let role = match agent {
                Some(r) => r,
                None => cfg
                    .agents
                    .keys()
                    .next()
                    .cloned()
                    .context("no agents configured")?,
            };
            let mut vars = BTreeMap::new();
            vars.insert("branch".into(), entry.branch.clone());
            vars.insert("repo".into(), dir.to_string_lossy().to_string());
            let step = config::Step {
                id: "say".into(),
                agent: Some(role),
                prompt: Some(message),
                ..Default::default()
            };
            pipeline::Runner::new(dir, cfg, vars, false, false).run(&[step])?;
        }

        Cmd::Stack { cmd } => match cmd {
            StackCmd::Push {
                branch,
                base,
                stack: stack_name,
            } => {
                push_stack(&root, cli.config.as_deref(), &branch, base, stack_name, false)?;
            }
            StackCmd::Names => {
                for name in Stacks::load(&root)?.stacks.keys() {
                    println!("{name}");
                }
            }
            StackCmd::Prune { base, yes } => prune(&root, cli.config.as_deref(), base, yes)?,
            StackCmd::List => {
                let st = Stacks::load(&root)?;
                if st.stacks.is_empty() {
                    println!("no stacks");
                }
                let here = util::current_branch(&root).unwrap_or_default();
                for (name, entries) in &st.stacks {
                    println!("{name}");
                    for (i, e) in entries.iter().enumerate() {
                        let mark = if e.branch == here { "*" } else { " " };
                        let pr = e.pr.map(|n| format!("  #{n}")).unwrap_or_default();
                        let gone = if entry_path(&root, e).is_ok() {
                            ""
                        } else {
                            "  (worktree gone)"
                        };
                        println!("  {mark} {}. {} <- {}{}{}", i + 1, e.branch, e.base, pr, gone);
                    }
                }
            }
            StackCmd::Pr { branch } => {
                let cfg = Config::load(&root, cli.config.as_deref())?;
                let branch = match branch {
                    Some(b) => b,
                    None => util::current_branch(&root)?,
                };
                let st = Stacks::load(&root)?;
                let base = st
                    .find(&branch)
                    .map(|e| e.base.clone())
                    .unwrap_or_else(|| cfg.stack.trunk.clone());
                println!("opening PR {branch} -> {base}");
                util::shell(
                    &root,
                    &format!("gh pr create --base {base} --head {branch} --fill"),
                )?;

                if let Some(label) = &cfg.stack.preview_label {
                    let files = util::changed_files(&root, &base)?;
                    let frontend = files.iter().any(|f| {
                        cfg.stack
                            .frontend_paths
                            .iter()
                            .any(|g| util::glob_match(g, f))
                    });
                    if frontend {
                        println!("frontend paths touched, labelling `{label}`");
                        util::shell(
                            &root,
                            &format!("gh pr edit {branch} --add-label {label}"),
                        )?;
                    }
                }
            }
        },
    }
    Ok(())
}

/// Checkout location for a worktree rigg creates itself.
fn worktree_dest(cfg: &Config, main: &std::path::Path, branch: &str) -> std::path::PathBuf {
    let base = match &cfg.stack.worktree_dir {
        Some(d) => std::path::PathBuf::from(shellexpand_home(d)),
        None => {
            let repo = main
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "repo".into());
            std::path::PathBuf::from(shellexpand_home("~/.rigg/worktrees")).join(repo)
        }
    };
    base.join(branch.replace('/', "-"))
}

fn shellexpand_home(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}

fn stack_names(st: &Stacks) -> String {
    if st.stacks.is_empty() {
        "(none)".into()
    } else {
        st.stacks.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Resolve a stack name (to its tip) or a branch name (to itself).
fn resolve_target(st: &Stacks, name: &str) -> Result<Entry> {
    if let Some(e) = st.tip_of(name) {
        return Ok(e.clone());
    }
    if let Some(e) = st.find(name) {
        return Ok(e.clone());
    }
    bail!(
        "no stack or branch `{name}`. Known stacks: {}",
        stack_names(st)
    )
}

/// Where a stack entry's checkout lives, recovering the path if it was not
/// recorded when the worktree was made.
fn entry_path(root: &std::path::Path, e: &Entry) -> Result<String> {
    if let Some(p) = &e.path {
        if std::path::Path::new(p).exists() {
            return Ok(p.clone());
        }
    }
    let from = util::main_checkout(root)?;
    util::worktree_path(&from, &e.branch)
        .with_context(|| format!("no worktree on disk for `{}`", e.branch))
}

#[derive(Default)]
struct RunOpts {
    pipeline: Option<String>,
    task: Option<String>,
    base: Option<String>,
    from: Option<String>,
    only: Vec<String>,
    dry_run: bool,
    headless: bool,
}

/// Load the config, pick the pipeline and run it against `root`.
fn execute(root: &std::path::Path, cfg_path: Option<&str>, o: RunOpts) -> Result<()> {
    let cfg = Config::load(root, cfg_path)?;
    let branch = util::current_branch(root)?;
    let st = Stacks::load(root)?;
    let base = o
        .base
        .or_else(|| st.find(&branch).map(|e| e.base.clone()))
        .unwrap_or_else(|| cfg.stack.trunk.clone());

    let mut steps = cfg.steps_for(o.pipeline.as_deref())?;
    if steps.is_empty() {
        bail!("pipeline has no steps");
    }
    if let Some(from) = &o.from {
        let idx = steps
            .iter()
            .position(|s| &s.id == from)
            .with_context(|| format!("no step with id `{from}`"))?;
        steps = steps.split_off(idx);
    }
    if !o.only.is_empty() {
        steps.retain(|s| o.only.contains(&s.id));
        if steps.is_empty() {
            bail!("no steps matched --only");
        }
    }
    let needs_task = steps
        .iter()
        .any(|s| s.prompt.as_deref().is_some_and(|p| p.contains("{{task}}")));
    let task = match o.task {
        Some(t) => t,
        None if needs_task => ask("Task: ")?,
        None => String::new(),
    };

    let mut vars = BTreeMap::new();
    vars.insert("task".into(), task);
    vars.insert("branch".into(), branch.clone());
    vars.insert("base".into(), base.clone());
    vars.insert("repo".into(), root.to_string_lossy().to_string());

    // Pushing or opening a PR from the trunk onto itself is a footgun; a purely
    // local pipeline on the trunk is fine.
    let publishes = steps.iter().any(|s| {
        s.run
            .as_deref()
            .is_some_and(|c| c.contains("git push") || c.contains("gh pr create"))
    });
    if branch == base && publishes && !o.dry_run {
        bail!(
            "step would push or open a PR from `{branch}` onto itself. Start a \
             branch first, e.g. `rigg new my-feature`, or pass --base <other-ref>."
        );
    }
    println!("rigg: {} step(s) on {branch} (base {base})", steps.len());
    let mut runner = pipeline::Runner::new(root.to_path_buf(), cfg, vars, o.dry_run, o.headless);
    runner.run(&steps)?;
    println!("\nrigg: pipeline complete");
    if !o.dry_run {
        herdr::notify(&format!("rigg: {branch} pipeline complete"));
    }
    Ok(())
}

/// Create the worktree for `branch` and record it on a stack. Returns the new
/// checkout path.
fn push_stack(
    root: &std::path::Path,
    cfg_path: Option<&str>,
    branch: &str,
    base: Option<String>,
    stack_name: Option<String>,
    new_stack: bool,
) -> Result<std::path::PathBuf> {
    let cfg = Config::load(root, cfg_path).unwrap_or_default();
    let mut st = Stacks::load(root)?;
    let here = util::current_branch(root).ok();

    let target = stack_name
        .or_else(|| {
            if new_stack || base.is_some() {
                None
            } else {
                here.as_deref()
                    .and_then(|b| st.stack_of(b))
                    .map(str::to_string)
            }
        })
        .unwrap_or_else(|| branch.to_string());
    let is_new = !st.stacks.contains_key(&target);

    let base = base
        .or_else(|| st.tip_of(&target).map(|e| e.branch.clone()))
        .unwrap_or_else(|| cfg.stack.trunk.clone());

    if is_new {
        println!("starting stack `{target}` at {base}");
    } else {
        println!("adding to stack `{target}`");
    }
    println!("creating worktree {branch} on top of {base}");

    // A new branch is cut from `base`'s last commit, so anything still
    // uncommitted there would silently not be part of it.
    if let Some(base_entry) = st.find(&base) {
        if let Ok(dir) = entry_path(root, base_entry) {
            let dirty = util::git(std::path::Path::new(&dir), &["status", "--porcelain"])
                .map(|o| !o.trim().is_empty())
                .unwrap_or(false);
            if dirty {
                bail!(
                    "`{base}` has uncommitted changes in {dir}, which the new branch \
                     would not include. Commit them first (a pipeline that stacks \
                     needs a commit step)."
                );
            }
        }
    }

    let from = util::main_checkout(root)?;
    // Herdr gives the worktree its own workspace; without it (or in a headless
    // setup) plain git is enough and keeps rigg usable on its own.
    let use_herdr = cfg.backend != Backend::Headless && herdr::inside_session();
    let mut path = if use_herdr {
        herdr::worktree_create(branch, &base, &from.to_string_lossy())?
    } else {
        let dest = worktree_dest(&cfg, &from, branch);
        util::git_worktree_add(&from, branch, &base, &dest)?;
        dest.to_string_lossy().to_string()
    };
    if path.is_empty() {
        path = util::worktree_path(&from, branch).unwrap_or_default();
    }
    if path.is_empty() {
        bail!("worktree for `{branch}` was created but its path could not be resolved");
    }
    st.stacks.entry(target.clone()).or_default().push(Entry {
        branch: branch.to_string(),
        base,
        path: Some(path.clone()),
        pr: None,
    });
    st.save(root)?;
    println!("stack `{target}` is now {} deep", st.stacks[&target].len());
    println!("worktree at {path}");
    Ok(std::path::PathBuf::from(path))
}

/// Remove worktrees for branches already merged into the trunk.
fn prune(
    root: &std::path::Path,
    cfg_path: Option<&str>,
    base: Option<String>,
    yes: bool,
) -> Result<()> {
    let cfg = Config::load(root, cfg_path).unwrap_or_default();
    let trunk = cfg.stack.trunk.clone();
    let base = base.unwrap_or_else(|| {
        let remote = format!("origin/{trunk}");
        if util::git(root, &["rev-parse", "--verify", "--quiet", &remote]).is_ok() {
            remote
        } else {
            trunk.clone()
        }
    });
    println!("testing against {base}\n");

    // The main checkout is never a prune candidate.
    let main_checkout = util::main_checkout(root)?.to_string_lossy().to_string();
    let here = root.to_string_lossy().to_string();

    let listing = util::git(root, &["worktree", "list", "--porcelain"])?;
    let mut path: Option<String> = None;
    let mut candidates: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();

    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            let branch = b.trim_start_matches("refs/heads/").to_string();
            let Some(p) = path.clone() else { continue };
            if p == main_checkout || p == here {
                continue;
            }
            let merged = util::git(
                root,
                &["merge-base", "--is-ancestor", &format!("refs/heads/{branch}"), &base],
            )
            .is_ok();
            if !merged {
                continue;
            }
            // A merged branch can still have uncommitted work in its checkout.
            match util::git(std::path::Path::new(&p), &["status", "--porcelain"]) {
                Ok(out) if out.trim().is_empty() => candidates.push((p, branch)),
                Ok(_) => skipped.push((p, "uncommitted changes".into())),
                Err(_) => skipped.push((p, "checkout unreadable".into())),
            }
        }
    }

    if candidates.is_empty() {
        println!("nothing to prune");
    }
    for (p, b) in &candidates {
        println!("  {b}  ({p})");
    }
    for (p, why) in &skipped {
        println!("  skipped: {p} - {why}");
    }
    println!("\n{} merged worktree(s)", candidates.len());

    if !yes {
        println!("dry run; pass --yes to remove them");
        return Ok(());
    }

    let ws = herdr::workspaces().unwrap_or_default();
    let in_use = util::cwds_in_use();
    let mut removed = 0;
    let mut failed = 0;
    for (p, b) in &candidates {
        match ws.iter().find(|(_, path)| path == p) {
            // Closing the workspace ends the session that lives in it.
            Some((id, _)) => {
                let _ = herdr::workspace_close(id);
            }
            // Nothing of ours owns it, so leave it alone while it is someone's
            // working directory.
            None if in_use.contains(p) => {
                println!("skipped {b}: in use by a running process");
                continue;
            }
            None => {}
        }
        match util::git(root, &["worktree", "remove", p]) {
            Ok(_) => {
                removed += 1;
                println!("removed {b}");
            }
            Err(e) => {
                failed += 1;
                println!("failed  {b}: {e}");
            }
        }
    }
    let mut st = Stacks::load(root)?;
    st.retain_branches(|e| !candidates.iter().any(|(_, b)| b == &e.branch));
    // Stack entries whose checkout is gone are just stale bookkeeping.
    let mut stale = Vec::new();
    st.retain_branches(|e| {
        if entry_path(root, e).is_ok() {
            true
        } else {
            stale.push(e.branch.clone());
            false
        }
    });
    if !stale.is_empty() {
        println!("dropped stale stack entries: {}", stale.join(", "));
    }
    st.save(root)?;
    println!("\nremoved {removed}, failed {failed}");
    Ok(())
}

fn doctor(root: &std::path::Path, cfg_path: Option<&str>, force_headless: bool) -> Result<()> {
    let mut problems = 0;
    let cfg = Config::load(root, cfg_path);

    // Only the backends actually in use are worth checking: a headless-only
    // config needs neither a herdr session nor the integration hooks.
    let mut herdr_kinds: Vec<String> = Vec::new();
    let mut headless_bins: Vec<(String, String)> = Vec::new();
    match &cfg {
        Ok(c) => {
            for (role, a) in &c.agents {
                let backend = if force_headless {
                    Backend::Headless
                } else {
                    c.backend_for(role)
                };
                match backend {
                    Backend::Herdr => herdr_kinds.push(a.kind.clone()),
                    Backend::Headless => headless_bins.push((role.clone(), headless::binary(a))),
                }
            }
        }
        Err(_) if force_headless => headless_bins.push(("?".into(), "claude".into())),
        Err(_) => herdr_kinds = vec!["claude".into(), "opencode".into()],
    }
    herdr_kinds.sort();
    herdr_kinds.dedup();

    println!(
        "backend        {}",
        match (herdr_kinds.is_empty(), headless_bins.is_empty()) {
            (false, false) => "herdr + headless".to_string(),
            (true, false) => Backend::Headless.as_str().to_string(),
            _ => Backend::Herdr.as_str().to_string(),
        }
    );

    if !herdr_kinds.is_empty() {
        println!("herdr binary   {}", herdr::bin());
        if herdr::inside_session() {
            println!("herdr session  yes (workspace {})",
                herdr::current_workspace().unwrap_or_else(|| "?".into()));
        } else {
            println!("herdr session  NO - run rigg from inside a herdr pane");
            problems += 1;
        }

        // Agent state detection is only reliable with the integration hooks in place.
        let status = herdr::call(&["integration", "status"]).unwrap_or_default();
        for kind in &herdr_kinds {
            let line = status
                .lines()
                .find(|l| l.starts_with(&format!("{kind}:")))
                .unwrap_or("");
            let state = line.split_once(':').map(|(_, r)| r.trim()).unwrap_or("");
            if state.starts_with("current") {
                println!("integration    {kind}: {state}");
            } else if state.is_empty() || state.starts_with("not installed") {
                println!(
                    "integration    {kind}: NOT INSTALLED - agent state will be guessed. \
                     Run `herdr integration install {kind}`"
                );
                problems += 1;
            } else {
                println!(
                    "integration    {kind}: {state} - run `herdr integration install {kind}` to update"
                );
                problems += 1;
            }
        }
    }

    for (role, bin) in &headless_bins {
        let label = format!("agent {role}");
        if which(bin) {
            println!("{label:<14} {bin} found");
        } else {
            println!("{label:<14} {bin} NOT ON PATH");
            problems += 1;
        }
    }

    if which("gh") {
        println!("gh             found");
    } else {
        println!("gh             missing (stack pr will fail)");
        problems += 1;
    }

    let path = match cfg_path {
        Some(p) => std::path::PathBuf::from(p),
        None => Config::path_for(root),
    };
    match &cfg {
        Ok(c) => println!("config         ok ({} steps)", c.steps.len()),
        Err(e) if path.exists() => {
            println!("config         INVALID: {e:#}");
            problems += 1;
        }
        Err(_) => {
            println!("config         missing - run `rigg init`");
            problems += 1;
        }
    }

    if problems == 0 {
        println!("\nall good");
    } else {
        println!("\n{problems} problem(s) to fix");
    }
    Ok(())
}

/// Read a line from the terminal.
fn ask(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim().to_string();
    if line.is_empty() {
        bail!("no task given");
    }
    Ok(line)
}

/// Invoked by the Herdr plugin action. The action runs detached from any
/// terminal, so open a pane in the invoking workspace and run there.
fn plugin_run() -> Result<()> {
    // The action context names the workspace the user actually invoked from;
    // HERDR_PANE_ID belongs to the plugin host, not that workspace.
    let cwd = plugin_context_cwd()
        .or_else(|| {
            std::env::var("HERDR_PANE_ID")
                .ok()
                .and_then(|p| herdr::pane_cwd(&p).ok())
        })
        .context("could not determine a working directory for the pipeline")?;

    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy().to_string();
    // Keep the pane alive after rigg exits so failures stay readable.
    let script = format!(
        "{} run; printf '\\n[rigg finished - press enter to close] '; read _",
        sh_quote(&exe)
    );
    // `herdr pane run` hands the command to a shell as text rather than exec'ing
    // argv, so the script has to survive one round of shell parsing.
    let cmd = format!("sh -c {}", sh_quote(&script));
    herdr::split_and_run(&cwd, &[&cmd])?;
    Ok(())
}

/// Single-quote a string for safe re-parsing by a shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn plugin_context_cwd() -> Option<String> {
    let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    for key in ["workspace_cwd", "focused_pane_cwd"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
