//! Accessibility Tree Parsing and Diffing
//!
//! Converts CDP Accessibility.getFullAXTree output into a compact text
//! representation with element indices. Supports diffing two trees to
//! show changes after an action.

use serde_json::Value;
use std::collections::HashMap;

/// A simplified AX node for display and diffing.
#[derive(Debug, Clone)]
pub struct AXNode {
    pub node_id: String,
    /// CDP backendDOMNodeId — the key for resolving to a real DOM node.
    pub backend_dom_node_id: Option<i64>,
    pub role: String,
    pub name: String,
    pub value: String,
    pub focused: bool,
    pub checked: Option<bool>,
    pub level: Option<i64>,
    pub parent_id: Option<String>,
    pub children_ids: Vec<String>,
    pub ignored: bool,
    /// Depth in the tree (for indentation).
    pub depth: usize,
}

impl AXNode {
    /// Whether this node is interactive (clickable, editable, etc.)
    pub fn is_interactive(&self) -> bool {
        matches!(
            self.role.as_str(),
            "textbox"
                | "button"
                | "link"
                | "combobox"
                | "checkbox"
                | "radio"
                | "menuitem"
                | "menuitemcheckbox"
                | "menuitemradio"
                | "tab"
                | "switch"
                | "slider"
                | "spinbutton"
                | "searchbox"
                | "option"
                | "listbox"
        )
    }

    /// Format as a display line: `[idx] role "name" extra_attrs`
    pub fn format_line(&self, index: usize) -> String {
        let indent = "  ".repeat(self.depth);
        let mut parts = vec![format!("{}[{}] {}", indent, index, self.role)];

        if !self.name.is_empty() {
            parts.push(format!("\"{}\"", truncate_str(&self.name, 80)));
        }
        if !self.value.is_empty() {
            parts.push(format!("value=\"{}\"", truncate_str(&self.value, 60)));
        }
        if self.focused {
            parts.push("focused".to_string());
        }
        if let Some(checked) = self.checked {
            parts.push(format!("checked={}", checked));
        }
        if let Some(level) = self.level {
            parts.push(format!("level={}", level));
        }

        parts.join(" ")
    }
}

