"""Credentials an instance's runs can see, set from Slack.

Named `creds` rather than `secrets` deliberately: this directory is on
`sys.path`, and a module called `secrets` here would shadow the standard
library one for everything in the process, slack_bolt included.

    @rigg secret FRONT_API_TOKEN fr_live_…
    @rigg secret                      # what is set, never what it is
    @rigg secret rm FRONT_API_TOKEN

Two files, one per instance, both sourced by everything a run touches:

    ~/.rigg/secrets/<instance>.env         Ansible writes this from the vault
    ~/.rigg/secrets/<instance>.local.env   this writes this one

They are separate on purpose. The Ansible role templates the first from
`vault_rigg_tokens`, so anything written there from chat would vanish on the
next converge. The local file is never templated, and it wins when both set the
same name — you set it here because you wanted it now.

The value reaches the next run without restarting anything: the bridge merges
these files into the environment each time it starts rigg, rather than
inheriting them once at boot.

**Slack keeps the message.** A bot cannot delete someone else's message, so a
token sent this way is in the workspace history until a person removes it.
That is the trade for setting one without a deploy, and it is why this refuses
to run outside a private channel and why the reply says to rotate.
"""

import os
import re
import stat
from pathlib import Path

NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")


def instance() -> str:
    return os.environ.get("RIGG_INSTANCE") or "default"


def _dir() -> Path:
    return Path(os.environ.get("RIGG_SECRETS_DIR") or (Path.home() / ".rigg" / "secrets"))


def managed_path(inst: str | None = None) -> Path:
    """The one Ansible owns. Read here, never written."""
    return _dir() / f"{inst or instance()}.env"


def local_path(inst: str | None = None) -> Path:
    return _dir() / f"{inst or instance()}.local.env"


def _read(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if not path.exists():
        return out
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, _, v = line.partition("=")
        out[k.strip()] = v.strip()
    return out


def local() -> dict[str, str]:
    return _read(local_path())


def managed() -> dict[str, str]:
    return _read(managed_path())


def current() -> dict[str, str]:
    """Everything a run should see. The local file wins on a clash."""
    return {**managed(), **local()}


def _write_local(values: dict[str, str]) -> None:
    d = _dir()
    d.mkdir(parents=True, exist_ok=True)
    os.chmod(d, stat.S_IRWXU)  # 0700
    p = local_path()
    body = [
        "# Set from Slack. Ansible does not template this file, so what is here",
        "# survives a converge. `secret rm <NAME>` removes one.",
        "",
    ]
    body += [f"{k}={v}" for k, v in sorted(values.items())]
    tmp = p.with_suffix(".env.tmp")
    tmp.write_text("\n".join(body) + "\n")
    os.chmod(tmp, stat.S_IRUSR | stat.S_IWUSR)  # 0600 before it has a real name
    tmp.replace(p)


def mask(value: str) -> str:
    return f"…{value[-4:]}" if len(value) > 8 else "set"


def command(repo, prefix, rest, say, channel_id, *, is_private) -> None:
    """`secret` — list, set or remove this instance's credentials."""
    rest = (rest or "").strip()
    here = local()
    from_vault = managed()

    if not rest:
        if not here and not from_vault:
            say(
                "nothing set for this instance.\n"
                "`secret <NAME> <value>` — e.g. `secret FRONT_API_TOKEN fr_live_…`\n"
                "Names only ever come back, never values."
            )
            return
        lines = []
        for name in sorted(set(here) | set(from_vault)):
            if name in here:
                where = "set here" + (", overriding the vault" if name in from_vault else "")
            else:
                where = "from the vault"
            # Not `here.get(name, from_vault[name])`: the default is evaluated
            # whether it is needed or not, so a name set only here would raise.
            value = here[name] if name in here else from_vault[name]
            lines.append(f"`{name}`  {mask(value)}  ({where})")
        say("credentials this instance can use:\n" + "\n".join(lines)
            + "\n\n`secret rm <NAME>` removes one set here.")
        return

    head, _, tail = rest.partition(" ")

    if head in ("rm", "remove", "delete"):
        name = tail.strip()
        if name not in here:
            if name in from_vault:
                say(f"`{name}` comes from the vault, not from here — "
                    f"remove it in Ansible (`vault_rigg_tokens`).")
            else:
                say(f"`{name}` is not set here — `secret` lists what is.")
            return
        del here[name]
        _write_local(here)
        say(f"removed `{name}`.")
        return

    name, value = head, tail.strip()
    if not NAME.match(name):
        say(f"`{name}` is not an environment variable name — "
            f"capitals, digits and underscores, e.g. `FRONT_API_TOKEN`.")
        return
    if not value:
        say(f"`secret {name} <value>` — the value has to come with it.")
        return

    # Membership of a private channel is the access control everywhere else
    # here; a public channel would be handing the token to the workspace.
    private = is_private(channel_id)
    if private is False:
        say(
            "not in a public channel — that would put the token in front of "
            "everyone in the workspace. Send it in a private channel or a DM."
        )
        return

    replacing = name in here
    here[name] = value
    _write_local(here)
    say(
        f"{'replaced' if replacing else 'stored'} `{name}` ({mask(value)}) for this "
        f"instance — the next run will have it, nothing needs restarting.\n"
        f"Slack keeps your message and I cannot delete it: remove it yourself, and "
        f"treat this as a credential that can be rotated."
    )
