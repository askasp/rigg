#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
exec uv run --quiet --with "mcp~=2.2" test_outbox.py "$@"
