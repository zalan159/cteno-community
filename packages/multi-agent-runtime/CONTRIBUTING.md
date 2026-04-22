# Contributing

Thanks for considering a contribution.

## Setup

```bash
cd /Users/zal/Cteno/packages/multi-agent-runtime
npm install
npm run typecheck
npm run build
```

## Test Strategy

This package has two layers of verification:

1. `smoke:*`
These are fast manual runs for inspecting behavior and generated output.

2. `e2e:*`
These are the release-quality checks.
They make real Claude calls and validate:
- dispatch lifecycle
- delegated role correctness
- generated file existence
- template-specific content expectations

Run the full suite with:

```bash
npm test
```

## Working Norms

- Keep the runtime provider-neutral where possible.
- Put Claude-specific logic behind the adapter layer.
- Prefer strengthening protocol objects and events over hardcoding template behavior.
- When adding a new built-in template, also add a live e2e that validates its artifact quality.
- Avoid weakening assertions just to make tests pass. If an assertion is too brittle, widen it to semantic equivalence.

## Pull Request Checklist

- `npm run typecheck`
- `npm run build`
- `npm test`
- Update [README.md](/Users/zal/Cteno/packages/multi-agent-runtime/README.md) if public behavior changed
- Update or add e2e coverage when adding templates, events, or adapter behavior
