---
id: "websearch"
name: "Web Search"
description: "Search the web for current information, news, and answers to questions"
category: "system"
version: "1.1.0"
supports_background: false
input_schema:
  type: object
  properties:
    query:
      type: string
      description: "The search query"
    max_results:
      type: number
      description: "Maximum number of results to return (default: 5)"
    allowed_domains:
      type: array
      items:
        type: string
      description: "Only include results from these domains (e.g. [\"github.com\", \"stackoverflow.com\"])"
    blocked_domains:
      type: array
      items:
        type: string
      description: "Exclude results from these domains (e.g. [\"pinterest.com\", \"quora.com\"])"
  required:
    - query
is_read_only: true
is_concurrency_safe: true
---

# Web Search Tool

Search the internet for current information, news, facts, and answers.

## When to Use

Use this tool when you need:
- **Current information** - Breaking news, recent events, latest data
- **Fact checking** - Verify information or find authoritative sources
- **Research** - Gather information on topics outside your knowledge
- **Real-time data** - Stock prices, weather, sports scores
- **Recent developments** - Technology updates, product releases

## When NOT to Use

Don't use for:
- General knowledge you already have
- Simple calculations or programming tasks
- Personal data (user's files, calendar, etc.)
- Information the user already provided

## CRITICAL REQUIREMENT - You MUST follow this:
- After answering the user's question, you **MUST** include a "Sources:" section at the end of your response
- In the Sources section, list all relevant URLs from the search results as markdown hyperlinks: [Title](URL)
- This is **MANDATORY** - never skip including sources in your response
- Example format:

  [Your answer here]

  Sources:
  - [Source Title 1](https://example.com/1)
  - [Source Title 2](https://example.com/2)

## Parameters

- `query` (string, required): The search query in natural language
- `max_results` (number, optional): Maximum results to return (default: 5, max: 10)
- `allowed_domains` (array of strings, optional): Only include results from these domains. Cannot be used together with `blocked_domains`.
- `blocked_domains` (array of strings, optional): Exclude results from these domains. Cannot be used together with `allowed_domains`.

## Examples

**Current events:**
```
query: "latest AI breakthroughs 2026"
```

**Fact checking:**
```
query: "when was the Eiffel Tower built"
```

**Technical information:**
```
query: "Rust async await best practices"
```

**Real-time data:**
```
query: "Apple stock price today"
```

**Domain-filtered search:**
```json
{
  "query": "Rust async await patterns",
  "allowed_domains": ["doc.rust-lang.org", "github.com"]
}
```

**Blocking low-quality sites:**
```json
{
  "query": "best laptop 2026",
  "blocked_domains": ["pinterest.com", "quora.com"]
}
```

## Response Format

Returns search results with:
- Title
- URL
- Snippet/description
- Relevance score (if available)

## Domain Filtering

- `allowed_domains` and `blocked_domains` are **mutually exclusive**. If both are specified, `allowed_domains` takes priority.
- Domain matching checks the host part of each result URL (e.g. `"github.com"` matches `https://github.com/user/repo`).
- Filtering is performed client-side after receiving search results.

## Implementation

This tool proxies search requests through Happy Server, which holds the search API key. No local configuration is needed.
