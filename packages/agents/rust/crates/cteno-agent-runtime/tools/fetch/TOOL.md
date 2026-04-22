---
id: "fetch"
name: "Fetch Web Content"
description: "Fetch web pages and extract main content, with optional LLM-based compression. Use this tool when you need to read or analyze web page content."
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    url:
      type: string
      description: "The URL to fetch"
    prompt:
      type: string
      description: "User's question or intent to guide content extraction and compression"
    max_length:
      type: number
      description: "Maximum content length (chars) before triggering LLM compression (default: 10000)"
    raw:
      type: boolean
      description: "If true, return raw extracted content without LLM compression (default: false)"
  required:
    - url
    - prompt
is_read_only: true
is_concurrency_safe: true
---

# Fetch Web Content

Fetches web pages, extracts readable content using Mozilla Readability algorithm, and optionally compresses the result using an LLM.

## Parameters

- `url` (string, required): The URL to fetch
- `prompt` (string, required): User's question to guide content extraction
- `max_length` (integer, optional): Maximum content length before compression (default: 10000)
- `raw` (boolean, optional): Return raw extracted content without LLM compression (default: false)

## Behavior

1. Checks in-memory cache (15-minute TTL) — a cache hit returns immediately with "(cached)" appended
2. Fetches the HTML from the given URL (follows redirects by default)
3. If the final URL lands on a **different host** (cross-domain redirect), returns a REDIRECT DETECTED notice instead of page content. You should make a new fetch request with the redirect URL.
4. Extracts main content using Readability algorithm
5. Converts HTML to Markdown and stores in cache
6. If content length < max_length or raw=true: returns directly
7. Otherwise: uses the session's compress LLM endpoint to extract relevant information based on the prompt

## IMPORTANT

- **WebFetch WILL FAIL for authenticated or private URLs.** Before using this tool, check if the URL points to an authenticated service (e.g. Google Docs, Confluence, Jira, GitHub private repos). If so, use a specialized tool or ask the user for credentials.

## Examples

```json
{
  "url": "https://example.com/article",
  "prompt": "What are the main points about Rust async programming?"
}
```

```json
{
  "url": "https://docs.example.com/api",
  "prompt": "How to authenticate API requests?",
  "max_length": 5000,
  "raw": false
}
```

## Notes

- Uses the session's profile compress endpoint (configured in profiles.json)
- Respects HTTP errors and returns appropriate error messages
- User-Agent: Mozilla/5.0 (compatible; CtenoBot/1.0)
- Timeout: 30 seconds
- Responses are cached for 15 minutes (up to 50 URLs). Cached responses include a "(cached)" indicator.
- Cross-domain redirects are detected and reported instead of silently followed
