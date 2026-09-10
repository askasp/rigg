use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::instance;

/// What a corpus sidecar says about itself, in its `corpus.toml`.
///
/// rigg holds no list of kinds. A sidecar describes how to run it and which
/// secret it needs, and `enable` turns that into repo config - so a new kind
/// is a new directory, not a change here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Relative to the sidecar directory.
    pub mcp_command: String,
    #[serde(default)]
    pub sync_command: Option<String>,
    #[serde(default)]
    pub backfill_command: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    /// Defaults to `mcp__<name>`.
    #[serde(default)]
    pub tools: Option<String>,
    #[serde(default)]
    pub sync_schedule: Option<String>,
    /// Filename inside the instance's corpus directory, for freshness.
    #[serde(default)]
    pub db: Option<String>,
    #[serde(default)]
    pub agent_kind: Option<String>,
    /// A starting prompt for a pipeline that reads the corpus.
    #[serde(default)]
    pub ask_prompt: Option<String>,
    #[serde(skip)]
    pub dir: PathBuf,
}

impl Manifest {
    pub fn tools(&self) -> String {
        self.tools.clone().unwrap_or_else(|| format!("mcp__{}", self.name))
    }

    pub fn kind(&self) -> String {
        self.agent_kind.clone().unwrap_or_else(|| "claude".into())
    }

    /// Where the sidecar sits once it is in the repo. Generated config points
    /// here and nowhere else - an absolute path into whoever ran `enable`
    /// would only work on their machine.
    /// Where the sidecar sits, as rigg's own config text says it. rigg renders
    /// `{{config}}` before running a step, so this reaches the config repo
    /// rather than whatever checkout the step happens to run in.
    pub fn vendored(&self) -> String {
        format!("{{{{config}}}}/tools/{}", self.name)
    }

    /// The same path for a reader that is not rigg. An mcp json is read by the
    /// agent, which expands `${VAR}` and knows nothing of `{{config}}` - and a
    /// relative path there resolves against the target, which is how a server
    /// silently starts from a stale copy in the repo being worked on.
    pub fn vendored_env(&self) -> String {
        format!("${{RIGG_CONFIG}}/tools/{}", self.name)
    }

