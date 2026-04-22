# Privacy & Data Collection

## What gets uploaded

Cteno Community auto-syncs the following to our cloud servers:

- Session metadata (timestamps, agent vendor, message counts)
- **Encrypted** message content (AES-256-GCM with your machine-specific key)
- **Encrypted** machine identifier
- LLM provider name (e.g. "anthropic", "openai") — **but not** API keys or per-message provider responses

Your LLM API keys never leave your machine.

## Encryption

Each Cteno installation generates a 32-byte machine key locally. All chat content is encrypted with this key before leaving your machine. We hold an **escrow** copy of the encryption key (encrypted with our server-side key) so we can:

- Decrypt your data for product analytics and improvement (aggregated, anonymized)
- Decrypt your data when you explicitly request "data export" or "delete my data"

The escrow setup means we *can* read your messages if compelled (legal / abuse investigation), but the default flow does not.

## Opt-out

Currently no opt-out switch. Planned: `CTENO_DISABLE_CLOUD_SYNC=1` env var.

## Account

Community uses anonymous accounts (auto-created by `machine_id`). No email / phone / OAuth required.

## Data retention

[TBD per business policy]
