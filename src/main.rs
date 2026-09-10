mod config;
mod corpus;
mod cron;
mod headless;
mod instance;
mod media;
mod model;
mod pipeline;
mod stack;
mod util;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::collections::BTreeMap;

use config::Config;
use stack::{Entry, Stacks};

#[derive(Parser)]
#[command(
    name = "rigg",
    version,
    about = "Pipeline runner for AI coding agents",
    after_help = "Shell keybindings and aliases:  rigg keys"
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
        /// Stack name. If this is the only argument and reads like a sentence
        /// it is taken as the task, and a name is generated.
        name: Option<String>,
        /// The task. Prompted for if omitted.
        prompt: Option<String>,
        #[arg(long)]
        pipeline: Option<String>,
        #[arg(long)]
        base: Option<String>,
        /// Attach an image file to the task. Repeatable.
        #[arg(long = "image", value_name = "PATH")]
        images: Vec<String>,
        /// Do not look at the clipboard for an image.
        #[arg(long)]
        no_paste: bool,
        /// Run agent steps on another kind: `--agent opencode` swaps every
        /// role, `--agent reviewer=opencode` swaps one. Repeatable.
        #[arg(long = "agent", value_name = "[ROLE=]KIND")]
        agents: Vec<String>,
        /// Set a prompt placeholder for this run: `--var effort=high` fills
        /// {{effort}}. Repeatable; overrides [vars] in the config.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Turn an optional part of the pipeline on for this run.
        #[arg(long = "with", value_name = "FEATURE")]
        with_features: Vec<String>,
        /// Turn one off for this run.
        #[arg(long = "without", value_name = "FEATURE")]
        without_features: Vec<String>,
        /// Run in this terminal instead of detaching.
        #[arg(long)]
        fg: bool,
    },
    /// Take an existing branch into a stack and run a pipeline on it.
    Adopt {
        /// The branch to take over. It is used as it stands: nothing is cut
        /// from it, and removing the stack later leaves it alone.
        branch: String,
        /// A task for the run, where the pipeline wants one. A pipeline that
        /// only reviews or previews the branch needs none.
        prompt: Option<String>,
        #[arg(long)]
        pipeline: Option<String>,
        /// What the branch is stacked on - what a PR would target and what a
        /// review diffs against. Defaults to the trunk.
        #[arg(long)]
        base: Option<String>,
        /// Name the stack. Defaults to the branch name.
        #[arg(long)]
        stack: Option<String>,
        /// Attach an image file to the task. Repeatable.
        #[arg(long = "image", value_name = "PATH")]
        images: Vec<String>,
        /// Do not look at the clipboard for an image.
        #[arg(long)]
        no_paste: bool,
        /// Run agent steps on another kind: `--agent opencode` swaps every
        /// role, `--agent reviewer=opencode` swaps one. Repeatable.
        #[arg(long = "agent", value_name = "[ROLE=]KIND")]
        agents: Vec<String>,
        /// Set a prompt placeholder for this run: `--var effort=high` fills
        /// {{effort}}. Repeatable; overrides [vars] in the config.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Turn an optional part of the pipeline on for this run.
        #[arg(long = "with", value_name = "FEATURE")]
        with_features: Vec<String>,
        /// Turn one off for this run.
        #[arg(long = "without", value_name = "FEATURE")]
        without_features: Vec<String>,
        /// Run in this terminal instead of detaching.
        #[arg(long)]
        fg: bool,
    },
    /// Append the next branch to a stack and run a pipeline on it.
    Add {
        /// Stack to extend, or the task itself. Omit to choose from a list.
        /// The new branch is named <stack>-<n>.
        stack: Option<String>,
        /// The task. Prompted for if omitted.
        prompt: Option<String>,
        #[arg(long)]
        pipeline: Option<String>,
        /// Name the new branch explicitly instead of <stack>-<n>.
        #[arg(long)]
        branch: Option<String>,
        /// Attach an image file to the task. Repeatable.
        #[arg(long = "image", value_name = "PATH")]
        images: Vec<String>,
        /// Do not look at the clipboard for an image.
        #[arg(long)]
        no_paste: bool,
        /// Run agent steps on another kind: `--agent opencode` swaps every
        /// role, `--agent reviewer=opencode` swaps one. Repeatable.
        #[arg(long = "agent", value_name = "[ROLE=]KIND")]
        agents: Vec<String>,
        /// Set a prompt placeholder for this run: `--var effort=high` fills
        /// {{effort}}. Repeatable; overrides [vars] in the config.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Turn an optional part of the pipeline on for this run.
        #[arg(long = "with", value_name = "FEATURE")]
        with_features: Vec<String>,
        /// Turn one off for this run.
        #[arg(long = "without", value_name = "FEATURE")]
        without_features: Vec<String>,
        /// Run in this terminal instead of detaching.
        #[arg(long)]
        fg: bool,
        /// Internal: this process is the detached worker for a queued add.
        #[arg(long, hide = true)]
        worker: bool,
    },
    /// Open an interactive agent on a stack, resuming its session.
    Attach {
        /// Stack or branch name (default: whatever the cwd is in).
        target: Option<String>,
        /// Print only the checkout path, for `cd "$(rigg attach --path billing)"`.
        #[arg(long)]
        path: bool,
        /// Start a fresh session instead of resuming the last one.
        #[arg(long)]
        new: bool,
        /// Open even though a run is still in progress.
        #[arg(long)]
        force: bool,
        /// Wait for the run to finish, then open its session.
        #[arg(long)]
        wait: bool,
        /// Agent role to open (default: the first one configured).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Send a follow-up prompt to a stack's agent session.
    Say {
        /// Stack or branch to talk to. Omit to choose from a list.
        target: Option<String>,
        /// The message. If `target` is the only argument and reads like a
        /// sentence, it is taken as the message and the stack is chosen.
        message: Option<String>,
        /// Agent role (default: the first one configured).
        #[arg(long)]
        agent: Option<String>,
        /// Leave the change in the branch instead of committing and pushing
        /// it. Pushing is the default: the branch is looked at through a
        /// preview and reviewed as a PR, and an unpushed change is in neither.
        #[arg(long)]
        no_push: bool,
        /// Attach an image file to the message. Repeatable.
        #[arg(long = "image", value_name = "PATH")]
        images: Vec<String>,
        /// Do not look at the clipboard for an image.
        #[arg(long)]
        no_paste: bool,
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
        /// Run agent steps on another kind: `--agent opencode` swaps every
        /// role, `--agent reviewer=opencode` swaps one. Repeatable.
        #[arg(long = "agent", value_name = "[ROLE=]KIND")]
        agents: Vec<String>,
        /// Set a prompt placeholder for this run: `--var effort=high` fills
        /// {{effort}}. Repeatable; overrides [vars] in the config.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Turn an optional part of the pipeline on for this run.
        #[arg(long = "with", value_name = "FEATURE")]
        with_features: Vec<String>,
        /// Turn one off for this run.
        #[arg(long = "without", value_name = "FEATURE")]
        without_features: Vec<String>,
        /// Run in the background and return, as `new` and `add` do.
        #[arg(long)]
        detach: bool,
    },
    /// Show the shell keybindings and aliases.
    Keys {
        /// Emit shell integration code instead of the table:
        /// `source <(rigg keys --shell zsh)`.
        #[arg(long)]
        shell: Option<String>,
    },
    /// Show a run's log.
    Logs {
        /// Stack or branch name. Omit to choose.
        target: Option<String>,
        /// Follow the log as it grows.
        #[arg(short, long)]
        follow: bool,
    },
    /// Ask a question about the code. Changes nothing, makes no branch.
    Ask {
        /// The question. Prompted for if omitted.
        question: Option<String>,
        /// Start a fresh conversation instead of carrying the last one on.
        #[arg(long)]
        new: bool,
        /// Carry on this exact conversation. Without it, `--continue` resumes
        /// whichever was last in the checkout - which is the wrong one as soon
        /// as two are going at once.
        #[arg(long)]
        session: Option<String>,
        /// Agent role, kind, or `role=kind` (default: the first role configured).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Show the steps a run would take, and what will run each of them.
    Plan {
        #[arg(long)]
        pipeline: Option<String>,
        /// Run agent steps on another kind: `--agent opencode` swaps every
        /// role, `--agent reviewer=opencode` swaps one. Repeatable.
        #[arg(long = "agent", value_name = "[ROLE=]KIND")]
        agents: Vec<String>,
        /// Turn an optional part on for this plan.
        #[arg(long = "with", value_name = "FEATURE")]
        with_features: Vec<String>,
        /// Turn an optional part off for this plan.
        #[arg(long = "without", value_name = "FEATURE")]
        without_features: Vec<String>,
    },
    /// Show the branch and stack state.
    Status,
    /// Print the pipeline names, one per line.
    Pipelines,
    /// Show the optional parts, what each contains, and who turns it on.
    Features,
    /// Take a branch further: run more of a pipeline than it has had, picking
    /// up after the last step it finished.
    Continue {
        /// Stack or branch. Omit to choose.
        target: Option<String>,
        /// Pipeline to carry on with. Defaults to the one the branch last ran.
        #[arg(long)]
        pipeline: Option<String>,
        /// Turn an optional part on for the rest of the run: `--with copilot`
        /// is how a branch gets a round it was not started with.
        #[arg(long = "with", value_name = "FEATURE")]
        with_features: Vec<String>,
        /// Turn one off.
        #[arg(long = "without", value_name = "FEATURE")]
        without_features: Vec<String>,
        /// Wait for the run in flight to finish, then carry on. Without it a
        /// branch that is busy is simply refused.
        #[arg(long)]
        wait: bool,
        /// Run in this terminal instead of detaching.
        #[arg(long)]
        fg: bool,
        /// Internal: this process is the detached waiter.
        #[arg(long, hide = true)]
        worker: bool,
    },
    /// Stop a run that is still going.
    Stop {
        /// Stack or branch. Omit to choose.
        target: Option<String>,
    },
    /// Check that the environment can actually drive agents.
    Doctor,
    /// Show, set or remove this instance's secrets.
    Secret {
        #[command(subcommand)]
        cmd: Option<SecretCmd>,
    },
    /// Repos this instance covers, whose declared schedules it runs.
    Repo {
        #[command(subcommand)]
        cmd: Option<RepoCmd>,
    },
    /// The repo capability is read from: pipelines, prompts, tools, schedules.
    #[command(name = "config-repo")]
    ConfigRepo {
        #[command(subcommand)]
        cmd: Option<ConfigRepoCmd>,
    },
    /// Which Slack channel drives which target, for the bridge to read.
    Channels {
        /// Machine-readable, for the bridge.
        #[arg(long)]
        json: bool,
    },
    /// Corpora a role can be given: what is available, and wiring one in.
    Corpus {
        #[command(subcommand)]
        cmd: Option<CorpusCmd>,
    },
    /// Schedules for this instance.
    Cron {
        #[command(subcommand)]
        cmd: Option<CronCmd>,
    },
    /// Fire whatever is due this minute. One crontab line per instance.
    Tick {
        /// Pretend it is this time instead of now.
        #[arg(long)]
        at: Option<String>,
        /// Say what would fire, start nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage a stack of dependent branches.
    Stack {
        #[command(subcommand)]
        cmd: StackCmd,
    },
}

#[derive(Subcommand)]
enum SecretCmd {
    /// Store a secret for this instance.
    Set { name: String, value: String },
    /// Remove one this instance set.
    Rm { name: String },
    /// List them, masked.
    List,
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Register one. Defaults to the repo you are standing in.
    Add { path: Option<String> },
    /// Stop covering one. Its declared schedules stop running.
    Rm { path: Option<String> },
    /// What this instance covers.
    List,
}

#[derive(Subcommand)]
enum ConfigRepoCmd {
    /// Point this instance at a config repo.
    Set { path: String },
    /// Go back to reading `.rigg/` from whatever repo you stand in.
    Clear,
    /// What is in force.
    Show,
}

#[derive(Subcommand)]
enum CorpusCmd {
    /// Sidecars rigg can find, and whether each is wired into this repo.
    List,
    /// Write the wiring for one into this repo's .rigg/, to be reviewed.
    Enable {
        /// A sidecar name, or a path to one.
        name: String,
        /// Print what would be written, write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Whether one is wired in, has its secret, and how fresh its data is.
    Status { name: Option<String> },
}

#[derive(Subcommand)]
enum CronCmd {
    /// Add a schedule: `rigg cron add "0 7 * * *" --pipeline morning-mail`.
    Add {
        /// Five cron fields, or an @alias, followed by an optional task.
        words: Vec<String>,
        #[arg(long)]
        pipeline: Option<String>,
        /// The task, if it was not typed after the schedule.
        #[arg(long)]
        task: Option<String>,
        /// Cut a branch and stack the work instead of running in place.
        #[arg(long)]
        new: bool,
        /// The repo to run in. Defaults to the one you are standing in.
        #[arg(long)]
        repo: Option<String>,
        /// Set a placeholder for every run of this job. Repeatable.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Where it reports, for a run with no Slack thread to reply in.
        #[arg(long)]
        channel: Option<String>,
        /// Name the job. Defaults to one derived from what it runs.
        #[arg(long)]
        id: Option<String>,
    },
    /// Change a schedule that already exists.
    Edit {
        id: String,
        #[arg(long)]
        schedule: Option<String>,
        #[arg(long)]
        pipeline: Option<String>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        pause: bool,
        #[arg(long)]
        resume: bool,
    },
    /// Remove one.
    Rm { id: String },
    /// Run one now, down the same path a tick takes.
    Run {
        id: String,
        /// Internal: a tick already recorded the start.
        #[arg(long, hide = true)]
        fired: bool,
    },
    /// Everything about one job, including its log.
    Show { id: String },
    /// Print a typed job as a [[schedules]] block, to make it permanent.
    Export { id: String },
    /// Print the job ids, one per line, for shell completion.
    Names,
    /// List them.
    List {
        /// Only jobs that report to this channel.
        #[arg(long)]
        channel: Option<String>,
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
        /// Keep the branches; only remove the checkouts.
        #[arg(long)]
        keep_branches: bool,
    },
    /// Remove a whole stack: its worktrees, branches and bookkeeping.
    Rm {
        /// Stack to remove. Omit to choose.
        stack: Option<String>,
        /// Actually remove it. Without this it only shows what would go.
        #[arg(long)]
        yes: bool,
        /// Remove even when a run is going, work is uncommitted, or a branch
        /// has commits that are not on the trunk.
        #[arg(long)]
        force: bool,
    },
    /// Open a PR for a stack entry against its own base.
    Pr {
        #[arg(long)]
        branch: Option<String>,
    },
}

/// Keybindings, and the zsh that installs them. Kept together so `rigg keys`
/// can never describe bindings the shell does not actually have.
const KEYS: [(&str, &str); 8] = [
    ("^X n", "start a new stack: type the task"),
    ("^X d", "add to a stack: pick it, then type"),
    ("^X m", "say something to a stack: pick it, then type"),
    ("^X a", "attach to a stack"),
    ("^X l", "follow a run's log"),
    ("^X s", "list stacks, keeping what you were typing"),
    ("^X r", "remove a stack: pick it, then confirm"),
    ("^X ?", "show this table"),
];

const ALIASES: [(&str, &str); 8] = [
    ("rn", "rigg new"),
    ("rad", "rigg add"),
    ("ra", "rigg attach"),
    ("rl", "rigg logs"),
    ("rsay", "rigg say"),
    ("rs", "rigg stack list"),
    ("rrm", "rigg stack rm"),
    ("rcd", "cd to a stack's checkout"),
];

const ZSH_INTEGRATION: &str = r#"# rigg shell integration; generated by `rigg keys --shell zsh`
export RIGG_ZSH=1

alias rn='rigg new'
alias ra='rigg attach'
alias rad='rigg add'
alias rl='rigg logs'
alias rsay='rigg say'
alias rs='rigg stack list'
alias rrm='rigg stack rm'

# A function, not an alias: cd has to happen in this shell, and `rigg attach
# --path` is too much to type for something done several times a day.
rcd() {
  local p
  p="$(rigg attach --path "$@")" || return $?
  cd "$p" || return $?
  pwd
}

rigg-new-widget() { BUFFER='rigg new'; zle accept-line }
zle -N rigg-new-widget
bindkey '^Xn' rigg-new-widget

rigg-add-widget() { BUFFER='rigg add'; zle accept-line }
zle -N rigg-add-widget
bindkey '^Xd' rigg-add-widget

rigg-say-widget() { BUFFER='rigg say'; zle accept-line }
zle -N rigg-say-widget
bindkey '^Xm' rigg-say-widget

rigg-attach-widget() { BUFFER='rigg attach'; zle accept-line }
zle -N rigg-attach-widget
bindkey '^Xa' rigg-attach-widget

rigg-logs-widget() { BUFFER='rigg logs -f'; zle accept-line }
zle -N rigg-logs-widget
bindkey '^Xl' rigg-logs-widget

rigg-rm-widget() { BUFFER='rigg stack rm'; zle accept-line }
zle -N rigg-rm-widget
bindkey '^Xr' rigg-rm-widget

rigg-stack-widget() { zle push-line; BUFFER='rigg stack list'; zle accept-line }
zle -N rigg-stack-widget
bindkey '^Xs' rigg-stack-widget

rigg-keys-widget() { zle push-line; BUFFER='rigg keys'; zle accept-line }
zle -N rigg-keys-widget
bindkey '^X?' rigg-keys-widget
"#;

/// Subcommands that can be prefixed with a pipeline name.
const PIPELINE_VERBS: [&str; 4] = ["new", "adopt", "add", "run"];

/// Accept `rigg <pipeline> new <name> "<prompt>"` by rewriting it into
/// `rigg new <name> "<prompt>" --pipeline <pipeline>`.
fn rewrite_pipeline_prefix(mut argv: Vec<String>) -> Vec<String> {
    if argv.len() < 3 {
        return argv;
    }
    let first = argv[1].clone();
    // `cron add` and `cron run` are subcommands, not a pipeline called cron.
    const NOT_A_PIPELINE: [&str; 6] = ["corpus", "cron", "repo", "secret", "stack", "tick"];
    if first.starts_with('-')
        || NOT_A_PIPELINE.contains(&first.as_str())
        || !PIPELINE_VERBS.contains(&argv[2].as_str())
    {
        return argv;
    }
    argv.remove(1);
    argv.push("--pipeline".into());
    argv.push(first);
    argv
}

fn main() {
    // Rust ignores SIGPIPE, so `rigg stack list | head` panics on a broken pipe
    // instead of just stopping. Restore the default disposition.
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        const SIGPIPE: i32 = 13;
        const SIG_DFL: usize = 0;
        signal(SIGPIPE, SIG_DFL);
    }

    if let Err(e) = real_main() {
        eprintln!("rigg: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse_from(rewrite_pipeline_prefix(std::env::args().collect()));

    // Works anywhere; it says nothing about a repository.
    if let Cmd::Keys { shell } = &cli.cmd {
        let shell = shell.clone();

            match shell.as_deref() {
                Some("zsh") => print!("{ZSH_INTEGRATION}"),
                Some(other) => bail!("no integration for `{other}`; only zsh so far"),
                None => {
                    println!("rigg shell keybindings\n");
                    for (k, d) in KEYS {
                        println!("  {k:<8} {d}");
                    }
                    println!("\naliases");
                    for (a, d) in ALIASES {
                        println!("  {a:<8} {d}");
                    }
                    if std::env::var("RIGG_ZSH").is_err() {
                        println!(
                            "\nNot loaded in this shell. Add to ~/.zshrc:\n  \
                             source <(rigg keys --shell zsh)"
                        );
                    }
                }
            }
            return Ok(());
        
    }

    instance::export()?;

    // Instance state is not repo state, so these work from anywhere.
    match cli.cmd {
        Cmd::Secret { cmd } => return secret_cmd(cmd),
        Cmd::Cron { cmd } => return cron_cmd(cmd),
        Cmd::Tick { at, dry_run } => return tick_cmd(at.as_deref(), dry_run),
        Cmd::Repo { cmd } => return repo_cmd(cmd),
        Cmd::ConfigRepo { cmd } => return config_repo_cmd(cmd),
        // Before repo_root(): the bridge asks for this from wherever it was
        // started, and with a config repo set there is nothing to stand in.
        Cmd::Channels { json } => return channels_cmd(cli.config.as_deref(), json),
        _ => {}
    }

    // Standing outside any repo is normal once capability lives in a config
    // repo - `rigg doctor` is the deploy checklist, and having to cd somewhere
    // first to read it is the kind of friction that makes people skip it.
    // Only the commands that just describe the setup fall back; anything that
    // acts on code still wants you to say which checkout you mean.
    let describes_setup = matches!(cli.cmd, Cmd::Doctor | Cmd::Pipelines);
    let root = match (util::repo_root(), instance::config_repo()) {
        (Ok(r), _) => r,
        (Err(_), Some(c)) if describes_setup => c,
        (Err(e), _) => return Err(e),
    };

    match cli.cmd {
        Cmd::Secret { .. }
        | Cmd::Cron { .. }
        | Cmd::Tick { .. }
        | Cmd::Repo { .. }
        | Cmd::ConfigRepo { .. }
        | Cmd::Channels { .. } => {
            unreachable!("handled above")
        }
        Cmd::Corpus { cmd } => corpus_cmd(&root, cli.config.as_deref(), cmd)?,
        Cmd::Keys { .. } => unreachable!("handled above"),
        Cmd::Init { force } => {
            let [preferred, legacy] = Config::candidates(&root);
            if legacy.exists() && !force {
                bail!(
                    "{} already exists; move it to {} or pass --force",
                    legacy.display(),
                    preferred.display()
                );
            }
            let path = preferred;
            if path.exists() && !force {
                bail!("{} already exists (use --force)", path.display());
            }
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&path, config::TEMPLATE)?;
            println!("wrote {}", path.display());

            // The rest of the tree is what nobody finds by reading one file:
            // where a prompt lives, where an mcp surface goes, and which of
            // the three planes this directory is.
            let dir = path.parent().unwrap_or(&root).to_path_buf();
            for (rel, body) in [
                ("README.md", config::README),
                ("prompts/reviewer.md", config::REVIEWER_PROMPT),
            ] {
                let f = dir.join(rel);
                if let Some(d) = f.parent() {
                    std::fs::create_dir_all(d)?;
                }
                if f.exists() && !force {
                    println!("kept  {}", f.display());
                    continue;
                }
                std::fs::write(&f, body)?;
                println!("wrote {}", f.display());
            }
            let mcp = dir.join("mcp");
            std::fs::create_dir_all(&mcp)?;
            let keep = mcp.join(".gitkeep");
            if !keep.exists() {
                std::fs::write(&keep, "")?;
            }
            println!("wrote {}/", mcp.display());

            println!("\nNext:");
            println!("  rigg doctor              is the chain intact");
            println!("  rigg plan                the steps a run would take");
            println!("  rigg corpus              what a role could be given");
            println!("  rigg cron                what runs unattended");
            println!("  rigg run --task \"...\"    run it");
        }

        Cmd::Doctor => doctor(&root, cli.config.as_deref())?,

        Cmd::Features => {
            let cfg = Config::load(&root, cli.config.as_deref())?;
            if cfg.features.is_empty() {
                println!("no optional parts; every step always runs");
                return Ok(());
            }
            // Every step in the file, wherever it is written, so a part shows
            // its members whether the config is one list or a chain.
            let all: Vec<&config::Step> = cfg
                .steps
                .iter()
                .chain(cfg.pipelines.values().flat_map(|p| p.steps.iter()))
                .collect();
            println!("optional parts        rigg run --with <name> / --without <name>\n");
            for (name, on) in &cfg.features {
                let members: Vec<&str> = all
                    .iter()
                    .filter(|s| s.feature.as_deref() == Some(name.as_str()))
                    .map(|s| s.id.as_str())
                    .collect();
                println!(
                    "  {:<12} {:<4} {}",
                    name,
                    if *on { "on" } else { "off" },
                    if members.is_empty() {
                        "(no steps use it)".to_string()
                    } else {
                        members.join(", ")
                    }
                );
                // Which names flip it, so the listing answers "how do I get
                // this" and not only "what is it".
                for want in [true, false] {
                    let names: Vec<&str> = cfg
                        .pipelines
                        .iter()
                        .filter(|(_, p)| p.features.get(name) == Some(&want))
                        .map(|(n, _)| n.as_str())
                        .collect();
                    if !names.is_empty() && want != *on {
                        println!(
                            "  {:<12} {:<4} {} in: {}",
                            "", "",
                            if want { "on" } else { "off" },
                            names.join(", ")
                        );
                    }
                }
            }
        }

        Cmd::Pipelines => {
            let cfg = Config::load(&root, cli.config.as_deref())?;
            for name in cfg.pipelines.keys() {
                println!("{name}");
            }
        }

        Cmd::Plan {
            pipeline,
            agents,
            with_features,
            without_features,
        } => {
            let mut cfg = Config::load(&root, cli.config.as_deref())?;
            // Quietly: the plan names the kind and model each step ends up
            // with, so a line announcing the swap says it a second time.
            override_agents(&mut cfg, &agents, false)?;
            let steps = pipeline_steps(
                &cfg, pipeline.as_deref(), &with_features, &without_features,
            )?;
            if steps.is_empty() {
                println!("this pipeline has no steps");
            }
            print!("{}", plan_text(&cfg, &root, &steps));
        }

        Cmd::Status => {
            let branch = util::current_branch(&root)?;
            println!("repo   {}", root.display());
            println!("branch {branch}");
            let st = Stacks::load(&root)?;
            if st.stacks.is_empty() {
                println!("stacks (none)");
            } else {
                println!();
                for (name, entries) in &st.stacks {
                    println!("{name}");
                    for (i, e) in entries.iter().enumerate() {
                        let marker = if e.branch == branch { "*" } else { " " };
                        let pr = e.pr.map(|n| format!(" #{n}")).unwrap_or_default();
                        let mut state = run_state(&root, e);
                        if state.is_empty() && entry_path(&root, e).is_err() {
                            state = "worktree gone".into();
                        }
                        let state = if state.is_empty() {
                            String::new()
                        } else {
                            format!("  [{state}]")
                        };
                        println!(
                            "  {marker} {}. {:<28} <- {}{}{}",
                            i + 1,
                            e.branch,
                            e.base,
                            pr,
                            state
                        );
                    }
                }
            }
        }

        Cmd::Run {
            task,
            pipeline,
            base,
            from,
            only,
            dry_run,
            agents,
            vars,
            with_features,
            without_features,
            detach,
        } => {
            let opts = RunOpts {
                pipeline, task, base, from, only, dry_run, agents, vars,
                with_features, without_features,
            };
            if detach {
                let branch = util::current_branch(&root)?;
                start_detached(&root, &root, &branch, opts)?;
            } else {
                execute(&root, cli.config.as_deref(), opts)?;
            }
        }

        Cmd::Continue {
            target, pipeline, with_features, without_features, wait, fg, worker,
        } => {
            let st = Stacks::load(&root)?;
            let entry = match target {
                Some(t) => resolve_target(&root, &st, &t)?,
                None => pick_stack(&root, &st)?,
            };
            if let Some(pid) = running_pid(&root, &entry.branch) {
                if !wait {
                    bail!(
                        "`{}` is still running (pid {pid}). Pass --wait to carry on \
                         as soon as it finishes.",
                        entry.branch
                    );
                }
                if !worker {
                    // Detach a waiter rather than holding this terminal - or,
                    // from Slack, a worker thread - for the length of a run.
                    queue_continue(
                        &root, &entry.branch, pipeline.as_deref(),
                        &with_features, &without_features,
                    )?;
                    return Ok(());
                }
                println!("waiting for `{}` to finish (pid {pid})...", entry.branch);
                while running_pid(&root, &entry.branch).is_some() {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
                println!("`{}` finished", entry.branch);
            }
            let dir = entry_path(&root, &entry)?;
            let dirp = std::path::Path::new(&dir);
            let cfg = Config::load(dirp, cli.config.as_deref())?;

            // What to carry on with: what was asked for, else what it ran last.
            let recorded = last_attempted(&root, &entry.branch);
            let pipeline = pipeline
                .or_else(|| recorded.as_ref().and_then(|(p, _)| p.clone()))
                .or_else(|| logged_pipeline(&root, &entry.branch))
                // A pipeline the branch once ran may have been renamed or
                // removed since. Carrying on with the default beats refusing
                // to continue a branch because the config moved on.
                .filter(|p| cfg.pipelines.contains_key(p));
            let steps = pipeline_steps(
                &cfg, pipeline.as_deref(), &with_features, &without_features,
            )?;

            // The log records each step by its description, or its id when it
            // has none - which is how a step is matched across two pipelines
            // that both contain it but number it differently.
            // The recorded id is exact. Runs from before it was recorded only
            // have the log, where a step appears by its description.
            let at = match recorded.map(|(_, id)| id) {
                Some(id) => steps.iter().position(|s| s.id == id).with_context(|| {
                    format!(
                        "`{}` stopped at `{id}`, which is not in pipeline `{}`. \
                         Name a pipeline that has it.",
                        entry.branch,
                        pipeline.as_deref().unwrap_or("steps")
                    )
                })?,
                None => {
                    let last = last_step(&root, &entry.branch).with_context(|| {
                        format!("`{}` has no run to continue from", entry.branch)
                    })?;
                    let label =
                        last.split_once(' ').map_or(last.clone(), |(_, l)| l.to_string());
                    steps
                        .iter()
                        .position(|s| s.description.as_deref().unwrap_or(&s.id) == label)
                        .with_context(|| {
                            format!(
                                "`{}` stopped at `{label}`, which is not in pipeline \
                                 `{}`. Name a pipeline that has it.",
                                entry.branch,
                                pipeline.as_deref().unwrap_or("steps")
                            )
                        })?
                }
            };

            // A step that failed is picked up again; one that finished is
            // stepped over.
            let failed = last_status(&root, &entry.branch).is_some_and(|s| s != "ok");
            let next = if failed { at } else { at + 1 };
            if next >= steps.len() {
                println!(
                    "`{}` has already finished `{}`",
                    entry.branch,
                    pipeline.as_deref().unwrap_or("steps")
                );
                return Ok(());
            }
            let from = steps[next].id.clone();
            println!(
                "continuing `{}` on `{}` from `{from}` ({} step(s) left)",
                entry.branch,
                pipeline.as_deref().unwrap_or("steps"),
                steps.len() - next
            );
            let opts = RunOpts {
                pipeline, from: Some(from), with_features, without_features,
                ..Default::default()
            };
            if fg {
                execute(dirp, None, opts)?;
            } else {
                start_detached(&root, dirp, &entry.branch, opts)?;
            }
        }

        Cmd::Ask { question, new, session, agent } => {
            let question = match question {
                Some(q) => q,
                None => ask("ask> ").context("nothing asked")?,
            };
            let mut cfg = Config::load(&root, cli.config.as_deref())?;
            let role = agent_role(&mut cfg, agent)?;
            let role_for_session = role.clone();
            let acfg = cfg
                .agents
                .get_mut(&role)
                .with_context(|| format!("no agent role `{role}`"))?;
            // A question is not a task: the configured args exist to let an
            // agent edit unattended, which is the opposite of what is wanted
            // here. `plan` is claude's read-only mode; an agent given a
            // `command` is left alone, since rigg cannot know what is safe in
            // someone else's argv.
            if acfg.command.is_some() {
                println!(
                    "warning: role `{role}` has its own `command`, so this cannot be \
                     made read-only. Ask it not to change anything."
                );
            } else {
                acfg.args = match acfg.kind.as_str() {
                    "claude" => vec!["--permission-mode".into(), "plan".into()],
                    // opencode's own read-only agent. Without it the only thing
                    // stopping an ask from editing is the prompt saying not to.
                    "opencode" => vec!["--agent".into(), "plan".into()],
                    _ => Vec::new(),
                };
            }

            let mut vars = BTreeMap::new();
            vars.insert("repo".into(), root.to_string_lossy().to_string());
            vars.insert("config".into(), cfg.dir.to_string_lossy().to_string());
            vars.insert("branch".into(), util::current_branch(&root).unwrap_or_default());
            let step = config::Step {
                id: "ask".into(),
                agent: Some(role),
                // `clear` means "do not resume", so a fresh conversation is
                // what --new asks for.
                clear: new,
                prompt: Some(format!(
                    "Answer this question about the code. Do not change any files, \
                     and do not commit anything - this is a question.\n\n\
                     The answer is read in Slack, which renders neither Markdown \
                     headings nor tables: prefer short paragraphs and bullets, and \
                     cite code as path:line.\n\n{question}"
                )),
                ..Default::default()
            };
            let kind = cfg.agents[&role_for_session].kind.clone();
            let mut runner = pipeline::Runner::new(root.clone(), cfg, vars, false);
            runner.session = session;
            runner.run(&[step])?;
            // Which conversation this was, so a caller can come back to this
            // one rather than to whatever spoke most recently here.
            //
            // Only for claude: the id is read out of claude's own history, so
            // after a turn on another agent it names some earlier claude
            // conversation. A caller that stored that - the Slack bridge stores
            // it per thread - would resume a conversation from another thread.
            if kind == "claude" {
                if let Some(id) = claude_last_session(&root.to_string_lossy()) {
                    println!("session: {id}");
                }
            }
        }

        Cmd::Stop { target } => {
            let st = Stacks::load(&root)?;
            let entry = match target {
                Some(t) => resolve_target(&root, &st, &t)?,
                None => pick_stack(&root, &st)?,
            };
            match running_pid(&root, &entry.branch) {
                None => println!("nothing running on `{}`", entry.branch),
                Some(pid) => {
                    stop_run(&root, &entry.branch, pid)?;
                    println!("stopped `{}`", entry.branch);
                }
            }
        }

        Cmd::New {
            name,
            prompt,
            pipeline,
            base,
            images,
            no_paste,
            agents,
            vars,
            with_features,
            without_features,
            fg,
        } => {
            let _ = &name;
            // A lone argument that reads like a sentence is the task, not a name.
            let (name, prompt) = match (name, prompt) {
                (Some(n), Some(p)) => (n, Some(p)),
                (Some(one), None) if one.trim().contains(char::is_whitespace) => {
                    (util::random_name(), Some(one))
                }
                (Some(n), None) => (n, None),
                (None, p) => (util::random_name(), p),
            };
            let prompt = match prompt {
                Some(p) => Some(p),
                None if fg => None,
                None => Some(ask("task> ").context("no task given")?),
            };
            // The branch a new stack starts is named after the stack itself.
            media::collect(&root, &name, &images, !no_paste)?;
            let path = push_stack(&root, cli.config.as_deref(), &name, base, None, true)?;
            let opts = RunOpts {
                pipeline, task: prompt, agents, vars, with_features, without_features,
                ..Default::default()
            };
            if fg {
                execute(&path, None, opts)?;
            } else {
                start_detached(&root, &path, &name, opts)?;
            }
        }

        Cmd::Adopt {
            branch,
            prompt,
            pipeline,
            base,
            stack,
            images,
            no_paste,
            agents,
            vars,
            with_features,
            without_features,
            fg,
        } => {
            // Before the task is asked for: a name that reaches no branch
            // should say so, not be answered with a question about the task.
            let (branch, remote) = adoptable(&util::main_checkout(&root)?, &branch)?;
            let mut opts = RunOpts {
                pipeline, task: prompt, agents, vars, with_features, without_features,
                ..Default::default()
            };
            // `new` always has a task, because a branch with nothing to do is
            // not worth cutting. Here the branch already exists, so a run that
            // only reviews or previews it needs none - and one is asked for
            // only when a step would otherwise be handed an empty {{task}}.
            if let Some(step) = task_step(&root, cli.config.as_deref(), &opts) {
                if !fg && !std::io::stdin().is_terminal() {
                    bail!("{}", no_task_for(&root, cli.config.as_deref(), &branch, &step));
                }
                if !fg {
                    opts.task = Some(ask("task> ")?);
                }
            }
            media::collect(&root, &branch, &images, !no_paste)?;
            let path = adopt_stack(
                &root, cli.config.as_deref(), &branch, remote.as_deref(), base, stack,
            )?;
            if fg {
                execute(&path, None, opts)?;
            } else {
                start_detached(&root, &path, &branch, opts)?;
            }
        }

        Cmd::Add {
            stack,
            prompt,
            pipeline,
            branch,
            images,
            no_paste,
            agents,
            vars,
            with_features,
            without_features,
            fg,
            worker,
        } => {
            let st = Stacks::load(&root)?;
            // A lone argument is the stack when it names one, otherwise the task.
            let (stack, prompt) = match (stack, prompt) {
                (Some(s), p) if st.stacks.contains_key(&s) => (s, p),
                (Some(task), None) => (pick_stack_name(&root, &st)?, Some(task)),
                (Some(s), Some(_)) => bail!(
                    "no stack `{s}`. Known stacks: {}. Use `rigg new {s} \"...\"` to \
                     start one.",
                    stack_names(&st)
                ),
                (None, p) => (pick_stack_name(&root, &st)?, p),
            };
            // A detached worker has no terminal to ask on, so settle the task here.
            let prompt = match prompt {
                Some(p) => Some(p),
                None if fg => None,
                None => Some(ask("task> ").context("no task given")?),
            };
            let n = st.stacks[&stack].len() + 1;
            let branch = branch.unwrap_or_else(|| format!("{stack}-{n}"));

            // Only the shell that queued the add has a clipboard or a terminal;
            // the worker it detaches re-enters here with the images already
            // staged under the branch it was given.
            if !worker {
                media::collect(&root, &branch, &images, !no_paste)?;
            }

            // Queueing happens in the detached worker, not here, so the shell
            // comes straight back even when the tip is still running.
            if !fg && !worker {
                start_queued_add(
                    &root, &stack, &branch, prompt.as_deref(), pipeline.as_deref(),
                    &agents, &vars, &with_features, &without_features,
                )?;
                return Ok(());
            }

            // The worker owns an entry reserved at queue time; its base is the
            // branch immediately below it in the stack.
            let base = st
                .find(&branch)
                .map(|e| e.base.clone())
                .or_else(|| st.tip_of(&stack).map(|e| e.branch.clone()))
                .context("nothing to build on")?;

            if let Some(pid) = running_pid(&root, &base) {
                println!("waiting for `{base}` to finish (pid {pid})...");
                while running_pid(&root, &base).is_some() {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
                println!("`{base}` finished");
            }
            if let Some(st) = last_status(&root, &base) {
                if st != "ok" {
                    bail!(
                        "not branching `{branch}`: the run on `{base}` did not \
                         succeed ({st}). Fix it, then `rigg add {stack} \"...\"` again."
                    );
                }
            }

            let cfg = Config::load(&root, cli.config.as_deref()).unwrap_or_default();
            // Anything left uncommitted on the base would not reach this branch.
            if let Some(dir) = st.find(&base).and_then(|e| entry_path(&root, e).ok()) {
                let dirty = util::git(std::path::Path::new(&dir), &["status", "--porcelain"])
                    .map(|o| !o.trim().is_empty())
                    .unwrap_or(false);
                if dirty {
                    bail!(
                        "`{base}` has uncommitted changes in {dir}, which `{branch}` \
                         would not include. Its pipeline needs a commit step."
                    );
                }
            }

            println!("creating worktree {branch} on top of {base}");
            let path = create_worktree(&root, &cfg, &branch, &base)?;
            let mut st = Stacks::load(&root)?;
            if let Some(e) = st
                .stacks
                .get_mut(&stack)
                .and_then(|es| es.iter_mut().find(|e| e.branch == branch))
            {
                e.path = Some(path.clone());
            } else {
                st.stacks.entry(stack.clone()).or_default().push(Entry {
                    branch: branch.clone(),
                    base: base.clone(),
                    path: Some(path.clone()),
                    pr: None,
                    adopted: false,
                });
            }
            st.save(&root)?;
            println!("worktree at {path}");

            execute(
                std::path::Path::new(&path),
                None,
                RunOpts {
                    pipeline, task: prompt, agents, vars, with_features,
                    without_features, ..Default::default()
                },
            )?;
        }

        Cmd::Logs { target, follow } => {
            let st = Stacks::load(&root)?;
            let branch = match target {
                Some(t) => resolve_target(&root, &st, &t).map(|e| e.branch).unwrap_or(t),
                None => pick_stack(&root, &st)?.branch,
            };
            let log = log_path(&root, &branch)?;
            if !log.exists() {
                bail!("no log for `{branch}` at {}", log.display());
            }
            if follow {
                let err = std::process::Command::new("tail")
                    .arg("-f")
                    .arg(&log)
                    .exec();
                bail!("could not tail {}: {err}", log.display());
            }
            print!("{}", std::fs::read_to_string(&log)?);
        }

        Cmd::Attach {
            target,
            path: path_only,
            new,
            force,
            wait,
            agent,
        } => {
            let st = Stacks::load(&root)?;
            let entry = match target {
                Some(t) => resolve_target(&root, &st, &t)?,
                // No target: prefer the stack we are standing in, else choose.
                None => match util::current_branch(&root)
                    .ok()
                    .and_then(|b| resolve_target(&root, &st, &b).ok())
                {
                    Some(e) => e,
                    None => pick_stack(&root, &st)?,
                },
            };
            let dir = entry_path(&root, &entry)?;
            if path_only {
                println!("{dir}");
                return Ok(());
            }

            // Claude will not resume a conversation its own process still has
            // open, so attaching mid-run gives "No conversation found to
            // continue" rather than the session you wanted.
            if let Some(pid) = running_pid(&root, &entry.branch) {
                if wait {
                    println!("waiting for the run on `{}` to finish (pid {pid})...", entry.branch);
                    while running_pid(&root, &entry.branch).is_some() {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                    println!("run finished");
                } else if !force {
                    bail!(
                        "`{}` is still running (pid {pid}).\n  \
                         rigg logs {} -f        watch it\n  \
                         rigg attach {} --wait  open it as soon as it finishes\n  \
                         rigg attach {} --force open a second session alongside it",
                        entry.branch,
                        entry.branch,
                        entry.branch,
                        entry.branch
                    );
                } else {
                    println!("warning: a run is still in progress (pid {pid})");
                }
            }

            let cfg = Config::load(std::path::Path::new(&dir), None)?;
            let role = match agent {
                Some(r) => r,
                None => cfg
                    .agents
                    .keys()
                    .next()
                    .cloned()
                    .context("no agents configured")?,
            };
            let kind = cfg
                .agents
                .get(&role)
                .with_context(|| format!("no agent role `{role}`"))?
                .kind
                .clone();

            println!("{} in {dir}", entry.branch);
            let mut cmd = std::process::Command::new(&kind);
            cmd.current_dir(&dir);
            if !new {
                match (kind.as_str(), claude_last_session(&dir)) {
                    // Name the conversation, so it is obvious which one opened.
                    ("claude", Some(id)) => {
                        println!("resuming session {id}");
                        cmd.arg("--resume").arg(id);
                    }
                    ("claude", None) => println!("no previous session here; starting fresh"),
                    _ => {
                        cmd.arg("--continue");
                    }
                }
            }
            // Hand the terminal over: rigg has nothing left to do.
            let err = cmd.exec();
            bail!("could not start `{kind}`: {err}");
        }

        Cmd::Say {
            target,
            message,
            agent,
            no_push,
            images,
            no_paste,
        } => {
            let st = Stacks::load(&root)?;
            let (entry, message) = match (target, message) {
                // Both given: an explicit target and its message.
                (Some(t), Some(m)) => (resolve_target(&root, &st, &t)?, m),
                // One argument is the message; choose who it goes to.
                (Some(m), None) => (pick_stack(&root, &st)?, m),
                // Nothing given: choose, then ask what to say.
                (None, _) => {
                    let e = pick_stack(&root, &st)?;
                    let m = ask(&format!("say to {}> ", e.branch))
                        .context("nothing to say")?;
                    (e, m)
                }
            };
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
            media::collect(&root, &entry.branch, &images, !no_paste)?;
            let attached = media::materialize(&root, &dir, &entry.branch)?;
            let message_for_commit = message.clone();
            let message = media::decorate(&message, &attached);

            let mut vars = BTreeMap::new();
            vars.insert("branch".into(), entry.branch.clone());
            vars.insert("repo".into(), dir.to_string_lossy().to_string());
            vars.insert("config".into(), cfg.dir.to_string_lossy().to_string());
            let step = config::Step {
                id: "say".into(),
                agent: Some(role),
                prompt: Some(message),
                ..Default::default()
            };
            // A say runs in the foreground and prints to whoever asked, so
            // without this the branch's log still shows whatever the last
            // detached run left - which reads as though the say never
            // happened, or worse, failed for a reason belonging to that run.
            let started = std::time::Instant::now();
            note_in_log(&root, &entry.branch, &format!("say: {}", first_line(&message_for_commit)));
            let mut runner = pipeline::Runner::new(dir.clone(), cfg, vars, false);
            // Whoever asked for a `say` is already being answered - by the
            // terminal, or by the bridge that posted the message. Three more
            // lines calling it a one-step pipeline only bury where the branch
            // actually is.
            runner.announce = false;
            let outcome = runner.run(&[step]);
            line_in_log(
                &root,
                &entry.branch,
                &match &outcome {
                    Ok(()) => format!("  ok ({}s)", started.elapsed().as_secs()),
                    Err(e) => format!("  failed: {e}"),
                },
            );
            outcome?;

            if !no_push {
                push_changes(&dir, &entry.branch, &message_for_commit)?;
            }
        }

        Cmd::Stack { cmd } => match cmd {
            StackCmd::Push {
                branch,
                base,
                stack: stack_name,
            } => {
                push_stack(&root, cli.config.as_deref(), &branch, base, stack_name, false)?;
            }
            StackCmd::Rm { stack, yes, force } => {
                let st = Stacks::load(&root)?;
                let name = match stack {
                    Some(n) if st.stacks.contains_key(&n) => n,
                    Some(n) => bail!("no stack `{n}`. Known stacks: {}", stack_names(&st)),
                    None => {
                        let e = pick_stack(&root, &st)?;
                        st.stack_of(&e.branch)
                            .map(str::to_string)
                            .context("that branch belongs to no stack")?
                    }
                };
                remove_stack(&root, &name, yes, force)?;
            }
            StackCmd::Names => {
                for name in Stacks::load(&root)?.stacks.keys() {
                    println!("{name}");
                }
            }
            StackCmd::Prune {
                base,
                yes,
                keep_branches,
            } => prune(&root, cli.config.as_deref(), base, yes, keep_branches)?,
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
                        let pr = e.pr.map(|n| format!(" #{n}")).unwrap_or_default();
                        let mut state = run_state(&root, e);
                        if state.is_empty() && entry_path(&root, e).is_err() {
                            state = "worktree gone".into();
                        }
                        let state = if state.is_empty() {
                            String::new()
                        } else {
                            format!("  [{state}]")
                        };
                        println!(
                            "  {mark} {}. {:<28} <- {}{}{}",
                            i + 1,
                            e.branch,
                            e.base,
                            pr,
                            state
                        );
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
                        util::shell(&root, &label_command(&branch, label))?;
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
    util::expand_home(p)
}

/// Choose a stack interactively: one stack needs no choosing, fzf is used when
/// present, otherwise a numbered list.
fn pick_stack(root: &std::path::Path, st: &Stacks) -> Result<Entry> {
    if st.stacks.is_empty() {
        bail!("no stacks; start one with `rigg new \"<task>\"`");
    }
    let names: Vec<&String> = st.stacks.keys().collect();
    if names.len() == 1 {
        return resolve_target(root, st, names[0]);
    }

    if which("fzf") {
        use std::io::Write as _;
        let mut child = std::process::Command::new("fzf")
            .arg("--prompt=stack> ")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            for n in &names {
                writeln!(stdin, "{n}")?;
            }
        }
        let out = child.wait_with_output()?;
        let choice = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if choice.is_empty() {
            bail!("nothing selected");
        }
        return resolve_target(root, st, &choice);
    }

    for (i, n) in names.iter().enumerate() {
        let target = resolve_target(root, st, n).ok();
        let branch = target.as_ref().map(|e| e.branch.clone()).unwrap_or_default();
        let extra = if &branch == *n {
            String::new()
        } else {
            format!("  ({branch})")
        };
        let state = match target.as_ref().map(|e| run_state(root, e)) {
            Some(s) if !s.is_empty() => format!("  [{s}]"),
            _ => String::new(),
        };
        println!("  {}  {:<28}{extra}{state}", i + 1, n);
    }
    let answer = ask("stack> ").context("no stack chosen")?;
    if let Ok(n) = answer.parse::<usize>() {
        if n >= 1 && n <= names.len() {
            return resolve_target(root, st, names[n - 1]);
        }
    }
    resolve_target(root, st, &answer)
}

/// Append a line to a branch's run log, with a timestamp.
///
/// Used by the things that do work without being a detached run, so that
/// `rigg logs` reflects everything that happened to a branch rather than only
/// the last pipeline.
fn note_in_log(root: &std::path::Path, branch: &str, text: &str) {
    use std::io::Write;
    let Ok(path) = log_path(root, branch) else { return };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let stamp = std::process::Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "\n---- {text} ---- {stamp}");
    }
}

/// Append a plain line, for an outcome rather than a heading.
fn line_in_log(root: &std::path::Path, branch: &str, text: &str) {
    use std::io::Write;
    let Ok(path) = log_path(root, branch) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{text}");
    }
}

/// The first line of a string, for a log header.
fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.chars().count() > 72 {
        format!("{}...", line.chars().take(69).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Where the last attempted step id is kept for a branch.
fn step_path(root: &std::path::Path, branch: &str) -> Result<std::path::PathBuf> {
    Ok(util::state_dir(root)?
        .join("run")
        .join(format!("{}.step", branch.replace('/', "-"))))
}

/// The pipeline a branch last ran, and the step it got to.
fn last_attempted(root: &std::path::Path, branch: &str) -> Option<(Option<String>, String)> {
    let raw = std::fs::read_to_string(step_path(root, branch).ok()?).ok()?;
    let mut lines = raw.lines();
    let pipeline = lines.next()?.trim().to_string();
    let id = lines.next()?.trim().to_string();
    (!id.is_empty()).then(|| ((!pipeline.is_empty()).then_some(pipeline), id))
}

fn status_path(root: &std::path::Path, branch: &str) -> Result<std::path::PathBuf> {
    Ok(util::state_dir(root)?
        .join("run")
        .join(format!("{}.status", branch.replace('/', "-"))))
}

/// How the last run on this branch ended, if it has ended.
fn last_status(root: &std::path::Path, branch: &str) -> Option<String> {
    std::fs::read_to_string(status_path(root, branch).ok()?)
        .ok()
        .map(|s| s.trim().to_string())
}

fn record_status(root: &std::path::Path, branch: &str, outcome: &str) {
    if let Ok(p) = status_path(root, branch) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, outcome);
    }
}

fn pid_path(root: &std::path::Path, branch: &str) -> Result<std::path::PathBuf> {
    Ok(util::state_dir(root)?
        .join("run")
        .join(format!("{}.pid", branch.replace('/', "-"))))
}

/// The pid of a detached run for this branch, if one is still alive.
fn running_pid(root: &std::path::Path, branch: &str) -> Option<u32> {
    let raw = std::fs::read_to_string(pid_path(root, branch).ok()?).ok()?;
    let pid: u32 = raw.trim().parse().ok()?;
    // Confirm it is still our run and not a recycled pid.
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    String::from_utf8_lossy(&cmdline)
        .contains("rigg")
        .then_some(pid)
}

/// The id of claude's most recent conversation for this directory, if any.
///
/// Resuming by id beats `--continue`: it says in the log which conversation was
/// picked, and does not depend on how claude decides what "most recent" means.
fn claude_last_session(dir: &str) -> Option<String> {
    let enc: String = dir
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let home = std::env::var("HOME").ok()?;
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(format!("{home}/.claude/projects/{enc}")).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let when = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(t, _)| when > *t) {
            newest = Some((when, id.to_string()));
        }
    }
    newest.map(|(_, id)| id)
}

/// The last step header a run wrote, as "2/9 apply-copilot-review".
fn last_step(root: &std::path::Path, branch: &str) -> Option<String> {
    let path = log_path(root, branch).ok()?;
    let text = read_tail(&path, 64 * 1024)?;
    let mut found = None;
    for line in text.lines() {
        let t = line.trim_start_matches(['-', ' ']);
        let Some(rest) = t.strip_prefix('[') else { continue };
        let Some((counter, tail)) = rest.split_once(']') else { continue };
        if !counter.contains('/') {
            continue;
        }
        // Strip the rule and timestamp the header is padded with.
        let label = tail.trim().trim_end_matches(|c: char| c == '-' || c.is_whitespace());
        let label = label
            .rsplit_once(' ')
            .filter(|(_, t)| t.len() == 8 && t.contains(':'))
            .map(|(l, _)| l)
            .unwrap_or(label);
        found = Some(format!("{counter} {}", label.trim().trim_end_matches('-').trim()));
    }
    found
}

/// Read at most `max` bytes from the end of a file.
fn read_tail(path: &std::path::Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > max {
        f.seek(SeekFrom::Start(len - max)).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).to_string())
}

/// One-line description of where a stack entry's run got to.
fn run_state(root: &std::path::Path, e: &Entry) -> String {
    let branch = &e.branch;
    if running_pid(root, branch).is_some() {
        // A queued worker is alive but only polling; it makes its worktree
        // once the branch below it finishes, so the absence of one says it has
        // not started.
        if entry_path(root, e).is_err() {
            return format!("queued behind {}", e.base);
        }
        return match last_step(root, branch) {
            Some(step) => format!("running {step}"),
            None => "running".into(),
        };
    }
    match last_status(root, branch).as_deref() {
        Some("ok") => "done".into(),
        // Cancelled on purpose, so not a failure to report as one.
        Some("stopped") => "stopped".into(),
        Some(other) => {
            let short = other.strip_prefix("failed: ").unwrap_or(other);
            format!("failed {}", short.split(" after ").next().unwrap_or(short))
        }
        None => match last_step(root, branch) {
            Some(step) => format!("stopped at {step}"),
            // No path was ever recorded, so this entry never got a worktree -
            // usually because the branch below it failed and the worker
            // refused to build on it.
            None if e.path.is_none() => match last_status(root, &e.base) {
                Some(st) if st != "ok" => format!("not started: `{}` failed", e.base),
                _ => "not started".into(),
            },
            None => String::new(),
        },
    }
}

fn log_path(root: &std::path::Path, branch: &str) -> Result<std::path::PathBuf> {
    Ok(util::state_dir(root)?
        .join("logs")
        .join(format!("{}.log", branch.replace('/', "-"))))
}

/// `origin/<trunk>` when `base` is the trunk, and `base` untouched otherwise.
///
/// The local trunk is only as new as the last time somebody pulled it by hand.
/// A branch cut from it re-solves whatever landed since - and the agent then
/// reports a fix that is already on main, or misses the commit that told it how
/// the repo does this.
///
/// Only the trunk is refreshed. A stack builds on its own tip, and
/// `origin/<tip>` is behind that for every step before the one that pushes -
/// cutting from the remote there would silently drop the work being built on.
fn fresh_base(from: &std::path::Path, cfg: &Config, base: &str) -> String {
    if base != cfg.stack.trunk {
        return base.to_string();
    }
    // A repo with no remote, or no network, still gets a branch: what it cuts
    // from is then the local trunk, which is what it was before this existed.
    let _ = util::git(from, &["fetch", "--quiet", "origin", base]);
    let remote = format!("origin/{base}");
    match util::git(from, &["rev-parse", "--verify", "--quiet", &remote]) {
        Ok(_) => remote,
        Err(_) => base.to_string(),
    }
}

/// Create the worktree for `branch` off `base`, returning its path.
fn create_worktree(
    root: &std::path::Path,
    cfg: &Config,
    branch: &str,
    base: &str,
) -> Result<String> {
    let from = util::main_checkout(root)?;
    if util::git(&from, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .is_ok()
    {
        bail!("branch `{branch}` already exists");
    }
    let dest = worktree_dest(cfg, &from, branch);
    util::git_worktree_add(&from, branch, &fresh_base(&from, cfg, base), &dest)?;
    let mut path = dest.to_string_lossy().to_string();
    if path.is_empty() {
        path = util::worktree_path(&from, branch).unwrap_or_default();
    }
    if path.is_empty() {
        bail!("worktree for `{branch}` was created but its path could not be resolved");
    }
    Ok(path)
}

/// Detach a worker that waits for the stack's tip, then branches and runs.
fn start_queued_add(
    root: &std::path::Path,
    stack: &str,
    branch: &str,
    prompt: Option<&str>,
    pipeline: Option<&str>,
    agents: &[String],
    vars: &[String],
    with_features: &[String],
    without_features: &[String],
) -> Result<()> {
    if let Some(id) = confirm_step(root, None, pipeline) {
        bail!(
            "step `{id}` asks for confirmation, which a detached run cannot do. \
             Run it with --fg, or drop `confirm` from that step."
        );
    }
    // Record the entry now, not when the worker runs, so a further `add` sees
    // this branch as the tip and chains behind it instead of reusing its name.
    let mut st = Stacks::load(root)?;
    let base = st
        .tip_of(stack)
        .map(|e| e.branch.clone())
        .context("stack has no entries to build on")?;
    st.stacks.entry(stack.to_string()).or_default().push(Entry {
        branch: branch.to_string(),
        base: base.clone(),
        path: None,
        pr: None,
        adopted: false,
    });
    st.save(root)?;

    let log = log_path(root, branch)?;
    if let Some(p) = log.parent() {
        std::fs::create_dir_all(p)?;
    }
    let out = std::fs::File::create(&log)?;
    let errs = out.try_clone()?;

    let exe = std::env::current_exe()?;
    let mut args: Vec<String> = vec![
        "add".into(),
        stack.into(),
    ];
    if let Some(p) = prompt {
        args.push(p.to_string());
    }
    args.push("--branch".into());
    args.push(branch.into());
    args.push("--worker".into());
    if let Some(p) = pipeline {
        args.push("--pipeline".into());
        args.push(p.to_string());
    }
    for a in agents {
        args.push("--agent".into());
        args.push(a.clone());
    }
    for v in vars {
        args.push("--var".into());
        args.push(v.clone());
    }
    for f in with_features {
        args.push("--with".into());
        args.push(f.clone());
    }
    for f in without_features {
        args.push("--without".into());
        args.push(f.clone());
    }
    let mut cmd = if which("setsid") {
        let mut c = std::process::Command::new("setsid");
        c.arg(&exe).args(&args);
        c
    } else {
        let mut c = std::process::Command::new(&exe);
        c.args(&args);
        c
    };
    let child = cmd
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(errs)
        .spawn()
        .context("could not queue the run")?;

    // Recorded under the new branch so `stack list` shows it and a further
    // `add` queues behind this one in turn.
    let pid = pid_path(root, branch)?;
    if let Some(p) = pid.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&pid, child.id().to_string())?;

    match running_pid(root, &base) {
        Some(p) => println!("queued `{branch}` behind `{base}` (pid {p})"),
        None => println!("started `{branch}` on top of `{base}`"),
    }
    println!("  rigg logs {branch} -f");
    Ok(())
}

/// Whether this run would stop to ask for a task it has not been given.
///
/// Detached, that question reaches a closed stdin and the run dies a second
/// after being started, leaving a pid file, no output and a log holding one
/// baffling line. Worth catching while there is still a terminal to say it to.
/// The first step of this run whose prompt would be handed an empty
/// `{{task}}`, if any. Named rather than merely counted, because "something
/// wants a task" is not enough to act on.
fn task_step(root: &std::path::Path, cfg_path: Option<&str>, o: &RunOpts) -> Option<String> {
    if o.task.is_some() {
        return None;
    }
    let cfg = Config::load(root, cfg_path).ok()?;
    let mut steps = pipeline_steps(
        &cfg, o.pipeline.as_deref(), &o.with_features, &o.without_features,
    )
    .ok()?;
    if let Some(from) = &o.from {
        if let Some(i) = steps.iter().position(|s| &s.id == from) {
            steps = steps.split_off(i);
        }
    }
    if !o.only.is_empty() {
        steps.retain(|s| o.only.contains(&s.id));
    }
    steps
        .iter()
        .find(|s| {
            s.prompt_text(&cfg.dir)
                .ok()
                .flatten()
                .is_some_and(|p| p.contains("{{task}}"))
        })
        .map(|s| s.id.clone())
}

fn needs_a_task(root: &std::path::Path, cfg_path: Option<&str>, o: &RunOpts) -> bool {
    task_step(root, cfg_path, o).is_some()
}

/// Pipelines that would run this branch without wanting a task - the answer to
/// "then what can I run?", which is otherwise a hunt through the config.
fn taskless_pipelines(root: &std::path::Path, cfg_path: Option<&str>) -> Vec<String> {
    let Ok(cfg) = Config::load(root, cfg_path) else {
        return Vec::new();
    };
    cfg.pipelines
        .keys()
        .filter(|name| {
            task_step(
                root,
                cfg_path,
                &RunOpts { pipeline: Some((*name).clone()), ..Default::default() },
            )
            .is_none()
        })
        .cloned()
        .collect()
}

/// The first step that would stop to ask something, if any.
fn confirm_step(
    root: &std::path::Path,
    cfg_path: Option<&str>,
    pipeline: Option<&str>,
) -> Option<String> {
    let cfg = Config::load(root, cfg_path).ok()?;
    cfg.steps_for(pipeline)
        .ok()?
        .into_iter()
        .find(|s| s.confirm)
        .map(|s| s.id)
}

/// Run the pipeline in the background so the terminal comes straight back.
fn start_detached(
    root: &std::path::Path,
    dir: &std::path::Path,
    branch: &str,
    o: RunOpts,
) -> Result<()> {
    if let Some(id) = confirm_step(root, None, o.pipeline.as_deref()) {
        bail!(
            "step `{id}` asks for confirmation, which a detached run cannot do. \
             Run it with --fg, or drop `confirm` from that step."
        );
    }
    if needs_a_task(dir, None, &o) {
        bail!(
            "this run starts at a step that needs a task, and none was given. \
             Pass --task \"...\", or --from a later step to pick up after the \
             work is done."
        );
    }
    let log = log_path(root, branch)?;
    if let Some(p) = log.parent() {
        std::fs::create_dir_all(p)?;
    }
    let out = std::fs::File::create(&log)?;
    let errs = out.try_clone()?;
    let _ = std::fs::remove_file(status_path(root, branch)?);

    let exe = std::env::current_exe()?;
    let mut args: Vec<String> = vec!["run".into()];
    if let Some(p) = &o.pipeline {
        args.push("--pipeline".into());
        args.push(p.clone());
    }
    if let Some(t) = &o.task {
        args.push("--task".into());
        args.push(t.clone());
    }
    // Where to start was being dropped, so a detached `continue` worked out
    // the right step and then ran the whole pipeline anyway.
    if let Some(f) = &o.from {
        args.push("--from".into());
        args.push(f.clone());
    }
    for id in &o.only {
        args.push("--only".into());
        args.push(id.clone());
    }
    if let Some(b) = &o.base {
        args.push("--base".into());
        args.push(b.clone());
    }
    for a in &o.agents {
        args.push("--agent".into());
        args.push(a.clone());
    }
    for v in &o.vars {
        args.push("--var".into());
        args.push(v.clone());
    }
    for f in &o.with_features {
        args.push("--with".into());
        args.push(f.clone());
    }
    for f in &o.without_features {
        args.push("--without".into());
        args.push(f.clone());
    }
    // setsid detaches from this terminal's session, so the run survives the
    // shell going away.
    let mut cmd = if which("setsid") {
        let mut c = std::process::Command::new("setsid");
        c.arg(&exe).args(&args);
        c
    } else {
        let mut c = std::process::Command::new(&exe);
        c.args(&args);
        c
    };
    let child = cmd
        .current_dir(dir)
        // So every step of the detached run, and anything those spawn, see a
        // PWD that agrees with the directory they are actually in.
        .env("PWD", dir)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(errs)
        .spawn()
        .context("could not start the pipeline in the background")?;

    let pid = pid_path(root, branch)?;
    if let Some(p) = pid.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&pid, child.id().to_string())?;

    println!("started `{branch}` in the background (pid {})", child.id());
    println!("  rigg logs {branch} -f");
    println!("  rigg attach {branch}");
    Ok(())
}