/// Parse CDP AX Tree nodes into our simplified representation.
pub fn parse_ax_tree(cdp_nodes: &[Value]) -> Vec<AXNode> {
    let mut nodes = Vec::new();
    let mut parent_map: HashMap<String, String> = HashMap::new();
    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();

    // First pass: extract basic info
    for node_val in cdp_nodes {
        let node_id = node_val["nodeId"].as_str().unwrap_or("").to_string();
        let backend_dom_node_id = node_val.get("backendDOMNodeId").and_then(|v| v.as_i64());
        let ignored = node_val
            .get("ignored")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let role = node_val
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let name = extract_ax_property(node_val, "name");
        let value = extract_ax_property(node_val, "value");

        let focused = node_val
            .get("properties")
            .and_then(|p| p.as_array())
            .map(|props| {
                props.iter().any(|p| {
                    p.get("name").and_then(|n| n.as_str()) == Some("focused")
                        && p.get("value")
                            .and_then(|v| v.get("value"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false);

        let checked = node_val
            .get("properties")
            .and_then(|p| p.as_array())
            .and_then(|props| {
                props.iter().find_map(|p| {
                    if p.get("name").and_then(|n| n.as_str()) == Some("checked") {
                        p.get("value")
                            .and_then(|v| v.get("value"))
                            .and_then(|v| v.as_str())
                            .map(|s| s == "true")
                    } else {
                        None
                    }
                })
            });

        let level = node_val
            .get("properties")
            .and_then(|p| p.as_array())
            .and_then(|props| {
                props.iter().find_map(|p| {
                    if p.get("name").and_then(|n| n.as_str()) == Some("level") {
                        p.get("value")
                            .and_then(|v| v.get("value"))
                            .and_then(|v| v.as_i64())
                    } else {
                        None
                    }
                })
            });

        // Track parent-child relationships
        if let Some(child_ids) = node_val.get("childIds").and_then(|c| c.as_array()) {
            let children: Vec<String> = child_ids
                .iter()
                .filter_map(|c| c.as_str().map(|s| s.to_string()))
                .collect();
            for child_id in &children {
                parent_map.insert(child_id.clone(), node_id.clone());
            }
            children_map.insert(node_id.clone(), children);
        }

        nodes.push(AXNode {
            node_id,
            backend_dom_node_id,
            role,
            name,
            value,
            focused,
            checked,
            level,
            parent_id: None,
            children_ids: Vec::new(),
            ignored,
            depth: 0,
        });
    }

    // Second pass: fill in parent/children and compute depth
    let depth_map = compute_depths(&children_map, cdp_nodes, &parent_map);
    for node in &mut nodes {
        node.parent_id = parent_map.get(&node.node_id).cloned();
        node.children_ids = children_map.get(&node.node_id).cloned().unwrap_or_default();
        node.depth = *depth_map.get(&node.node_id).unwrap_or(&0);
    }

    nodes
}

/// Compute tree depth for each node.
/// Traverses from ALL root nodes (nodes without a parent), not just the first.
/// This ensures modal/portal subtrees (siblings of the main document root) are included.
fn compute_depths(
    children_map: &HashMap<String, Vec<String>>,
    cdp_nodes: &[Value],
    parent_map: &HashMap<String, String>,
) -> HashMap<String, usize> {
    let mut depths = HashMap::new();

    // Find all root nodes (nodes that have no parent).
    // In most pages: the document root + any portal/modal roots.
    let mut stack: Vec<(String, usize)> = Vec::new();
    for node_val in cdp_nodes {
        let node_id = node_val["nodeId"].as_str().unwrap_or("").to_string();
        if !node_id.is_empty() && !parent_map.contains_key(&node_id) {
            stack.push((node_id, 0));
        }
    }

    while let Some((id, depth)) = stack.pop() {
        depths.insert(id.clone(), depth);
        if let Some(children) = children_map.get(&id) {
            for child_id in children.iter().rev() {
                stack.push((child_id.clone(), depth + 1));
            }
        }
    }

    depths
}

/// Indexed node for display (owned data).
pub struct IndexedNode {
    pub index: usize,
    pub depth: usize,
    pub role: String,
    pub name: String,
    pub value: String,
    pub focused: bool,
    pub checked: Option<bool>,
    pub level: Option<i64>,
}

/// Result of building an indexed tree.
pub struct IndexedTreeResult {
    pub nodes: Vec<IndexedNode>,
    /// index → AX nodeId
    pub node_id_map: Vec<String>,
    /// index → backendDOMNodeId (for precise DOM resolution)
    pub backend_node_map: Vec<Option<i64>>,
}

/// Filter and index AX nodes for display.
///
/// `query` — if provided, only include nodes whose name, value, or role
/// contains this substring (case-insensitive). Useful for finding specific
/// elements without writing JS queries.
pub fn build_indexed_tree(
    nodes: &[AXNode],
    max_nodes: usize,
    interactive_only: bool,
    query: Option<&str>,
) -> IndexedTreeResult {
    let mut indexed = Vec::new();
    let mut node_id_map = Vec::new();
    let mut backend_node_map = Vec::new();

    let query_lower = query.map(|q| q.to_lowercase());

    for node in nodes {
        if indexed.len() >= max_nodes {
            break;
        }

        // Skip ignored and generic/none roles
        if node.ignored {
            continue;
        }
        if matches!(
            node.role.as_str(),
            "none" | "generic" | "InlineTextBox" | ""
        ) {
            if node.name.is_empty() {
                continue;
            }
        }

        if interactive_only && !node.is_interactive() {
            continue;
        }

        // Query filter: match name, value, or role (case-insensitive)
        if let Some(ref q) = query_lower {
            let name_lower = node.name.to_lowercase();
            let value_lower = node.value.to_lowercase();
            let role_lower = node.role.to_lowercase();
            if !name_lower.contains(q) && !value_lower.contains(q) && !role_lower.contains(q) {
                continue;
            }
        }

        let idx = indexed.len();
        indexed.push(IndexedNode {
            index: idx,
            depth: node.depth,
            role: node.role.clone(),
            name: node.name.clone(),
            value: node.value.clone(),
            focused: node.focused,
            checked: node.checked,
            level: node.level,
        });
        node_id_map.push(node.node_id.clone());
        backend_node_map.push(node.backend_dom_node_id);
    }

    IndexedTreeResult {
        nodes: indexed,
        node_id_map,
        backend_node_map,
    }
}

/// Render indexed tree to text.
pub fn render_tree(indexed: &[IndexedNode]) -> String {
    let mut lines = Vec::new();
    for node in indexed {
        let indent = "  ".repeat(node.depth);
        let mut parts = vec![format!("{}[{}] {}", indent, node.index, node.role)];

        if !node.name.is_empty() {
            parts.push(format!("\"{}\"", truncate_str(&node.name, 80)));
        }
        if !node.value.is_empty() {
            parts.push(format!("value=\"{}\"", truncate_str(&node.value, 60)));
        }
        if node.focused {
            parts.push("focused".to_string());
        }
        if let Some(checked) = node.checked {
            parts.push(format!("checked={}", checked));
        }
        if let Some(level) = node.level {
            parts.push(format!("level={}", level));
        }

        lines.push(parts.join(" "));
    }
    lines.join("\n")
}

/// Diff two AX tree snapshots and produce a human-readable change summary.
///
/// Uses `backendDOMNodeId` as the stable key (CDP `nodeId` changes between calls).
/// Falls back to structural fingerprint (role+name+depth) for nodes without backendDOMNodeId.
pub fn diff_ax_trees(before: &[AXNode], after: &[AXNode]) -> String {
    // Build maps keyed by backendDOMNodeId (stable across AX tree fetches).
    // Nodes without backendDOMNodeId are matched by structural fingerprint.
    let mut before_by_bn: HashMap<i64, &AXNode> = HashMap::new();
    let mut before_by_fp: HashMap<String, &AXNode> = HashMap::new();
    for n in before.iter().filter(|n| !n.ignored) {
        if let Some(bn_id) = n.backend_dom_node_id {
            before_by_bn.insert(bn_id, n);
        } else {
            before_by_fp.insert(structural_fingerprint(n), n);
        }
    }

    let mut after_by_bn: HashMap<i64, &AXNode> = HashMap::new();
    let mut after_by_fp: HashMap<String, &AXNode> = HashMap::new();
    for n in after.iter().filter(|n| !n.ignored) {
        if let Some(bn_id) = n.backend_dom_node_id {
            after_by_bn.insert(bn_id, n);
        } else {
            after_by_fp.insert(structural_fingerprint(n), n);
        }
    }

    let mut changes = Vec::new();

    // New nodes (in after but not in before)
    for (&bn_id, node) in &after_by_bn {
        if !before_by_bn.contains_key(&bn_id) && should_report_node(node) {
            changes.push(format_change('+', "new", node));
        }
    }
    for (fp, node) in &after_by_fp {
        if !before_by_fp.contains_key(fp) && should_report_node(node) {
            changes.push(format_change('+', "new", node));
        }
    }

    // Deleted nodes (in before but not in after)
    for (&bn_id, node) in &before_by_bn {
        if !after_by_bn.contains_key(&bn_id) && should_report_node(node) {
            changes.push(format_change('-', "del", node));
        }
    }
    for (fp, node) in &before_by_fp {
        if !after_by_fp.contains_key(fp) && should_report_node(node) {
            changes.push(format_change('-', "del", node));
        }
    }

    // Modified nodes (same backendDOMNodeId, different content)
    for (&bn_id, after_node) in &after_by_bn {
        if let Some(before_node) = before_by_bn.get(&bn_id) {
            let mods = detect_modifications(before_node, after_node);
            if !mods.is_empty() && should_report_node(after_node) {
                let indent = "  ".repeat(after_node.depth.min(4));
                let mut desc = format!("~ {}[mod] {}", indent, after_node.role);
                if !after_node.name.is_empty() {
                    desc.push_str(&format!(" \"{}\"", truncate_str(&after_node.name, 40)));
                }
                desc.push_str(&format!(" ({})", mods.join(", ")));
                changes.push(desc);
            }
        }
    }

    if changes.is_empty() {
        "No significant DOM changes.".to_string()
    } else if changes.len() > 50 {
        // Massive change = page navigation or full reload, summarize instead of listing
        let added = changes.iter().filter(|c| c.starts_with('+')).count();
        let removed = changes.iter().filter(|c| c.starts_with('-')).count();
        let modified = changes.iter().filter(|c| c.starts_with('~')).count();
        format!(
            "Page structure changed significantly ({} added, {} removed, {} modified). Use browser_state to see current page.",
            added, removed, modified
        )
    } else {
        if changes.len() > 15 {
            let total = changes.len();
            changes.truncate(15);
            changes.push(format!("... and {} more changes", total - 15));
        }
        changes.join("\n")
    }
}

/// Structural fingerprint for nodes without backendDOMNodeId.
fn structural_fingerprint(node: &AXNode) -> String {
    format!("{}:{}:{}:{}", node.role, node.depth, node.name, node.value)
}

/// Format a single change line.
fn format_change(sigil: char, tag: &str, node: &AXNode) -> String {
    let indent = "  ".repeat(node.depth.min(4));
    let mut desc = format!("{} {}[{}] {}", sigil, indent, tag, node.role);
    if !node.name.is_empty() {
        desc.push_str(&format!(" \"{}\"", truncate_str(&node.name, 60)));
    }
    if sigil == '+' && !node.value.is_empty() {
        desc.push_str(&format!(" value=\"{}\"", truncate_str(&node.value, 40)));
    }
    desc
}

/// Detect meaningful property changes between two matching nodes.
fn detect_modifications(before: &AXNode, after: &AXNode) -> Vec<String> {
    let mut mods = Vec::new();
    if before.name != after.name && !after.name.is_empty() {
        mods.push(format!(
            "name: \"{}\" → \"{}\"",
            truncate_str(&before.name, 30),
            truncate_str(&after.name, 30)
        ));
    }
    if before.value != after.value {
        mods.push(format!(
            "value: \"{}\" → \"{}\"",
            truncate_str(&before.value, 30),
            truncate_str(&after.value, 30)
        ));
    }
    if before.focused != after.focused && after.focused {
        mods.push("focused".to_string());
    }
    if before.checked != after.checked {
        if let Some(checked) = after.checked {
            mods.push(format!("checked={}", checked));
        }
    }
    mods
}

/// Whether a node is worth reporting in diffs (filter noise).
/// Only report interactive elements and meaningful content nodes.
fn should_report_node(node: &AXNode) -> bool {
    // Always skip structural noise
    if matches!(
        node.role.as_str(),
        "none"
            | "generic"
            | "InlineTextBox"
            | ""
            | "LineBreak"
            | "ignored"
            | "Abbr"
            | "Ruby"
            | "RubyAnnotation"
    ) {
        return false;
    }

    // Interactive elements are always worth reporting
    if node.is_interactive() {
        return true;
    }

    // High-signal semantic nodes — always report
    if matches!(
        node.role.as_str(),
        "heading"
            | "dialog"
            | "alert"
            | "alertdialog"
            | "status"
            | "tooltip"
            | "banner"
            | "form"
            | "table"
    ) {
        return true;
    }

    // image — only report if it has alt text (meaningful)
    if matches!(node.role.as_str(), "img" | "image") {
        return !node.name.is_empty();
    }

    // StaticText / paragraph / text — skip in diffs (too noisy, they flood the output)
    if matches!(node.role.as_str(), "StaticText" | "paragraph" | "text") {
        return false;
    }

    // Structural containers that just add noise
    if matches!(
        node.role.as_str(),
        "navigation" | "main" | "row" | "cell" | "contentinfo"
    ) {
        return false;
    }

    // Skip deep structural containers (group, list, section, etc.) that are just wrappers
    if matches!(
        node.role.as_str(),
        "group"
            | "list"
            | "listitem"
            | "section"
            | "article"
            | "region"
            | "complementary"
            | "contentinfo"
            | "directory"
    ) {
        return false;
    }

    // Default: report if it has a name (meaningful content)
    !node.name.is_empty()
}

/// Extract a named property from the AX node's "properties" or direct fields.
fn extract_ax_property(node: &Value, prop_name: &str) -> String {
    // Try direct field first (name, value are often top-level in CDP response)
    if let Some(val) = node.get(prop_name) {
        if let Some(v) = val.get("value").and_then(|v| v.as_str()) {
            return v.to_string();
        }
        if let Some(v) = val.as_str() {
            return v.to_string();
        }
    }

    // Try properties array
    if let Some(props) = node.get("properties").and_then(|p| p.as_array()) {
        for p in props {
            if p.get("name").and_then(|n| n.as_str()) == Some(prop_name) {
                if let Some(v) = p.get("value").and_then(|v| v.get("value")) {
                    if let Some(s) = v.as_str() {
                        return s.to_string();
                    }
                    if let Some(n) = v.as_f64() {
                        return n.to_string();
                    }
                }
            }
        }
    }

    String::new()
}

/// Truncate a string to max_len characters, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len - 3).collect();
        format!("{}...", truncated)
    }
}
