---
id: "browser_adapter"
name: "Browser Adapter"
description: "Run or manage site-specific browser automation adapters — pre-built scripts for common websites like GitHub, Twitter, Reddit, YouTube, etc."
category: "system"
version: "1.0.0"
supports_background: false
should_defer: true
search_hint: "browser automation adapter scripts websites"
input_schema:
  type: object
  properties:
    action:
      type: string
      description: "Action to perform: run, list, show, create, delete, search, install"
      enum: ["run", "list", "create", "delete", "show", "search", "install"]
    adapter_name:
      type: string
      description: "Adapter name (e.g. github/repo, bilibili/search). Required for run/show/delete/install. Used as search query for search action."
    args:
      type: object
      description: "Arguments to pass to the adapter (for run action)"
    adapter_json:
      type: string
      description: "JSON string of a SiteAdapter definition (for create action)"
  required: ["action"]
is_read_only: false
is_concurrency_safe: false
---

# Browser Adapter

Run pre-built site-specific browser automation scripts. Each adapter targets a specific website and extracts structured data using the browser's authenticated session.

## Actions

### list
List all available adapters with their name, domain, and description.

### show
Show full details of an adapter including its arguments and script.
- `adapter_name`: Required. e.g. "github/repo"

### run
Execute an adapter script in the browser context.
- `adapter_name`: Required. The adapter to run.
- `args`: Arguments object matching the adapter's arg spec.

The adapter runs as JavaScript in the browser tab matching the adapter's domain. If no matching tab exists, one will be created. The browser session must already exist (call browser_navigate first).

### create
Create a new adapter from a JSON definition.
- `adapter_json`: Required. JSON string of a SiteAdapter object with name, domain, description, args, script, read_only fields.

### delete
Delete an adapter.
- `adapter_name`: Required. The adapter to delete.

### search
Search the bb-sites repository (epiral/bb-sites on GitHub) for available adapters. Supports ~48 sites including bilibili, zhihu, douban, xiaohongshu, weibo, arxiv, and more.
- `adapter_name`: Optional. Search query to filter by site name. Leave empty to list all sites.

### install
Download and install an adapter from bb-sites into the local adapters directory.
- `adapter_name`: Required. The adapter to install, e.g. "bilibili/search", "zhihu/hot".

## Adapter Format

Adapters are JSON files stored in `{workdir}/.cteno/adapters/`:
```json
{
  "name": "github/repo",
  "domain": "github.com",
  "description": "Get GitHub repository info",
  "args": [{"name": "repo", "description": "owner/repo format", "required": true}],
  "script": "const resp = await fetch('https://api.github.com/repos/' + args.repo); ...",
  "read_only": true
}
```

## Notes

- Adapters leverage the browser's existing login cookies — no separate API keys needed
- The script executes in the page context with full access to cookies and DOM
- A set of default adapters for popular sites is installed on first use
