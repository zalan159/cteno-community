# Open Source Checklist

This is the current hardening checklist before splitting `multi-agent-runtime` into its own public repository.

## Done

- Claude Agent SDK adapter implemented
- Rust workspace scaffold implemented
- Unified workspace / role / dispatch event model implemented
- Three built-in templates implemented:
  - `Coding Studio`
  - `OPC Solo Company`
  - `Autoresearch`
- Live Claude e2e coverage added for all three templates
- Public-facing README expanded with setup, usage, templates, API, and test guidance
- Contributor guide added

## Still Needed Before First Public Release

### Packaging

- Choose and add an explicit OSS license
- Decide final package visibility:
  - keep internal scope
  - or publish under a public scope
- Fill in final npm metadata:
  - repository
  - homepage
  - bugs URL
  - keywords

### CI / Automation

- Add CI for `typecheck` and `build`
- Decide how live Claude e2e should run in CI:
  - only in protected environments with auth
  - or as manually triggered workflow
- Add a non-auth CI fallback if we want contributor PRs to get some signal without Claude access

### Runtime Hardening

- Replace FIFO dispatch correlation with stronger task correlation
- Decide how `workspace.initialized` should behave long-term when `sessionId` arrives late
- Add better support for multiple concurrent dispatches in the same workspace
- Decide whether `dispatch.result` should carry richer structured metadata than plain `resultText`

### Template Productization

- Define the acceptance bar for a built-in template
- Decide naming and versioning strategy for built-in templates
- Add at least one example of a custom user-authored template

### Repository Hygiene

- Add release notes / changelog policy
- Add issue templates if this becomes a standalone public repo
- Add a security policy if we expect external usage at meaningful scale

## Release Recommendation

Current state is strong enough for:
- internal incubation
- design partner sharing
- early source-available extraction

Current state is not yet ideal for:
- broad npm publication without caveats
- outside contributors relying on stable protocol guarantees

## Suggested Next Moves

1. Add license and repo metadata
2. Decide CI strategy for live Claude e2e
3. Harden task correlation beyond FIFO
4. Then split into its own repository