/// Apply `--agent`: `kind` swaps every role, `role=kind` swaps one.
///
/// `args` and `command` go with it, because both are written for the binary
/// being replaced - claude takes `--permission-mode`, opencode does not take
/// it at all, so carrying them over would produce an agent that cannot start.
/// What the new kind cannot work without, it gets: see `default_args`.
fn override_agents(cfg: &mut Config, specs: &[String], announce: bool) -> Result<()> {
    for spec in specs {
        let (roles, kind): (Vec<String>, &str) = match spec.split_once('=') {
            Some((role, kind)) => {
                if !cfg.agents.contains_key(role) {
                    let known: Vec<&str> = cfg.agents.keys().map(|s| s.as_str()).collect();
                    bail!(
                        "no agent role `{role}`; this config has {}",
                        if known.is_empty() { "none".into() } else { known.join(", ") }
                    );
                }
                (vec![role.to_string()], kind)
            }
            None => (cfg.agents.keys().cloned().collect(), spec.as_str()),
        };
        for role in roles {
            let a = cfg.agents.get_mut(&role).expect("checked above");
            if a.kind == kind {
                continue;
            }
            if announce {
                let dropped = !a.args.is_empty() || a.command.is_some();
                let gained = default_args(kind);
                println!(
                    "  role `{role}`: {} -> {kind}{}{}",
                    a.kind,
                    if dropped { ", dropping args written for it" } else { "" },
                    if gained.is_empty() { String::new() } else { format!(", using {}", gained.join(" ")) }
                );
            }
            a.kind = kind.to_string();
            a.args = default_args(kind);
            a.command = None;
        }
    }
    Ok(())
}

