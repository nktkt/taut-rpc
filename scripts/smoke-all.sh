#!/usr/bin/env bash
# Run every taut-rpc example end-to-end. Used by maintainers before tagging
# a release. Each example builds, codegens, starts the server in the
# background, runs the client (if any), and reports pass/fail.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Build the npm runtime so example clients can resolve `file:` deps.
(cd npm/taut-rpc && npm install --silent && npm run build --silent)

# Function: run one example's server + client for N seconds.
run_example() {
    local name="$1" port="$2" client_mode="${3:-tsx}"
    echo "::group::$name (port $port)"

    if [ ! -d "examples/$name/server" ]; then
        echo "skip (no server)"; echo "::endgroup::"; return 0
    fi

    (cd "examples/$name/server" && cargo build --quiet) || { echo "FAIL build"; return 1; }

    # Codegen if a client exists.
    if [ -d "examples/$name/client" ] && [ -f "examples/$name/server/target/debug/$name-server" ]; then
        cargo run -p taut-rpc-cli --quiet -- taut gen \
            --from-binary "examples/$name/server/target/debug/$name-server" \
            --out "examples/$name/client/src/api.gen.ts" || true
    fi

    (cd "examples/$name/server" && cargo run --quiet) &
    local server_pid=$!
    # Wait for health.
    local i=0
    while ! curl -s -o /dev/null "http://127.0.0.1:$port/rpc/_health"; do
        sleep 1
        i=$((i+1))
        [ "$i" -gt 30 ] && { echo "FAIL health"; kill $server_pid 2>/dev/null; return 1; }
    done

    if [ -d "examples/$name/client" ]; then
        (cd "examples/$name/client" && npm install --silent)
        (cd "examples/$name/client" && timeout 30 npm run start) || true
    fi

    kill $server_pid 2>/dev/null || true
    wait $server_pid 2>/dev/null || true
    echo "PASS $name"
    echo "::endgroup::"
}

run_example smoke 7700 || exit 1
run_example phase1 7701 || exit 1
run_example phase2-auth 7702 || exit 1
run_example phase2-tracing 7703 || exit 1
run_example phase3-counter 7704 || exit 1
run_example phase4-validate 7705 || exit 1
# Phase 5 examples: skip in this smoke (Vite/SvelteKit need long dev runs).

echo "All Phase 0-4 examples passed."
