#!/usr/bin/env bash
# The corpus checks, with the dependencies they need.
set -euo pipefail
cd "$(dirname "$0")"
exec uv run --quiet --with "requests~=2.34" --with "mcp~=2.2" \
  --with "mail-parser-reply~=1.36" test_mail.py "$@"
