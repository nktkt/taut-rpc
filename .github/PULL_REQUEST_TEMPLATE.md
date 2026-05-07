## Summary
<!-- 1-3 sentences. Reference any issue this fixes. -->

## SPEC / Roadmap link
<!-- e.g. SPEC §7, ROADMAP Phase 5. If your PR adds a new feature
that the SPEC doesn't cover yet, draft a SPEC change first. -->

## Changes
<!-- Bulleted list of what this PR does. -->

## Test plan
- [ ] `cargo test --workspace --all-targets` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] Phase 0–4 examples still build (smoke check)
- [ ] Updated documentation (concepts / guides / tutorials)
- [ ] Updated CHANGELOG.md under `[Unreleased]`
- [ ] If changing IR shape: bumped `IR_VERSION` + added SPEC §9.1 row + migration note

## Breaking changes
<!-- Y/N. If Y, describe what users need to do. -->

## Wire/IR impact
<!-- Y/N for each. If wire format or IR shape changed, link the SPEC §9.1 row. -->

🤖 Generated with [Claude Code](https://claude.com/claude-code)