/// What a kind needs to edit anything, for a role that was just swapped onto it.
///
/// A swap arrives with no args, and claude with no permission mode does not
/// fail: it describes the change, writes nothing, and exits 0. The step is
/// reported ok and the run dies three steps later at `git push`, saying the
/// branch has no commits - which is true, and not the reason.
fn default_args(kind: &str) -> Vec<String> {
    match kind {
        // auto, not acceptEdits: a pipeline's own steps commit and push, and
        // acceptEdits allows file edits only. Not bypassPermissions either -
        // auto does all three unattended without waiving the rest.
        "claude" => vec!["--permission-mode".into(), "auto".into()],
        _ => Vec::new(),
    }
}

/// The role an `--agent` value names, for a command that runs a single turn.
///
/// It takes every spelling `run --agent` takes - a role, a kind, or `role=kind`
/// - because in Slack they arrive as the same `ai=`, and being told that
/// `opencode` is not a role is not an answer anybody wants.
fn agent_role(cfg: &mut Config, spec: Option<String>) -> Result<String> {
    let default = cfg
        .agents
        .keys()
        .next()
        .cloned()
        .context("no agents configured")?;
    let Some(spec) = spec else { return Ok(default) };
    if cfg.agents.contains_key(&spec) {
        return Ok(spec);
    }
    // Not a role, so a kind: swap the role that would have answered, rather
    // than every role, since only one of them is about to run.
    let spec = if spec.contains('=') { spec } else { format!("{default}={spec}") };
    let role = spec.split('=').next().unwrap_or(&default).to_string();
    override_agents(cfg, &[spec], true)?;
    Ok(role)
}

