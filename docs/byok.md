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
