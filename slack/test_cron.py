#!/usr/bin/env python3
"""Checks for the cron verb, which now only translates.

    uv run --with "slack-bolt~=1.30" python slack/test_cron.py

What can go wrong is no longer scheduling - rigg owns that, and its own tests
cover it. What can go wrong here is the translation: a job made in one channel
reporting to another, a listing showing somebody else's jobs, or a token
reaching Slack in the output of a command that quoted a log.
"""

import os
import pathlib
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

os.environ["RIGG_HOME"] = tempfile.mkdtemp(prefix="rigg-cron-test-")
os.environ["RIGG_INSTANCE"] = "test"

import cron  # noqa: E402

ok = 0


def check(label, cond, detail=""):
    global ok
    if not cond:
        print(f"FAIL  {label} {detail}", file=sys.stderr)
        sys.exit(1)
    ok += 1
    print(f"ok    {label}")


class Rigg:
    """Stands in for the binary, remembering what it was asked to run."""

    def __init__(self, out="", code=0):
        self.calls = []
        self.out, self.code = out, code

    def __call__(self, repo, args, timeout=120):
        self.calls.append(list(args))
        return self.code, self.out


def run(rest, rigg, prefix="julies-rigg"):
    said = []
    cron.command(
        "/tmp/repo", prefix, rest, said.append, "C123",
        pipelines=["mail-drafts"], run_rigg=rigg,
        split_modifiers=lambda t: (t, {}), features={},
    )
    return said


# --- the translation --------------------------------------------------------

r = Rigg(out="nothing scheduled for #julies-rigg")
run("", r)
check("a bare `cron` lists this channel's jobs only",
      r.calls == [["cron", "list", "--channel", "#julies-rigg"]], str(r.calls))

r = Rigg()
run("list", r)
check("so does an explicit `list`",
      r.calls == [["cron", "list", "--channel", "#julies-rigg"]], str(r.calls))

r = Rigg()
run('add "0 7 * * *" --pipeline mail-drafts', r)
check("a job made here reports here",
      r.calls[0] == ["cron", "add", "0 7 * * *", "--pipeline", "mail-drafts",
                     "--channel", "#julies-rigg"], str(r.calls))

r = Rigg()
run('add "0 7 * * *" --pipeline mail-drafts --channel "#somewhere-else"', r)
check("an explicit --channel is left alone",
      r.calls[0].count("--channel") == 1
      and "#somewhere-else" in r.calls[0], str(r.calls))

r = Rigg()
run("show morning-mail", r)
check("every other verb passes straight through",
      r.calls == [["cron", "show", "morning-mail"]], str(r.calls))

r = Rigg()
run('add "0 7 * * *" --task "draft replies to anything unanswered"', r)
check("a quoted task survives as one argument",
      "draft replies to anything unanswered" in r.calls[0], str(r.calls))

said = run("run morning-mail", Rigg(out="ok"))
check("a run says something before it starts, not only when it ends",
      len(said) == 2 and "morning-mail" in said[0] and "minute" in said[0], str(said))

said = run("list", Rigg(out="a job"))
check("a listing is instant, so it gets no such preamble",
      len(said) == 1, str(said))

r = Rigg()
run("export adhoc", r)
check("export passes through too", r.calls == [["cron", "export", "adhoc"]], str(r.calls))

# --- what comes back --------------------------------------------------------

said = run("list", Rigg(out="  morning-mail  0 7 * * *"))
check("output is posted", "morning-mail" in said[0], str(said))

said = run("rm nope", Rigg(out="", code=1))
check("a failure with no output still says something",
      "failed" in said[0], str(said))

said = run("list", Rigg(out="x" * 5000))
check("a huge listing is truncated rather than dropped",
      len(said[0]) < 3600 and said[0].endswith("…"), str(len(said[0])))

# --- secrets must not reach the channel -------------------------------------

secret = "xoxb-9999-very-secret-value"
# redact only masks values it knows are secret, so put one in the store first.
d = pathlib.Path(os.environ["RIGG_HOME"]) / "instances" / "test"
d.mkdir(parents=True, exist_ok=True)
(d / "secrets.local.env").write_text(f"SLACK_BOT_TOKEN={secret}\n")

said = run("show job", Rigg(out=f"log said Authorization: Bearer {secret} oops"))
check("a token quoted in a job's output is redacted",
      secret not in said[0], said[0])

check("a short value is not redacted into meaninglessness",
      cron.redact("abc", {"K": "ab"}) == "abc")

# --- the markers an unattended run talks back with --------------------------

check("a report is what follows the marker",
      cron.report("noise\nRIGG-REPORT\nthe answer") == "the answer")
check("no marker means no report", cron.report("just noise") is None)
check("what a run learned is picked up",
      cron.learned("RIGG-LEARNED: the API caps at 50/min") ==
      ["the API caps at 50/min"])
check("a request for a secret names it",
      cron.secret_requests("RIGG-NEEDS-SECRET: API_KEY | to call X | console")
      == [("API_KEY", "to call X", "console")])

print(f"\n{ok} checks passed")