/// A pipeline's steps with the parts that are switched off removed.
fn pipeline_steps(
    cfg: &Config,
    pipeline: Option<&str>,
    with: &[String],
    without: &[String],
) -> Result<Vec<config::Step>> {
    let mut steps = cfg.steps_for(pipeline)?;
    let features = cfg.features_for(pipeline, with, without)?;
    let off: Vec<&str> = features
        .iter()
        .filter(|(_, on)| !**on)
        .map(|(f, _)| f.as_str())
        .collect();
    if !off.is_empty() {
        steps.retain(|s| s.feature.as_deref().is_none_or(|f| !off.contains(&f)));
    }
    Ok(steps)
}

/// One line per step: what it is, which role runs it, and on what.
///
/// The model is the reason this exists rather than a list of ids: `kind` is
/// rigg's setting and the model is the agent's own, so the two together are
/// what actually answers "what is about to run this".
fn plan_text(cfg: &Config, root: &std::path::Path, steps: &[config::Step]) -> String {
    let width = |f: &dyn Fn(&config::Step) -> usize| steps.iter().map(f).max().unwrap_or(0);
    let idw = width(&|s: &config::Step| s.id.len());
    let rolew = width(&|s: &config::Step| role_of(s).len());
    let mut out = String::new();
    for (i, step) in steps.iter().enumerate() {
        let runner = match &step.agent {
            Some(role) => {
                let a = &cfg.agents[role];
                format!("  {} {}", a.kind, model::model_for(a, root))
            }
            None => String::new(),
        };
        // Whether a step runs at all can depend on the diff, which does not
        // exist yet - so say so rather than promise it.
        let only = match &step.when_changed {
            Some(globs) => format!("  only if {} changed", globs.join(", ")),
            None => String::new(),
        };
        // Left-aligned, so the first line has no leading space for a reader
        // that trims one - the Slack bridge does, and it would take the
        // whole listing out of column.
        let line = format!(
            "{:<2}  {:<idw$}  {:<rolew$}{runner}{only}",
            i + 1,
            step.id,
            role_of(step),
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// A step's role, or what runs it when it has none.
fn role_of(step: &config::Step) -> &str {
    step.agent.as_deref().unwrap_or("shell")
}

/// Which pipeline a branch last ran, from the header of its log.
fn logged_pipeline(root: &std::path::Path, branch: &str) -> Option<String> {
    let text = read_tail(&log_path(root, branch).ok()?, 4096)?;
    let line = text.lines().find(|l| l.starts_with("rigg  "))?;
    let (_, rest) = line.split_once("pipeline ")?;
    let name = rest.split(&[',', ' '][..]).next()?;
    (name != "steps").then(|| name.to_string())
}

/// Detach a process that waits for the run in flight, then continues.
fn queue_continue(
    root: &std::path::Path,
    branch: &str,
    pipeline: Option<&str>,
    with: &[String],
    without: &[String],
) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut args: Vec<String> =
        vec!["continue".into(), branch.into(), "--wait".into(), "--worker".into()];
    if let Some(p) = pipeline {
        args.push("--pipeline".into());
        args.push(p.to_string());
    }
    for f in with {
        args.push("--with".into());
        args.push(f.clone());
    }
    for f in without {
        args.push("--without".into());
        args.push(f.clone());
    }
    let mut cmd = if which("setsid") {
        let mut c = std::process::Command::new("setsid");
        c.arg(&exe).args(&args);
        c
    } else {
        let mut c = std::process::Command::new(&exe);
        c.args(&args);
        c
    };
    // Its own output cannot go to the branch log, which the run it starts
    // truncates - but discarding it leaves a queued continuation with no
    // account of what it decided or why it did nothing.
    let note = step_path(root, branch)?.with_extension("continue.log");
    if let Some(d) = note.parent() {
        std::fs::create_dir_all(d)?;
    }
    let out = std::fs::File::create(&note)?;
    let errs = out.try_clone()?;
    cmd.current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(errs)
        .spawn()
        .context("could not queue the continuation")?;
    println!("queued: `{branch}` will carry on as soon as its run finishes");
    Ok(())
}

/// Stop a detached run and everything it started.
///
/// The run is its own process group - `start_detached` puts it there with
/// setsid - so signalling the group is what reaches the agent and any shell
/// step under it. Killing the pid alone would leave those orphaned and still
/// holding the branch.
fn stop_run(root: &std::path::Path, branch: &str, pid: u32) -> Result<()> {
    let group = format!("-{pid}");
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &group])
        .status();
    // Give it a moment to go down politely before insisting.
    for _ in 0..20 {
        if running_pid(root, branch).is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if running_pid(root, branch).is_some() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &group])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    record_status(root, branch, "stopped");
    if let Ok(p) = pid_path(root, branch) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// Run the configured teardown in a checkout about to be removed.
