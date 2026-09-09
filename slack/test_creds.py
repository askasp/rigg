#!/usr/bin/env python3
"""Checks for credentials set from Slack.

    python3 slack/test_creds.py

The things worth asserting are the ones that are quiet when wrong: file modes,
that a value never comes back in a reply, and that the Ansible-managed file is
only ever read.
"""
import os
import stat
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
TMP = tempfile.mkdtemp(prefix="rigg-creds-test-")
os.environ["RIGG_HOME"] = TMP
os.environ["RIGG_INSTANCE"] = "test"

import creds  # noqa: E402

ok = 0


def check(label, cond, detail=""):
    global ok
    if not cond:
        print(f"FAIL  {label} {detail}", file=sys.stderr)
        sys.exit(1)
    ok += 1
    print(f"ok    {label}")


said = []


def run(rest, private=True):
    said.clear()
    creds.command("/repo", "rigg-tasks", rest, said.append, "C1",
                  is_private=lambda cid: private)
    return said[0] if said else ""


out = run("")
check("nothing set says how to set one", "secret <NAME> <value>" in out, out)

out = run("FRONT_API_TOKEN fr_live_abcdefgh1234")
check("a credential is stored", "stored" in out, out)
check("...masked, never echoed", "fr_live_abcdefgh1234" not in out and "…1234" in out, out)
check("...and says Slack still has the message", "rotate" in out, out)
check("...and says nothing needs restarting", "restarting" in out, out)
check("...and it is readable back", creds.local()["FRONT_API_TOKEN"] == "fr_live_abcdefgh1234")

p = creds.local_path()
check("the file is 0600", stat.S_IMODE(p.stat().st_mode) == 0o600,
      oct(stat.S_IMODE(p.stat().st_mode)))
check("the directory is 0700", stat.S_IMODE(p.parent.stat().st_mode) == 0o700,
      oct(stat.S_IMODE(p.parent.stat().st_mode)))

out = run("FRONT_API_TOKEN other-value-entirely")
check("setting it again replaces", "replaced" in out, out)
check("...and the new one is what is stored",
      creds.local()["FRONT_API_TOKEN"] == "other-value-entirely")

# --- refusals ---------------------------------------------------------------
out = run("front_api_token x")
check("a lowercase name is refused", "environment variable name" in out, out)
out = run("FRONT_API_TOKEN")
check("a name with no value is refused", "has to come with it" in out, out)
out = run("SOME_TOKEN abc123def456", private=False)
check("a public channel is refused", "public channel" in out, out)
check("...and nothing was written", "SOME_TOKEN" not in creds.local())

# Slack not answering is not the same as answering "public".
out = run("MAYBE_TOKEN abc123def456", private=None)
check("an unknown channel is allowed rather than blocked", "stored" in out, out)
run("rm MAYBE_TOKEN")

# --- the Ansible-managed file is read, never written ------------------------
creds.managed_path().write_text("# from the vault\nVAULT_ONLY=v-secret-value\n"
                                "FRONT_API_TOKEN=from-the-vault\n")
check("the vault file is read", creds.managed()["VAULT_ONLY"] == "v-secret-value")
check("a local value wins over the vault",
      creds.current()["FRONT_API_TOKEN"] == "other-value-entirely")
check("and the vault still supplies what is only there",
      creds.current()["VAULT_ONLY"] == "v-secret-value")

# A name set here that the vault has never heard of — the listing must not
# reach into the vault for a default it does not need.
run("LOCAL_ONLY_TOKEN abcdefgh1234")
out = run("")
check("a name set only here lists fine", "LOCAL_ONLY_TOKEN" in out, out)
check("...and does not leak its value", "abcdefgh1234" not in out, out)
run("rm LOCAL_ONLY_TOKEN")

out = run("")
check("the listing names both sources", "VAULT_ONLY" in out and "FRONT_API_TOKEN" in out, out)
check("...and says which is which", "from the vault" in out and "overriding" in out, out)
check("...and leaks no values",
      "v-secret-value" not in out and "other-value-entirely" not in out, out)

out = run("rm VAULT_ONLY")
check("removing a vault-managed name explains where it lives", "Ansible" in out, out)
check("...and does not touch the vault file", "VAULT_ONLY" in creds.managed())

out = run("rm FRONT_API_TOKEN")
check("removing a locally set one works", "removed" in out, out)
check("...and the vault value shows through again",
      creds.current()["FRONT_API_TOKEN"] == "from-the-vault")

out = run("rm NOT_A_THING")
check("removing what is not set says so", "not set here" in out, out)

print(f"\n{ok} checks passed")
