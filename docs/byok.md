# BYOK (Bring Your Own Key)

Cteno Community runs on your own LLM API keys. Configure via `~/.cteno/profiles.json` or in-app settings.

## Supported providers

- Anthropic (Claude models)
- OpenAI (GPT models)
- DeepSeek (deepseek-chat, deepseek-reasoner)
- Google Gemini
- Local Ollama / LM Studio (OpenAI-compatible endpoints)

## profiles.json schema

```json
{
  "anthropic": { "api_key": "sk-ant-...", "base_url": "https://api.anthropic.com" },
  "openai":    { "api_key": "sk-...",     "base_url": "https://api.openai.com/v1" },
  "deepseek":  { "api_key": "sk-...",     "base_url": "https://api.deepseek.com/v1" },
  "gemini":    { "api_key": "...",        "base_url": "https://generativelanguage.googleapis.com" }
}
```

## Per-session model override

In session creation: `model: "anthropic/claude-3-5-sonnet"` / `openai/gpt-4o` / `deepseek/deepseek-chat` 等。

## Custom OpenAI-compatible endpoint (Ollama / LM Studio / others)

```json
{ "openai": { "api_key": "ollama", "base_url": "http://localhost:11434/v1" } }
```

## Coding Plan presets

The in-app BYOK profile page also supports Coding Plan presets. A preset asks for one
Coding Plan SK and creates a group of regular local profiles, then sets the recommended
model as the default Cteno/BYOK profile.

Current presets:

- GLM Coding Plan: `GLM-5.1`, `GLM-5-Turbo`, `GLM-4.7`, `GLM-4.5-Air`
- Kimi Code: `kimi-for-coding`
- MiniMax Token Plan: `MiniMax-M2.7`, `MiniMax-M2.7-highspeed`, `MiniMax-M2.5`
- Bailian Coding Plan: `qwen3.5-plus`, `kimi-k2.5`, `glm-5`, `MiniMax-M2.5`,
  `qwen3-max-2026-01-23`, `qwen3-coder-next`, `qwen3-coder-plus`, `glm-4.7`

These profiles use Anthropic-compatible Coding Plan endpoints such as
`https://api.z.ai/api/anthropic`, `https://api.kimi.com/coding`,
`https://api.minimax.io/anthropic`, and
`https://coding-intl.dashscope.aliyuncs.com/apps/anthropic`. Do not replace them
with the normal provider chat endpoints unless the provider explicitly says that
endpoint is valid for Coding Plan keys.