///
/// Reported but never fatal: the removal was asked for, and a branch whose
/// services will not stop is still a branch the user wants gone. Saying so
/// matters though - a leaked docker stack is invisible until the disk fills.
fn teardown(cfg: &Config, dir: &str, branch: &str, base: &str, dry_run: bool) {
    let Some(cmd) = &cfg.stack.teardown else {
        return;
    };
    let mut vars = BTreeMap::new();
    vars.insert("branch".to_string(), branch.to_string());
    vars.insert("base".to_string(), base.to_string());
    vars.insert("repo".to_string(), dir.to_string());
    vars.insert("config".to_string(), cfg.dir.to_string_lossy().to_string());
    let cmd = util::render(cmd, &vars);
    // Naming the branch, not the command: a teardown is usually a script, and
    // its first line is `set -eu` for every branch alike.
    if dry_run {
        println!("  would run teardown for {branch}");
        return;
    }
    println!("  teardown for {branch}");
    if let Err(e) = util::shell_quiet(std::path::Path::new(dir), &cmd, &[]) {
        println!("  teardown failed for {branch}: {e}");
    }
}

/// Add a label to the PR for `branch`.
///
/// `gh pr edit --add-label` still queries Projects (classic), which GitHub has
/// deprecated, so it fails outright; the REST endpoint avoids that query.
fn label_command(branch: &str, label: &str) -> String {
    format!(
        "set -e\n\
         pr=$(gh pr list --head {branch} --json number --jq '.[0].number')\n\
         gh api \"repos/{{owner}}/{{repo}}/issues/$pr/labels\" -f \"labels[]={label}\" --jq '[.[].name]'\n"
    )
}

/// Wrap on whitespace for the run header.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + word.len() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

/// Choose a stack and return its name.
fn pick_stack_name(root: &std::path::Path, st: &Stacks) -> Result<String> {
    let e = pick_stack(root, st)?;
    st.stack_of(&e.branch)
        .map(str::to_string)
        .context("that branch belongs to no stack")
}

