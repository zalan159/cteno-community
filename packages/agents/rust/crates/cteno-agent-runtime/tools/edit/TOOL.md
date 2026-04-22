---
id: "edit"
name: "File Edit"
description: "Search and replace text in files with flexible matching strategies and LLM auto-correction"
category: "system"
version: "2.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    path:
      type: string
      description: "File path to edit (supports ~ expansion)"
    workdir:
      type: string
      description: "Working directory used to resolve relative path (default: ~)"
    instruction:
      type: string
      description: |
        A clear, semantic instruction for the code change. Must explain:
        1. WHY the change is needed
        2. WHERE the change should happen
        3. WHAT is the high-level change
        4. WHAT is the desired outcome

        Example: "In the 'calculateTotal' function, update the 'taxRate' constant from 0.05 to 0.075 to reflect new regional tax laws."
    old_string:
      type: string
      description: "The exact text to find and replace (include context lines for uniqueness)"
    new_string:
      type: string
      description: "The replacement text"
    expected_replacements:
      type: number
      description: "Expected number of replacements (default: 1)"
  required:
    - path
    - instruction
    - old_string
    - new_string
is_read_only: false
is_concurrency_safe: false
---

# File Edit Tool

**CRITICAL: Always read the file first before editing.**

Use the read tool to examine content before calling edit.

## Usage Requirements

**Required Parameters:**

1. `instruction` - Detailed explanation of WHY/WHERE/WHAT/OUTCOME
   - ✅ Good: "In the 'renderUserProfile' function, add a null check for the 'user' object to prevent crashes when user data is loading"
   - ❌ Bad: "Add null check" (too vague)
   - ❌ Bad: "Change the code" (no context)

2. `old_string` - EXACT literal text (NEVER escape)
   - Must include at least 3 lines of context BEFORE and AFTER
   - Must match whitespace and indentation precisely
   - ❌ Do NOT escape: Use literal text, not "\\n" or "\""

3. `new_string` - EXACT literal text (NEVER escape)
   - Ensure the resulting code is correct and idiomatic
   - Must be different from old_string

**Optional Parameters:**
- `expected_replacements` (number, default: 1): Expected number of matches

## Matching Strategies (tried in order)

1. **Exact Match**: Literal string match
2. **Flexible Line Match**: Ignores leading whitespace differences per line
3. **Regex Flexible Match**: Tokenizes by delimiters, tolerates whitespace
4. **LLM Auto-Correction**: If all strategies fail, LLM attempts to fix the search string

## Best Practices

- Prefer breaking complex changes into multiple atomic edits
- Always verify with read tool after editing
- If edit fails, check current file content with read tool

## Examples

```javascript
// Simple replacement
edit({
  path: "~/project/config.json",
  instruction: "Update version number from 1.0.0 to 1.1.0 in package config",
  old_string: '"version": "1.0.0"',
  new_string: '"version": "1.1.0"'
})

// Multi-line replacement with context
edit({
  path: "~/project/main.rs",
  instruction: "In main function, update greeting message to include 'World'",
  old_string: `fn main() {
    println!("Hello");
}`,
  new_string: `fn main() {
    println!("Hello, World!");
}`
})
```

## Error Handling

| Error | Cause | Solution |
|-------|-------|----------|
| "0 occurrences found" | old_string not in file | Use read tool to verify content |
| "Multiple occurrences" | old_string not unique | Add more context lines |
| "No changes to apply" | old_string == new_string | Check your parameters |
| "LLM correction failed" | Auto-fix couldn't resolve | Check file with read tool |