    fn rel(&self, cmd: &str) -> String {
        let (bin, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
        let path = format!("{}/{bin}", self.vendored());
        if rest.is_empty() { path } else { format!("{path} {rest}") }
    }

    pub fn load(dir: &Path) -> Result<Manifest> {
        let path = dir.join("corpus.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("{} is not a corpus sidecar", dir.display()))?;
        let mut m: Manifest = toml::from_str(&text)
            .with_context(|| format!("reading {}", path.display()))?;
        m.dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        Ok(m)
    }
}

/// Where to look for sidecars: `RIGG_CORPUS_PATH`, else beside the binary's
/// own checkout, so the ones shipped with rigg are found without setting it.
fn search_path() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("RIGG_CORPUS_PATH") {
        if !v.trim().is_empty() {
            return v.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| Some(p.parent()?.parent()?.parent()?.to_path_buf()))
        .into_iter()
        .collect()
}

/// Every sidecar on the search path.
pub fn discover() -> Vec<Manifest> {
    let mut out = Vec::new();
    for root in search_path() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let d = e.path();
            if d.join("corpus.toml").is_file() {
                if let Ok(m) = Manifest::load(&d) {
                    if !out.iter().any(|o: &Manifest| o.name == m.name) {
                        out.push(m);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn find(want: &str) -> Result<Manifest> {
    let as_path = Path::new(want);
    if as_path.join("corpus.toml").is_file() {
        return Manifest::load(as_path);
    }
    let all = discover();
    if let Some(m) = all.into_iter().find(|m| m.name == want) {
        return Ok(m);
    }
    let names: Vec<String> = discover().into_iter().map(|m| m.name).collect();
    if names.is_empty() {
        bail!("no corpus sidecar called `{want}`, and none found on RIGG_CORPUS_PATH");
    }
    bail!("no corpus sidecar called `{want}`; found: {}", names.join(", "))
}

fn config_dir(root: &Path) -> PathBuf {
    root.join(".rigg")
}

/// Is this corpus already wired into the repo?
pub fn enabled(root: &Path, m: &Manifest) -> bool {
    let toml_path = crate::config::Config::path_for(root);
    std::fs::read_to_string(toml_path)
        .map(|s| s.contains(&format!("[agents.{}]", m.name)))
        .unwrap_or(false)
}

pub fn db_path(m: &Manifest) -> Option<PathBuf> {
    m.db.as_ref().map(|f| instance::corpus_dir().join(f))
}

/// The config `enable` appends. Kept as one string so `--dry-run` prints
/// exactly what would be written.
pub fn wiring(m: &Manifest) -> String {
    let mut s = String::new();
    s.push_str(&format!("\n# corpus `{}`, wired by `rigg corpus enable`.\n", m.name));
    if let Some(d) = &m.description {
        s.push_str(&format!("# {d}\n"));
    }
    s.push_str(&format!("[agents.{}]\n", m.name));
    s.push_str(&format!("kind = \"{}\"\n", m.kind()));
    if !m.needs.is_empty() {
        let list: Vec<String> = m.needs.iter().map(|n| format!("\"{n}\"")).collect();
        s.push_str(&format!("needs = [{}]\n", list.join(", ")));
    }
    s.push_str(&format!(
        "args = [\"--permission-mode\", \"auto\",\n        \"--mcp-config\", \".rigg/mcp/{}.json\", \"--strict-mcp-config\",\n        \"--allowedTools\", \"{}\"]\n",
        m.name,
        m.tools()
    ));
    if let Some(c) = &m.sync_command {
        s.push_str(&format!(
            "\n[[pipelines.{}-sync.steps]]\nid = \"sync\"\nrun = \"{}\"\n",
            m.name,
            m.rel(c)
        ));
    }
    if let Some(c) = &m.backfill_command {
        s.push_str(&format!(
            "\n[[pipelines.{}-backfill.steps]]\nid = \"backfill\"\nrun = \"{}\"\n",
            m.name,
            m.rel(c)
        ));
    }
    if let Some(p) = &m.ask_prompt {
        s.push_str(&format!(
            "\n[[pipelines.{}-ask.steps]]\nid = \"ask\"\nagent = \"{}\"\nclear = true\nprompt = \"\"\"\n{}\n\"\"\"\n",
            m.name,
            m.name,
            p.trim()
        ));
    }
    s
}

pub fn mcp_json(m: &Manifest) -> String {
    format!(
        "{{\n  \"mcpServers\": {{\n    \"{}\": {{ \"command\": \"{}/{}\" }}\n  }}\n}}\n",
        m.name,
        m.vendored_env(),
        m.mcp_command
    )
}

pub struct Enabled {
    pub mcp_file: PathBuf,
    pub config_file: PathBuf,
    pub tools_dir: PathBuf,
    pub copied: usize,
    pub wiring: String,
}

fn copy_tree(from: &Path, to: &Path) -> Result<usize> {
    let mut n = 0;
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        // Caches and local state are not part of the tool.
        if name == "__pycache__" || name == ".git" {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        if src.is_dir() {
            n += copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)
                .with_context(|| format!("copying {}", src.display()))?;
            // cp keeps the mode; std::fs::copy does too, but be explicit about
            // the one that matters: a wrapper nobody can execute.
            if let Ok(md) = src.metadata() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &dst,
                    std::fs::Permissions::from_mode(md.permissions().mode()),
                );
            }
            n += 1;
        }
    }
    Ok(n)
}

/// Write the wiring into the repo, leaving it uncommitted to be read.
pub fn enable(root: &Path, m: &Manifest, dry_run: bool) -> Result<Enabled> {
    if enabled(root, m) {
        bail!(
            "`{}` is already wired into {}",
            m.name,
            crate::config::Config::path_for(root).display()
        );
    }
    let cfg = crate::config::Config::path_for(root);
    if !cfg.exists() {
        bail!("no {} - run `rigg init` first", cfg.display());
    }
    let mcp_file = config_dir(root).join("mcp").join(format!("{}.json", m.name));
    let tools_dir = config_dir(root).join("tools").join(&m.name);
    let wiring = wiring(m);
    let mut copied = 0;
    if !dry_run {
        // The tool goes into the repo that uses it, so the config can name it
        // relatively and the checkout is a complete description of what rigg
        // can do here. Diverging from upstream afterwards is allowed.
        copied = copy_tree(&m.dir, &tools_dir)
            .with_context(|| format!("copying {} into the repo", m.name))?;
        if let Some(d) = mcp_file.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&mcp_file, mcp_json(m))
            .with_context(|| format!("writing {}", mcp_file.display()))?;
        let mut body = std::fs::read_to_string(&cfg)?;
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&wiring);
        std::fs::write(&cfg, body).with_context(|| format!("writing {}", cfg.display()))?;
    }
    Ok(Enabled { mcp_file, config_file: cfg, tools_dir, copied, wiring })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Manifest {
        Manifest {
            name: "mail".into(),
            description: Some("Front conversations".into()),
            mcp_command: "mcp.sh".into(),
            sync_command: Some("run.sh".into()),
            backfill_command: Some("run.sh --backfill --since {{since}}".into()),
            needs: vec!["FRONT_API_TOKEN".into()],
            tools: None,
            sync_schedule: Some("*/15 * * * *".into()),
            db: Some("mail.db".into()),
            agent_kind: None,
            ask_prompt: None,
            dir: PathBuf::from("/opt/rigg/mail"),
        }
    }

    #[test]
    fn the_tool_prefix_defaults_to_the_name() {
        assert_eq!(m().tools(), "mcp__mail");
    }

    #[test]
    fn a_command_points_at_the_config_repo_without_losing_its_arguments() {
        assert_eq!(
            m().rel("run.sh --backfill --since {{since}}"),
            "{{config}}/tools/mail/run.sh --backfill --since {{since}}"
        );
        assert_eq!(m().rel("run.sh"), "{{config}}/tools/mail/run.sh");
    }

    #[test]
    fn an_mcp_command_uses_the_variable_the_agent_expands() {
        // A `./` here resolves against the target repo, so the agent would
        // start whatever stale copy that checkout happens to have - which
        // looks exactly like the server working.
        let j = mcp_json(&m());
        assert!(!j.contains("./.rigg"), "{j}");
        assert!(j.contains("${RIGG_CONFIG}"), "{j}");
        assert!(!j.contains("{{config}}"), "{j}");
    }

    #[test]
    fn the_wiring_names_the_secret_and_the_tools() {
        let w = wiring(&m());
        assert!(w.contains("needs = [\"FRONT_API_TOKEN\"]"), "{w}");
        assert!(w.contains("--allowedTools\", \"mcp__mail\""), "{w}");
        assert!(w.contains("[[pipelines.mail-sync.steps]]"), "{w}");
        assert!(w.contains("{{config}}/tools/mail/run.sh --backfill"), "{w}");
        assert!(!w.contains("/opt/rigg"), "no absolute path may reach the repo: {w}");
    }

    #[test]
    fn the_wiring_parses_as_toml() {
        let w = wiring(&m());
        toml::from_str::<toml::Value>(&w).expect("wiring is not valid toml");
    }

    #[test]
    fn the_mcp_key_is_the_tool_prefix() {
        let j = mcp_json(&m());
        assert!(j.contains("\"mail\": { \"command\": \"${RIGG_CONFIG}/tools/mail/mcp.sh\" }"), "{j}");
    }
}