fn stack_names(st: &Stacks) -> String {
    if st.stacks.is_empty() {
        "(none)".into()
    } else {
        st.stacks.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Resolve a branch name to itself, or a stack name to the branch you most
/// likely mean: the one being worked on now, else the newest that exists on
/// disk. The tip of a queued stack is usually still waiting and has no
/// checkout yet, so it is the wrong answer.
fn resolve_target(root: &std::path::Path, st: &Stacks, name: &str) -> Result<Entry> {
    if let Some(entries) = st.stacks.get(name) {
        if let Some(e) = entries
            .iter()
            .find(|e| running_pid(root, &e.branch).is_some() && entry_path(root, e).is_ok())
        {
            return Ok(e.clone());
        }
        if let Some(e) = entries.iter().rev().find(|e| entry_path(root, e).is_ok()) {
            return Ok(e.clone());
        }
        if let Some(e) = entries.last() {
            return Ok(e.clone());
        }
    }
    if let Some(e) = st.find(name) {
        return Ok(e.clone());
    }

    // Nothing matched exactly, so try what a person would have meant. Names
    // carry a random suffix to keep them unique, not to be typed back, and a
    // channel prefix is noise once you know which channel you are in.
    let tail = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    for test in [
        &|t: &str| t.starts_with(name) as _,
        &|t: &str| t.contains(name) as _,
    ] as [&dyn Fn(&str) -> bool; 2]
    {
        let hits: Vec<&String> = st
            .stacks
            .keys()
            .filter(|k| test(&tail(k)) || test(k))
            .collect();
        if hits.len() == 1 {
            return resolve_target(root, st, hits[0]);
        }
        if hits.len() > 1 {
            let names: Vec<&str> = hits.iter().map(|h| h.as_str()).collect();
            bail!("`{name}` matches {} - say which", names.join(", "));
        }
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
    agents: Vec<String>,
    vars: Vec<String>,
    with_features: Vec<String>,
    without_features: Vec<String>,
}

/// Load the config, pick the pipeline and run it against `root`.
fn execute(root: &std::path::Path, cfg_path: Option<&str>, o: RunOpts) -> Result<()> {
    let mut cfg = Config::load(root, cfg_path)?;
    override_agents(&mut cfg, &o.agents, true)?;
    let branch = util::current_branch(root)?;
    let st = Stacks::load(root)?;
    let base = o
        .base
        .or_else(|| st.find(&branch).map(|e| e.base.clone()))
        .unwrap_or_else(|| cfg.stack.trunk.clone());

    let whole = pipeline_steps(
        &cfg, o.pipeline.as_deref(), &o.with_features, &o.without_features,
    )?;
    if whole.is_empty() {
        bail!("pipeline has no steps");
    }
    let mut steps = whole.clone();
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
    missing_secrets(&cfg, &steps)?;

    let mut needs_task = false;
    for step in &steps {
        if let Some(p) = step.prompt_text(&cfg.dir)? {
            if p.contains("{{task}}") {
                needs_task = true;
                break;
            }
        }
    }
    let task = match o.task {
        Some(t) => t,
        None if needs_task => ask("Task: ").context("this pipeline needs a task")?,
        None => String::new(),
    };

    // Not on a dry run: this writes files, and the point of --dry-run is that
    // it touches nothing. It also consumes the staging, which a rehearsal must
    // not do to the real run that follows it.
    let task = if o.dry_run {
        task
    } else {
        let attached = media::materialize(root, root, &branch)?;
        media::decorate(&task, &attached)
    };

    // Config defaults first, then this run's overrides, then the built-ins -
    // which win, so `[vars] branch = ...` cannot shadow the real branch.
    let mut vars = cfg.vars_for(o.pipeline.as_deref());
    for v in &o.vars {
        let (k, val) = v
            .split_once('=')
            .with_context(|| format!("--var wants name=value, got `{v}`"))?;
        vars.insert(k.to_string(), val.to_string());
    }
    vars.insert("task".into(), task);
    vars.insert("branch".into(), branch.clone());
    vars.insert("base".into(), base.clone());
    vars.insert("repo".into(), root.to_string_lossy().to_string());
    // Capability lives in the config repo, the cwd is the target - so a step
    // reaching for a vendored tool must say which root it means.
    vars.insert("config".into(), cfg.dir.to_string_lossy().to_string());

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
    let pipeline_name = o.pipeline.as_deref().unwrap_or("steps");
    println!(
        "rigg  {branch}  (base {base}, pipeline {pipeline_name}, {} steps)",
        steps.len()
    );
    // The full task, not just the truncated echo in the first step's argv.
    let task = vars.get("task").cloned().unwrap_or_default();
    for (i, line) in wrap(task.trim(), 68).into_iter().enumerate() {
        println!("{}  {line}", if i == 0 { "task" } else { "    " });
    }
    // Read off before the config is handed to the runner.
    let push_when_done = cfg.stack.push_when_done;
    let trunk = cfg.stack.trunk.clone();
    let mut runner = pipeline::Runner::new(root.to_path_buf(), cfg, vars, o.dry_run);
    // Number what runs against the whole pipeline, not against the slice.
    // A `continue` that reports `[1/2] wait-copilot` reads as a fresh run of
    // something else; `[8/9]` is a place in the plan the branch started with.
    // Ids are unique within a pipeline, which is what makes this exact.
    runner.numbers = steps
        .iter()
        .filter_map(|s| whole.iter().position(|w| w.id == s.id).map(|i| i + 1))
        .collect();
    runner.total = whole.len();
    let mut outcome = runner.run(&steps);

    // Whatever the run left in the checkout is committed and pushed. A branch
    // is looked at somewhere else - a preview mounts the checkout, a PR is
    // what gets reviewed - so a pipeline that applies a review after it has
    // already pushed leaves the fixes in neither, and nothing says so.
    //
    // Only a branch rigg is managing: `rigg run` in a checkout of your own
    // should not commit what is lying around in it.
    let managed = st.find(&branch).is_some();
    if outcome.is_ok() && push_when_done && !o.dry_run && managed && branch != base {
        let dirty = util::git(root, &["status", "--porcelain"])
            .map(|o| !o.trim().is_empty())
            .unwrap_or(false);
        
        // Check if there are commits to push even when the working directory is clean.
        // This happens when the run committed changes but they haven't been pushed yet.
        let has_commits_to_push = if !dirty {
            let remote = util::git(root, &["remote"]).map(|o| o.trim().to_string()).unwrap_or_default();
            if remote.is_empty() {
                false
            } else {
                let base_ref = format!("{}/{}", remote, trunk);
                let merge_base = util::git(root, &["merge-base", &base_ref, &branch])
                    .map(|o| o.trim().to_string())
                    .unwrap_or_default();
                if merge_base.is_empty() {
                    false
                } else {
                    util::git(root, &["rev-list", "--count", &format!("{merge_base}..{branch}")])
                        .map(|o| o.trim().parse::<usize>().unwrap_or(0) > 0)
                        .unwrap_or(false)
                }
            }
        } else {
            false
        };
        
        if dirty || has_commits_to_push {
            // The task names what the run was for; where there is none - a
            // `continue` that only applies a review - the step that left the
            // work names it better than anything else available.
            let subject = match task.lines().find(|l| !l.trim().is_empty()) {
                Some(l) => l.to_string(),
                None => runner.at.clone().unwrap_or_else(|| "follow-up".into()),
            };
            println!();
            outcome = push_changes(root, &branch, &subject);
        }
    }

    if !o.dry_run {
        // Where it got to, recorded rather than left to be read back out of a
        // log - which a foreground run does not even write.
        if let Some(at) = &runner.at {
            // The pipeline goes with it: continuing needs to know which list
            // that step belonged to, and a foreground run writes no log to
            // read it back out of.
            let record = format!("{}\n{at}\n", o.pipeline.as_deref().unwrap_or(""));
            let _ = step_path(root, &branch).map(|p| {
                p.parent().map(std::fs::create_dir_all);
                std::fs::write(p, record)
            });
        }
        record_status(
            root,
            &branch,
            &match &outcome {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("failed: {e}"),
            },
        );
    }
    outcome?;
    println!("\nrigg: pipeline complete");
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
    if util::git(&from, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .is_ok()
    {
        bail!(
            "branch `{branch}` already exists. Pick another name, or delete it with \
             `git branch -d {branch}` if it is finished."
        );
    }
    let dest = worktree_dest(&cfg, &from, branch);
    util::git_worktree_add(&from, branch, &base, &dest)?;
    let mut path = dest.to_string_lossy().to_string();
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
        adopted: false,
    });
    st.save(root)?;
    println!("stack `{target}` is now {} deep", st.stacks[&target].len());
    println!("worktree at {path}");
    Ok(std::path::PathBuf::from(path))
}

/// The branch `adopt` should take, and the remote-tracking ref to create it
/// from when it is not local yet. Settled before anything is created, so a
/// name that reaches no branch is reported as that rather than as whatever
/// the next step happens to notice.
fn adoptable(from: &std::path::Path, branch: &str) -> Result<(String, Option<String>)> {
    let head = |b: &str| {
        util::git(from, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).is_ok()
    };
    // `origin/feature/login` is how a branch is named when you have just read
    // it off `git branch -a` or a PR page, so take the remote off rather than
    // reporting that no such branch exists.
    let mut branch = branch.to_string();
    if !head(&branch) {
        let split = branch.split_once('/').map(|(a, b)| (a.to_string(), b.to_string()));
        if let Some((remote, rest)) = split {
            let known = util::git(from, &["remote"])
                .map(|o| o.lines().any(|r| r.trim() == remote))
                .unwrap_or(false);
            let tracked = util::git(
                from,
                &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{branch}")],
            )
            .is_ok();
            if known && tracked && !rest.is_empty() {
                println!("`{branch}` is a remote-tracking name; the branch is `{rest}`");
                branch = rest;
            }
        }
    }
    if head(&branch) {
        return Ok((branch, None));
    }
    // A branch only on a remote is worth adopting too - that is the colleague's
    // PR case - so it is materialised as a local branch tracking it.
    match util::remote_branch(from, &branch) {
        Some(r) => Ok((branch, Some(r))),
        None => bail!(
            "no branch `{branch}`, here or on a remote. `git fetch` first if it \
             is someone else's, or `rigg new {branch} \"...\"` to start it."
        ),
    }
}

/// How rigg is being addressed, for commands suggested back to whoever is
/// reading them. A terminal types `rigg`; the Slack bridge sets this to
/// `@rigg`, since a command line is no use in a message.
fn addressed_as() -> String {
    std::env::var("RIGG_ADDRESSED_AS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "rigg".into())
}

/// Why an `adopt` cannot run, and the two ways out of it.
///
/// "A step wants a task" is true and useless on its own: it names neither the
/// step nor what to do instead. A branch that already exists is usually
/// adopted to be looked at rather than worked on, so the pipelines that want
/// nothing said to them are the likely answer and are listed.
fn no_task_for(
    root: &std::path::Path,
    cfg_path: Option<&str>,
    branch: &str,
    step: &str,
) -> String {
    let me = addressed_as();
    let mut out = format!(
        "`{step}` is the step that hands the agent a task, and this run has \
         none. `{branch}` already exists, so there is nothing for rigg to \
         infer one from.\n\n\
         Say what to do with it:\n  \
         {me} adopt {branch} <what to do>\n"
    );
    let free = taskless_pipelines(root, cfg_path);
    if free.is_empty() {
        out.push_str(
            "\nEvery pipeline here has a step that wants a task. One that only \
             reviews or previews the branch would need none - see `rigg features` \
             for the parts to leave out.",
        );
    } else {
        out.push_str("\nOr run one that only looks at the branch as it stands:\n");
        for name in &free {
            out.push_str(&format!("  {me} {name} adopt {branch}\n"));
        }
    }
    out
}

/// Take an existing branch into a stack: a worktree on the branch itself,
/// recorded so every other verb reaches it by name.
///
/// The difference from `push_stack` is that nothing is created. The branch is
/// someone's already - a colleague's PR, something you cut by hand - and the
/// point is to put a pipeline on it as it stands: a preview, a review, and
/// then `add` to carry it further.
fn adopt_stack(
    root: &std::path::Path,
    cfg_path: Option<&str>,
    branch: &str,
    remote: Option<&str>,
    base: Option<String>,
    stack_name: Option<String>,
) -> Result<std::path::PathBuf> {
    let cfg = Config::load(root, cfg_path).unwrap_or_default();
    let mut st = Stacks::load(root)?;
    let from = util::main_checkout(root)?;

    if let Some(name) = st.stack_of(branch) {
        bail!(
            "`{branch}` is already in stack `{name}`. Take it further with \
             `rigg continue {name}`, or talk to it with `rigg say {name} \"...\"`."
        );
    }

    let target = stack_name.unwrap_or_else(|| branch.to_string());
    let is_new = !st.stacks.contains_key(&target);
    let base = base
        .or_else(|| st.tip_of(&target).map(|e| e.branch.clone()))
        .unwrap_or_else(|| cfg.stack.trunk.clone());
    if util::git(&from, &["rev-parse", "--verify", "--quiet", &base]).is_err() {
        bail!("no ref `{base}` to stack `{branch}` on; pass --base <ref>");
    }
    // The base is what a review diffs against and what a PR targets, so a base
    // sharing no history with the branch would quietly review the whole repo.
    if util::git(&from, &["merge-base", &base, remote.unwrap_or(branch)]).is_err() {
        println!(
            "note: `{branch}` and `{base}` share no history, so a step that \
             diffs them sees everything. Pass --base <ref> if that is wrong."
        );
    }

    // git allows a branch in one worktree at a time, so an existing checkout is
    // the one to use rather than a collision to report - unless it is the main
    // one, which is not rigg's to take over.
    let existing = util::worktree_path(&from, branch);
    if existing.as_deref().map(std::path::Path::new) == Some(from.as_path()) {
        bail!(
            "`{branch}` is checked out in the main repo at {}, so it cannot \
             have a worktree of its own. Switch that checkout to another \
             branch, then adopt it again.",
            from.display()
        );
    }

    if is_new {
        println!("starting stack `{target}` on the existing branch {branch} (base {base})");
    } else {
        println!("adding `{branch}` to stack `{target}` (base {base})");
    }
    let path = match existing {
        Some(p) => {
            println!("using the worktree already on `{branch}` at {p}");
            p
        }
        None => {
            let dest = worktree_dest(&cfg, &from, branch);
            util::git_worktree_checkout(&from, branch, remote, &dest)?;
            let mut path = dest.to_string_lossy().to_string();
            if path.is_empty() {
                path = util::worktree_path(&from, branch).unwrap_or_default();
            }
            if path.is_empty() {
                bail!("worktree for `{branch}` was created but its path could not be resolved");
            }
            println!("worktree at {path}");
            path
        }
    };

    st.stacks.entry(target.clone()).or_default().push(Entry {
        branch: branch.to_string(),
        base,
        path: Some(path.clone()),
        pr: None,
        adopted: true,
    });
    st.save(root)?;
    println!("stack `{target}` is now {} deep", st.stacks[&target].len());
    Ok(std::path::PathBuf::from(path))
}

/// Commit and push whatever a turn left behind, so it reaches the PR.
fn push_changes(dir: &std::path::Path, branch: &str, message: &str) -> Result<()> {
    let changed = util::git(dir, &["status", "--porcelain"])?;
    if changed.trim().is_empty() {
        // No uncommitted changes, but check if the branch is ahead of the base
        // and needs to be pushed.
        let remote = util::git(dir, &["remote"]).map(|o| o.trim().to_string()).unwrap_or_default();
        if remote.is_empty() {
            println!("no remote to push to; committed on {branch}");
            return Ok(());
        }
        
        // Check if branch is ahead of the base (main or the stack base).
        // Use 'git merge-base' to find the common ancestor, then count commits
        // that are on the branch but not on the base.
        let base = util::git(dir, &["rev-parse", "--abbrev-ref", "HEAD@{u}"])
            .map(|o| o.trim().to_string())
            .ok()
            .and_then(|up| up.split('/').last().map(|b| b.to_string()))
            .unwrap_or_else(|| "main".to_string());
        
        let merge_base = util::git(dir, &["merge-base", &base, branch])
            .map(|o| o.trim().to_string())
            .unwrap_or_default();
        
        if merge_base.is_empty() {
            println!("no remote to push to; committed on {branch}");
            return Ok(());
        }
        
        let count = util::git(dir, &["rev-list", "--count", &format!("{merge_base}..{branch}")])
            .map(|o| o.trim().parse::<usize>().unwrap_or(0))
            .unwrap_or(0);
        
        if count > 0 {
            println!("pushing {count} commit(s)");
            util::git(dir, &["push", "-u", "origin", branch])?;
            println!("pushed {branch}");
            return Ok(());
        }
        
        println!("nothing changed, so nothing to push");
        return Ok(());
    }
    // The request makes a better commit subject than anything generic.
    let subject: String = message
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("follow-up")
        .trim()
        .chars()
        .take(72)
        .collect();
    println!("committing {} change(s)", changed.lines().count());
    util::git(dir, &["add", "-A"])?;
    util::git(dir, &["commit", "-m", &subject])?;
    // A repo with no remote is not a failure to push, it is nothing to push
    // to. The commit is still the right thing to have made.
    if util::git(dir, &["remote"]).map(|o| o.trim().is_empty()).unwrap_or(true) {
        println!("no remote to push to; committed on {branch}");
        return Ok(());
    }
    util::git(dir, &["push", "-u", "origin", branch])?;
    println!("pushed {branch}");
    Ok(())
}

/// Remove every branch of a stack, newest first.
fn remove_stack(root: &std::path::Path, name: &str, yes: bool, force: bool) -> Result<()> {
    let mut st = Stacks::load(root)?;
    let entries = st.stacks.get(name).cloned().unwrap_or_default();
    let cfg = Config::load(root, None).unwrap_or_default();
    let trunk = format!("origin/{}", cfg.stack.trunk);
    let trunk = if util::git(root, &["rev-parse", "--verify", "--quiet", &trunk]).is_ok() {
        trunk
    } else {
        cfg.stack.trunk.clone()
    };

    println!("stack `{name}`");
    let mut blocked = Vec::new();
    for e in entries.iter().rev() {
        let dir = entry_path(root, e).ok();
        let running = running_pid(root, &e.branch).is_some();
        let dirty = dir
            .as_deref()
            .and_then(|d| util::git(std::path::Path::new(d), &["status", "--porcelain"]).ok())
            .map(|o| o.lines().count())
            .unwrap_or(0);
        // Only a branch rigg would delete has to be on the trunk first. An
        // adopted one is kept either way, so its commits are not at stake.
        let unmerged = !e.adopted
            && util::git(
                root,
                &["merge-base", "--is-ancestor", &format!("refs/heads/{}", e.branch), &trunk],
            )
            .is_err();

        let mut notes = Vec::new();
        if running {
            notes.push("a run is going".to_string());
        }
        if dirty > 0 {
            notes.push(format!("{dirty} uncommitted"));
        }
        if unmerged {
            notes.push("not on the trunk".to_string());
        }
        if !notes.is_empty() && !force {
            blocked.push(e.branch.clone());
        }
        let verdict = if notes.is_empty() || force { "remove" } else { "KEEP  " };
        // Said here rather than only when it happens: "remove" against a
        // branch rigg is not going to delete would otherwise read as a threat.
        if e.adopted {
            notes.push("adopted, so the branch itself is kept".to_string());
        }
        println!(
            "  {verdict} {}{}",
            e.branch,
            if notes.is_empty() {
                String::new()
            } else {
                format!("  ({})", notes.join(", "))
            }
        );
    }

    if !blocked.is_empty() {
        println!(
            "\n{} branch(es) hold work that is not on the trunk, uncommitted, or still \
             running. Pass --force to remove them anyway.",
            blocked.len()
        );
        return Ok(());
    }
    if !yes {
        for e in entries.iter().rev() {
            if let Ok(dir) = entry_path(root, e) {
                teardown(&cfg, &dir, &e.branch, &e.base, true);
            }
        }
        println!("\ndry run; pass --yes to remove");
        return Ok(());
    }

    // Newest first: a branch below is the base of the one above it.
    for e in entries.iter().rev() {
        if let Some(dir) = entry_path(root, e).ok() {
            teardown(&cfg, &dir, &e.branch, &e.base, false);
            let mut args = vec!["worktree", "remove", &dir];
            if force {
                args.push("--force");
            }
            if let Err(err) = util::git(root, &args) {
                println!("  could not remove {}: {err}", e.branch);
                continue;
            }
        }
        media::discard(root, &e.branch);
        // An adopted branch was here before rigg was: give the checkout back
        // and leave the branch, which is someone else's to delete.
        if e.adopted {
            println!("  removed the checkout for {} (branch kept: adopted)", e.branch);
            continue;
        }
        let flag = if force { "-D" } else { "-d" };
        match util::git(root, &["branch", flag, &e.branch]) {
            Ok(_) => println!("  removed {}", e.branch),
            Err(err) => println!("  removed {} (branch kept: {err})", e.branch),
        }
    }
    st.stacks.remove(name);
    st.save(root)?;
    println!("\nstack `{name}` is gone");
    Ok(())
}

/// Remove worktrees for branches already merged into the trunk.
fn prune(
    root: &std::path::Path,
    cfg_path: Option<&str>,
    base: Option<String>,
    yes: bool,
    keep_branches: bool,
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
            // Its *work* has to be on the trunk. A branch that never
            // committed is contained in the trunk as well, trivially, and
            // pruning that as landed work is not the same thing at all.
            let merged = util::ever_committed(root, &branch)
                && util::git(
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
        for (p, b) in &candidates {
            teardown(&cfg, p, b, &trunk, true);
        }
        println!("dry run; pass --yes to remove them");
        return Ok(());
    }

    let in_use = util::cwds_in_use();
    // An adopted branch is kept even when its worktree goes: rigg did not
    // create it, so it is not rigg's to delete.
    let stacks = Stacks::load(root)?;
    let mut removed = 0;
    let mut failed = 0;
    for (p, b) in &candidates {
        // Leave a checkout alone while it is someone's working directory.
        if in_use.contains(p) {
            println!("skipped {b}: in use by a running process");
            continue;
        }
        teardown(&cfg, p, b, &trunk, false);
        match util::git(root, &["worktree", "remove", p]) {
            Ok(_) => {
                removed += 1;
                // The branch is an ancestor of the trunk, so `-d` is safe: it
                // refuses anything not actually merged.
                let adopted = stacks.find(b).is_some_and(|e| e.adopted);
                if !keep_branches && !adopted {
                    let _ = util::git(root, &["branch", "-d", b]);
                }
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


/// Refuse a run whose roles are missing a token, before any turn is spent.
fn corpus_cmd(root: &std::path::Path, cfg_path: Option<&str>, cmd: Option<CorpusCmd>) -> Result<()> {
    match cmd.unwrap_or(CorpusCmd::List) {
        CorpusCmd::List => {
            let all = corpus::discover();
            if all.is_empty() {
                println!("no corpus sidecars found");
                println!("  a sidecar is a directory with a corpus.toml; set RIGG_CORPUS_PATH to look elsewhere");
                return Ok(());
            }
            for m in &all {
                let state = if corpus::enabled(root, m) { "wired in" } else { "available" };
                println!("  {:<12} {:<10} {}", m.name, state, m.description.as_deref().unwrap_or(""));
                println!("  {:<12} {}", "", m.dir.display());
            }
            println!("\n  rigg corpus enable <name>");
        }
        CorpusCmd::Enable { name, dry_run } => {
            let m = corpus::find(&name)?;
            let done = corpus::enable(root, &m, dry_run)?;
            if dry_run {
                println!("would copy  {} -> {}", m.dir.display(), done.tools_dir.display());
                println!("would write {}\n", done.mcp_file.display());
                print!("{}", corpus::mcp_json(&m));
                println!("\nwould append to {}:", done.config_file.display());
                print!("{}", done.wiring);
                return Ok(());
            }
            println!("copied   {} file(s) -> {}", done.copied, done.tools_dir.display());
            println!("wrote    {}", done.mcp_file.display());
            println!("appended {}", done.config_file.display());
            println!("\nNothing is committed - read the diff before you do.");

            let have = instance::secrets();
            let missing: Vec<&String> = m
                .needs
                .iter()
                .filter(|n| !have.contains_key(*n) && !instance::set_in_env(n))
                .collect();
            if !missing.is_empty() {
                println!("\nThen give instance `{}` what it needs:", instance::name());
                for n in missing {
                    println!("  rigg secret set {n} <value>");
                }
            }
            if m.backfill_command.is_some() {
                println!("\nFirst import, in the foreground so you can watch it:");
                println!("  rigg run --pipeline {}-backfill --var since=<date>", m.name);
            }
            if let (Some(_), Some(sched)) = (&m.sync_command, &m.sync_schedule) {
                println!("\nThen keep it fresh:");
                println!("  rigg cron add \"{sched}\" --pipeline {}-sync", m.name);
            }
        }
        CorpusCmd::Status { name } => {
            let all = match name {
                Some(n) => vec![corpus::find(&n)?],
                None => corpus::discover(),
            };
            if all.is_empty() {
                println!("no corpus sidecars found");
                return Ok(());
            }
            let have = instance::secrets();
            let cfg = Config::load(root, cfg_path).ok();
            for m in &all {
                println!("{}", m.name);
                println!("  wiring   {}", if corpus::enabled(root, m) { "in this repo" } else { "NOT in this repo - rigg corpus enable" });
                for n in &m.needs {
                    let s = if have.contains_key(n) {
                        format!("{} ({})", instance::mask(&have[n]), instance::provenance(n))
                    } else if instance::set_in_env(n) {
                        "from the environment".into()
                    } else {
                        "MISSING - rigg secret set".into()
                    };
                    println!("  {n:<8} {s}");
                }
                match corpus::db_path(m) {
                    Some(p) if p.exists() => {
                        let age = std::fs::metadata(&p)
                            .and_then(|md| md.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .map(|d| format!("{} min ago", d.as_secs() / 60))
                            .unwrap_or_else(|| "?".into());
                        let size = std::fs::metadata(&p).map(|m| m.len() / 1024).unwrap_or(0);
                        println!("  data     {size} KB, last written {age}");
                        println!("           {}", p.display());
                    }
                    Some(p) => println!("  data     none yet at {}", p.display()),
                    None => {}
                }
                if let Some(c) = &cfg {
                    let jobs = cron::load().unwrap_or_default();
                    let mine: Vec<&cron::Job> = jobs
                        .iter()
                        .filter(|j| j.pipeline.as_deref().map(|p| p.starts_with(&m.name)).unwrap_or(false))
                        .collect();
                    let _ = c;
                    if mine.is_empty() {
                        println!("  schedule none");
                    } else {
                        for j in mine {
                            println!(
                                "  schedule {} {} ({})",
                                j.schedule,
                                j.pipeline.as_deref().unwrap_or(""),
                                j.last_status.as_deref().unwrap_or("not run yet")
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn missing_secrets(cfg: &Config, steps: &[config::Step]) -> Result<()> {
    let have = instance::secrets();
    let mut missing: Vec<(String, String)> = Vec::new();
    for step in steps {
        let Some(role) = &step.agent else { continue };
        let Some(a) = cfg.agents.get(role) else { continue };
        for n in &a.needs {
            if !have.get(n).map(|v| !v.trim().is_empty()).unwrap_or(false)
                && !instance::set_in_env(n)
            {
                missing.push((role.clone(), n.clone()));
            }
        }
    }
    missing.dedup();
    if missing.is_empty() {
        return Ok(());
    }
    let mut msg = String::new();
    for (role, n) in &missing {
        msg.push_str(&format!("role `{role}` needs {n}, which instance `{}` does not have\n", instance::name()));
    }
    for (_, n) in &missing {
        msg.push_str(&format!("  rigg secret set {n} <value>\n"));
    }
    bail!("{}", msg.trim_end());
}

/// The main checkout of the repo at `path`, or of the one we are standing in.
fn a_repo(path: Option<String>) -> Result<std::path::PathBuf> {
    let root = match path {
        Some(p) => std::fs::canonicalize(&p).with_context(|| format!("{p} is not there"))?,
        None => util::repo_root()?,
    };
    // Register the main checkout, never a worktree: schedules are read from
    // there, so registering a branch would tie them to that branch's life.
    Ok(util::main_checkout(&root).unwrap_or(root))
}

fn config_repo_cmd(cmd: Option<ConfigRepoCmd>) -> Result<()> {
    match cmd.unwrap_or(ConfigRepoCmd::Show) {
        ConfigRepoCmd::Set { path } => {
            let p = std::path::PathBuf::from(util::expand_home(&path));
            let p = std::fs::canonicalize(&p)
                .with_context(|| format!("{} is not there", p.display()))?;
            if !Config::candidates(&p).iter().any(|c| c.exists()) {
                bail!("{} has no .rigg/rigg.toml - run `rigg init` in it first", p.display());
            }
            instance::set_config_repo(&p)?;
            let cfg = Config::load(&p, None)?;
            println!("instance `{}` reads capability from {}", instance::name(), p.display());
            println!(
                "  {} pipeline(s), {} target(s), {} schedule(s)",
                cfg.pipelines.len(),
                cfg.targets.len(),
                cfg.schedules.len()
            );
        }
        ConfigRepoCmd::Clear => {
            if instance::clear_config_repo()? {
                println!("cleared; rigg reads .rigg/ from the repo you stand in again");
            } else {
                println!("no config repo was set");
            }
        }
        ConfigRepoCmd::Show => match instance::config_repo() {
            None => {
                println!("no config repo: rigg reads .rigg/ from the repo you stand in");
                println!("  set one:  rigg config-repo set <path>");
            }
            Some(p) => {
                println!("instance {}  {}", instance::name(), p.display());
                match Config::load(&p, None) {
                    Err(e) => println!("  does not load: {e:#}"),
                    Ok(cfg) => {
                        println!();
                        for (name, t) in &cfg.targets {
                            let d = t.dir();
                            let state = if d.is_dir() { "" } else { "   MISSING" };
                            println!("  target {name:<14} {}{state}", d.display());
                        }
                        if cfg.targets.is_empty() {
                            println!("  no [targets.*] declared");
                        }
                    }
                }
            }
        },
    }
    Ok(())
}

/// The bridge holds no channel map of its own; it asks for this.
fn channels_cmd(cfg_path: Option<&str>, json: bool) -> Result<()> {
    let root = match instance::config_repo() {
        Some(c) => c,
        None => util::repo_root()?,
    };
    let cfg = Config::load(&root, cfg_path)?;
    if json {
        let out: Vec<serde_json::Value> = cfg
            .channels
            .iter()
            .map(|(name, c)| {
                let dir = cfg.target_dir(c.target.as_deref()).ok();
                serde_json::json!({
                    "channel": name,
                    "target": c.target,
                    "repo": dir.map(|d| d.display().to_string()),
                    "commands": c.commands,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if cfg.channels.is_empty() {
        println!("no [channels.*] in {}", cfg.dir.display());
        return Ok(());
    }
    for (name, c) in &cfg.channels {
        let dir = cfg.target_dir(c.target.as_deref());
        let where_ = match dir {
            Ok(d) if d.is_dir() => d.display().to_string(),
            Ok(d) => format!("{}   MISSING", d.display()),
            Err(e) => format!("{e:#}"),
        };
        let verbs = match &c.commands {
            Some(v) => format!("  [{}]", v.join(" ")),
            None => String::new(),
        };
        println!("  #{name:<20} {where_}{verbs}");
    }
    Ok(())
}

fn repo_cmd(cmd: Option<RepoCmd>) -> Result<()> {
    match cmd.unwrap_or(RepoCmd::List) {
        RepoCmd::Add { path } => {
            let root = a_repo(path)?;
            if instance::add_repo(&root)? {
                println!("instance `{}` now covers {}", instance::name(), root.display());
                let n = crate::config::Config::load(&root, None)
                    .map(|c| c.schedules.len())
                    .unwrap_or(0);
                println!("  {n} declared schedule(s) - rigg cron");
            } else {
                println!("{} was already registered", root.display());
            }
        }
        RepoCmd::Rm { path } => {
            let root = a_repo(path)?;
            if instance::rm_repo(&root)? {
                println!("{} removed; its declared schedules no longer run", root.display());
            } else {
                bail!("{} is not registered to instance `{}`", root.display(), instance::name());
            }
        }
        RepoCmd::List => {
            let list = instance::repos();
            println!("instance {}  {}", instance::name(), instance::repos_path().display());
            if list.is_empty() {
                println!("\nno repos covered:  rigg repo add");
                return Ok(());
            }
            println!();
            for r in list {
                let n = crate::config::Config::load(&r, None)
                    .map(|c| c.schedules.len())
                    .unwrap_or(0);
                let state = if r.is_dir() { format!("{n} schedule(s)") } else { "MISSING".into() };
                println!("  {:<60} {state}", r.display());
            }
        }
    }
    Ok(())
}

fn secret_cmd(cmd: Option<SecretCmd>) -> Result<()> {
    match cmd.unwrap_or(SecretCmd::List) {
        SecretCmd::Set { name, value } => {
            instance::set_secret(&name, &value)?;
            println!("{name} set for instance `{}`", instance::name());
            println!("{}", instance::local_path().display());
        }
        SecretCmd::Rm { name } => {
            if instance::rm_secret(&name)? {
                println!("{name} removed");
            } else if instance::provenance(&name) == "from the vault" {
                bail!("{name} comes from the vault; remove it there");
            } else {
                bail!("{name} is not set for instance `{}`", instance::name());
            }
        }
        SecretCmd::List => {
            let all = instance::secrets();
            println!("instance {}  {}", instance::name(), instance::dir().display());
            if all.is_empty() {
                println!("\nno secrets yet:  rigg secret set NAME value");
                return Ok(());
            }
            println!();
            for (k, v) in &all {
                println!("  {k:<28} {:<10} {}", instance::mask(v), instance::provenance(k));
            }
        }
    }
    Ok(())
}

/// Split a typed schedule from the task that followed it.
fn split_schedule(words: &[String]) -> Result<(String, Option<String>)> {
    if words.is_empty() {
        bail!("a schedule, and what to run:  rigg cron add \"0 7 * * *\" --pipeline morning-mail");
    }
    if words[0].starts_with('@') {
        let task = words[1..].join(" ");
        return Ok((words[0].clone(), (!task.is_empty()).then_some(task)));
    }
    // A quoted schedule arrives as one word; an unquoted one as five.
    let parts: Vec<&str> = words[0].split_whitespace().collect();
    if parts.len() == 5 {
        let task = words[1..].join(" ");
        return Ok((words[0].trim().to_string(), (!task.is_empty()).then_some(task)));
    }
    if words.len() < 5 {
        bail!(
            "a schedule is five fields - minute hour day month weekday - and this has {}",
            words.len()
        );
    }
    let task = words[5..].join(" ");
    Ok((words[..5].join(" "), (!task.is_empty()).then_some(task)))
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 24 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

fn cron_cmd(cmd: Option<CronCmd>) -> Result<()> {
    match cmd.unwrap_or(CronCmd::List { channel: None }) {
        CronCmd::Add { words, pipeline, task, new, repo, vars, channel, id } => {
            let (schedule, typed) = split_schedule(&words)?;
            let expr = cron::Expr::parse(&schedule)?;
            let task = task.or(typed);
            if pipeline.is_none() && task.is_none() {
                bail!("nothing to run: give it --pipeline, or a task after the schedule");
            }
            let repo = match repo {
                Some(r) => std::fs::canonicalize(&r)
                    .with_context(|| format!("{r} is not there"))?
                    .display()
                    .to_string(),
                None => util::repo_root()?.display().to_string(),
            };
            let mut jobs = cron::load()?;
            let base = id.unwrap_or_else(|| {
                slug(pipeline.as_deref().or(task.as_deref()).unwrap_or("job"))
            });
            let mut name = base.clone();
            let mut n = 2;
            while jobs.iter().any(|j| j.id == name) {
                name = format!("{base}-{n}");
                n += 1;
            }
            let job = cron::Job {
                id: name.clone(),
                schedule: schedule.clone(),
                repo,
                pipeline,
                task,
                kind: if new { "new".into() } else { "run".into() },
                vars,
                channel,
                source: None,
                paused: false,
                created: Some(cron::now(None)?.stamp()),
                last_run: None,
                last_status: None,
                last_detail: None,
                pid: None,
            };
            println!("{}  {}", job.id, job.describe());
            let next = expr.next(&cron::now(None)?, 3);
            println!("  {schedule}");
            for w in &next {
                println!("  next  {}", w.stamp());
            }
            jobs.push(job);
            cron::save(&jobs)?;
            if !crontab_has_tick() {
                println!("\nNothing is knocking yet. Add one crontab line:");
                println!("  * * * * * {} {} tick", cron_sh_path(), instance::name());
            }
        }
        CronCmd::Edit { id, schedule, pipeline, task, repo, vars, channel, pause, resume } => {
            let mut jobs = cron::load()?;
            let found = cron::find(&jobs, &id)?;
            let target = found.id.clone();
            // A declaration lives in a repo and is changed there, in a diff.
            // Pausing is instance intent, so that one is still allowed.
            if let Some(src) = found.source.clone() {
                let only_state = schedule.is_none() && pipeline.is_none() && task.is_none()
                    && repo.is_none() && vars.is_empty() && channel.is_none();
                if !only_state {
                    bail!(
                        "`{target}` is declared in {}/.rigg/rigg.toml - edit it there.\n\
                         Only --pause and --resume apply here.",
                        src
                    );
                }
            }
            if let Some(s) = &schedule {
                cron::Expr::parse(s)?;
            }
            let job = jobs.iter_mut().find(|j| j.id == target).expect("just found");
            if let Some(s) = schedule {
                job.schedule = s;
                // A schedule that had stopped parsing is worth a clean slate.
                if job.last_status.as_deref() == Some("bad-schedule") {
                    job.last_status = None;
                    job.last_detail = None;
                }
            }
            if let Some(p) = pipeline {
                job.pipeline = if p.is_empty() { None } else { Some(p) };
            }
            if let Some(t) = task {
                job.task = if t.is_empty() { None } else { Some(t) };
            }
            if let Some(r) = repo {
                job.repo = std::fs::canonicalize(&r)
                    .with_context(|| format!("{r} is not there"))?
                    .display()
                    .to_string();
            }
            if !vars.is_empty() {
                job.vars = vars;
            }
            if let Some(c) = channel {
                job.channel = if c.is_empty() { None } else { Some(c) };
            }
            if pause {
                job.paused = true;
            }
            if resume {
                job.paused = false;
            }
            let line = format!("{}  {}  {}", job.id, job.schedule, job.describe());
            let paused = job.paused;
            cron::save(&jobs)?;
            println!("{line}{}", if paused { "  [paused]" } else { "" });
        }
        CronCmd::Rm { id } => {
            let mut jobs = cron::load()?;
            let found = cron::find(&jobs, &id)?;
            if let Some(src) = &found.source {
                bail!(
                    "`{}` is declared in {}/.rigg/rigg.toml - remove it there.",
                    found.id, src
                );
            }
            let target = found.id.clone();
            jobs.retain(|j| j.id != target);
            cron::save(&jobs)?;
            println!("{target} removed");
        }
        CronCmd::Run { id, fired } => {
            let jobs = cron::load()?;
            let target = cron::find(&jobs, &id)?.id.clone();
            cron::run_job(&target, fired)?;
        }
        CronCmd::Show { id } => {
            let jobs = cron::load()?;
            let job = cron::find(&jobs, &id)?;
            println!("{}", job.id);
            println!("  schedule  {}{}", job.schedule, if job.paused { "  [paused]" } else { "" });
            match &job.source {
                Some(src) => println!("  declared  {src}/.rigg/rigg.toml"),
                None => println!("  declared  no - typed with `rigg cron add`"),
            }
            if let Some(c) = &job.channel {
                println!("  reports   {c}");
            }
            println!("  runs      rigg {}", job.argv().join(" "));
            println!("  in        {}", job.repo);
            if let Some(w) = cron::Expr::parse(&job.schedule).ok().and_then(|e| e.next(&cron::now(None).ok()?, 1).into_iter().next()) {
                println!("  next      {}", w.stamp());
            }
            if let Some(r) = &job.last_run {
                println!("  last      {r}  {}", job.last_status.as_deref().unwrap_or("?"));
            }
            if let Some(d) = &job.last_detail {
                println!("            {d}");
            }
            let log = cron::log_path(&job.id);
            if log.exists() {
                println!("  log       {}", log.display());
            }
        }
        CronCmd::Export { id } => {
            let jobs = cron::load()?;
            let j = cron::find(&jobs, &id)?;
            let short = j.id.rsplit('/').next().unwrap_or(&j.id);
            println!("# paste into {}/.rigg/rigg.toml, then:  rigg cron rm {}", j.repo, j.id);
            println!("[[schedules]]");
            println!("id       = \"{short}\"");
            println!("cron     = \"{}\"", j.schedule);
            if let Some(p) = &j.pipeline {
                println!("pipeline = \"{p}\"");
            }
            if let Some(t) = &j.task {
                println!("task     = \"{}\"", t.replace('"', "\\\""));
            }
            if let Some(c) = &j.channel {
                println!("channel  = \"{c}\"");
            }
            if j.kind != "run" {
                println!("kind     = \"{}\"", j.kind);
            }
            if !j.vars.is_empty() {
                let pairs: Vec<String> = j
                    .vars
                    .iter()
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| format!("{k} = \"{v}\""))
                    .collect();
                println!("vars     = {{ {} }}", pairs.join(", "));
            }
        }
        CronCmd::Names => {
            for j in cron::load()? {
                println!("{}", j.id);
            }
        }
        CronCmd::List { channel } => {
            let mut jobs = cron::load()?;
            if let Some(want) = &channel {
                // The bridge asks per channel, so a listing in #julies-rigg
                // does not invite editing #carls-rigg's jobs. A job that names
                // no channel reports to the instance default and belongs to
                // nobody in particular - hiding it from every listing is how
                // it becomes invisible, so it is shown everywhere instead.
                let want = want.trim_start_matches('#');
                jobs.retain(|j| match j.channel.as_deref() {
                    Some(c) => c.trim_start_matches('#') == want,
                    None => true,
                });
            }
            println!("instance {}  {}", instance::name(), instance::cron_path().display());
            if jobs.is_empty() {
                match &channel {
                    Some(c) => println!("\nnothing scheduled for {c}"),
                    None => println!(
                        "\nnothing scheduled:  rigg cron add \"0 7 * * *\" --pipeline morning-mail"
                    ),
                }
                return Ok(());
            }
            let now = cron::now(None)?;
            println!();
            for j in &jobs {
                let next = match cron::Expr::parse(&j.schedule) {
                    Ok(e) => e
                        .next(&now, 1)
                        .first()
                        .map(|w| w.stamp())
                        .unwrap_or_else(|| "never".into()),
                    Err(_) => "BAD SCHEDULE".into(),
                };
                let next = if j.paused { "paused".to_string() } else { next };
                let mark = if j.declared() { " ·" } else { "  " };
                let where_to = match (&channel, &j.channel) {
                    (Some(_), None) => "  (instance default channel)",
                    _ => "",
                };
                println!("{mark} {:<22} {:<16} {:<18} {}{where_to}", j.id, j.schedule, next, j.describe());
                if let (Some(r), Some(st)) = (&j.last_run, &j.last_status) {
                    println!("  {:<20} last {r}  {st}{}", "", j.last_detail.as_deref().map(|d| format!("  {d}")).unwrap_or_default());
                }
            }
            if jobs.iter().any(|j| j.declared()) {
                println!("\n  ·  declared in a repo - edit the file, not `rigg cron`");
            }
            if !crontab_has_tick() {
                println!("\nNothing is knocking. Add:  * * * * * {} {} tick", cron_sh_path(), instance::name());
            }
        }
    }
    Ok(())
}

fn cron_sh_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent()?.parent()?.parent().map(|r| r.join("cron.sh")))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/path/to/rigg/cron.sh".into())
}

/// Whether anything is actually knocking. A schedule nobody calls is the one
/// failure that looks exactly like a job that has not come round yet.
fn crontab_has_tick() -> bool {
    std::process::Command::new("crontab")
        .arg("-l")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(" tick"))
        .unwrap_or(false)
}

fn tick_cmd(at: Option<&str>, dry_run: bool) -> Result<()> {
    let fired = cron::tick(at, dry_run)?;
    for f in &fired {
        println!("{}: {}", f.id, f.why);
    }
    Ok(())
}

fn doctor(root: &std::path::Path, cfg_path: Option<&str>) -> Result<()> {
    let mut problems = 0;
    let cfg = Config::load(root, cfg_path);

    // Which of the two roots is which is the thing most worth saying first:
    // every relative path in the config is read against one of them.
    match instance::config_repo() {
        None => println!("config repo    none - reading .rigg/ from {}", root.display()),
        Some(c) => {
            if Config::candidates(&c).iter().any(|p| p.exists()) {
                println!("config repo    {}", c.display());
            } else {
                println!("config repo    {} has no .rigg/rigg.toml - IGNORED", c.display());
                problems += 1;
            }
        }
    }
    if let Ok(c) = &cfg {
        for (name, t) in &c.targets {
            let d = t.dir();
            let label = format!("  target {name}");
            if d.is_dir() {
                let here = c
                    .target_for(root)
                    .map(|(n, _)| n == name)
                    .unwrap_or(false);
                let mark = if here { "   <- you are here" } else { "" };
                println!("{label:<14} {}{mark}", d.display());
            } else {
                println!("{label:<14} {} MISSING", d.display());
                problems += 1;
            }
        }
    }

    // Each step is one non-interactive turn, so the only thing to check per
    // role is that the binary it would spawn exists.
    let bins: Vec<(String, String)> = match &cfg {
        Ok(c) => c
            .agents
            .iter()
            .map(|(role, a)| (role.clone(), headless::binary(a)))
            .collect(),
        Err(_) => vec![("?".into(), "claude".into())],
    };

    for (role, bin) in &bins {
        let label = format!("agent {role}");
        if which(bin) {
            println!("{label:<14} {bin} found");
        } else {
            println!("{label:<14} {bin} NOT ON PATH");
            problems += 1;
        }
    }

    let have = instance::secrets();
    println!("instance       {} ({})", instance::name(), instance::dir().display());
    if let Ok(c) = &cfg {
        // One line per secret, not per role that wants it.
        let mut wanted: Vec<(&String, &String)> = Vec::new();
        for (role, a) in &c.agents {
            for n in &a.needs {
                if !wanted.iter().any(|(_, w)| *w == n) {
                    wanted.push((role, n));
                }
            }
        }
        for (role, n) in wanted {
            let label = format!("secret {n}");
            if have.get(n).map(|v| !v.trim().is_empty()).unwrap_or(false) {
                println!("{label:<14} {} ({})", instance::mask(&have[n]), instance::provenance(n));
            } else if instance::set_in_env(n) {
                println!("{label:<14} from the environment");
            } else {
                println!("{label:<14} MISSING - role `{role}` needs it");
                println!("{:<14} rigg secret set {n} <value>", "");
                problems += 1;
            }
        }
    }

    if let Ok(c) = &cfg {
        for (name, a) in &c.approvals {
            let label = format!("approval {name}");
            let missing: Vec<&String> = a
                .needs
                .iter()
                .filter(|n| {
                    !have.get(*n).map(|v| !v.trim().is_empty()).unwrap_or(false)
                        && !instance::set_in_env(n)
                })
                .collect();
            if a.run.is_none() {
                println!("{label:<14} no `run` - approving it does nothing");
                problems += 1;
            } else if missing.is_empty() {
                println!("{label:<14} ready");
            } else {
                // Not a problem: a dry run is the designed behaviour, and
                // saying so beforehand is the point of declaring `needs`.
                println!(
                    "{label:<14} dry run - {} not set",
                    missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
        }
    }

    // A schedule the repo declares, and whether this instance will run it.
    if let Ok(c) = &cfg {
        if !c.schedules.is_empty() {
            let main = util::main_checkout(root).unwrap_or_else(|_| root.to_path_buf());
            // With a config repo set there is only one place schedules are
            // read from, so they run by definition - there is no second
            // registration to forget, and saying otherwise while `rigg cron`
            // lists them firing is the confusing kind of wrong.
            let covered = instance::config_repo().is_some()
                || instance::repos().iter().any(|r| *r == main);
            if !covered {
                println!("schedules      {} declared, NOT RUN - rigg repo add", c.schedules.len());
                problems += 1;
            }
            let now = cron::now(None).ok();
            for sch in &c.schedules {
                let label = format!("  {}", sch.id);
                let next = match (cron::Expr::parse(&sch.cron), &now) {
                    (Ok(e), Some(n)) => e
                        .next(n, 1)
                        .first()
                        .map(|w| w.stamp())
                        .unwrap_or_else(|| "never".into()),
                    _ => "BAD SCHEDULE".into(),
                };
                let where_ = match c.target_dir(sch.target.as_deref()) {
                    Ok(d) if d.is_dir() => sch.target.clone().unwrap_or_else(|| "config repo".into()),
                    Ok(_) => format!("{} MISSING", sch.target.as_deref().unwrap_or("?")),
                    Err(_) => format!("no target {}", sch.target.as_deref().unwrap_or("?")),
                };
                if covered {
                    println!("{label:<14} {} next {next}  -> {where_}", sch.cron);
                } else {
                    println!("{label:<14} {}  -> {where_}", sch.cron);
                }
            }
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
        Ok(c) => {
            let n = c.steps_for(None).map(|s| s.len()).unwrap_or(0);
            let named: Vec<&str> = c.pipelines.keys().map(|s| s.as_str()).collect();
            let which = c.default_pipeline.as_deref().unwrap_or("steps");
            if named.is_empty() {
                println!("config         ok ({n} steps)");
            } else {
                println!(
                    "config         ok (default `{which}`: {n} steps; pipelines: {})",
                    named.join(", ")
                );
            }
        }
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
        bail!("nothing entered");
    }
    Ok(line)
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn label_command_is_valid_shell() {
        let cmd = super::label_command("my-branch", "preview");
        assert!(cmd.contains("issues/$pr/labels"), "{cmd}");
        // gh's {owner}/{repo} placeholders must survive verbatim.
        assert!(cmd.contains("repos/{owner}/{repo}"), "{cmd}");
        let out = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&cmd)
            .output()
            .expect("sh");
        assert!(out.status.success(), "invalid shell: {cmd}");
    }

    fn repo(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir()
            .join(format!("rigg-base-{tag}-{}", crate::util::random_name()));
        std::fs::create_dir_all(&d).unwrap();
        crate::util::git(&d, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        d
    }

    /// A branch cut from the local trunk is a branch cut from the last time
    /// somebody pulled. The remote-tracking ref is what "off main" means.
    #[test]
    fn the_trunk_is_taken_from_the_remote() {
        let origin = repo("origin");
        std::fs::write(origin.join("f"), "one").unwrap();
        crate::util::git(&origin, &["add", "-A"]).unwrap();
        crate::util::git(&origin, &["-c", "user.email=t@t", "-c", "user.name=t",
                                    "commit", "-qm", "one"]).unwrap();

        let local = repo("local");
        crate::util::git(&local, &["remote", "add", "origin",
                                   &origin.to_string_lossy()]).unwrap();
        crate::util::git(&local, &["fetch", "--quiet", "origin", "main"]).unwrap();
        crate::util::git(&local, &["reset", "--hard", "--quiet", "origin/main"]).unwrap();

        // main moves on the remote and nobody pulls.
        std::fs::write(origin.join("f"), "two").unwrap();
        crate::util::git(&origin, &["-c", "user.email=t@t", "-c", "user.name=t",
                                    "commit", "-qam", "two"]).unwrap();

        let cfg = Config::default();
        assert_eq!(cfg.stack.trunk, "main");
        let base = super::fresh_base(&local, &cfg, "main");
        assert_eq!(base, "origin/main");
        let head = crate::util::git(&local, &["rev-parse", &base]).unwrap();
        let remote = crate::util::git(&origin, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head.trim(), remote.trim(), "the fetch has to have happened");

        std::fs::remove_dir_all(&origin).ok();
        std::fs::remove_dir_all(&local).ok();
    }

    /// A stack builds on its own tip, which is ahead of `origin/<tip>` for
    /// every step before the one that pushes.
    #[test]
    fn a_stacked_base_is_left_alone() {
        let local = repo("stack");
        let cfg = Config::default();
        assert_eq!(super::fresh_base(&local, &cfg, "some-branch"), "some-branch");
        std::fs::remove_dir_all(&local).ok();
    }

    /// No remote is not a reason to refuse a branch.
    #[test]
    fn a_repo_with_no_remote_still_cuts_from_its_trunk() {
        let local = repo("bare");
        let cfg = Config::default();
        assert_eq!(super::fresh_base(&local, &cfg, "main"), "main");
        std::fs::remove_dir_all(&local).ok();
    }
}
