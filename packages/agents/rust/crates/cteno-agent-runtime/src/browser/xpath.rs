//! XPath Builder — generates XPath mappings from CDP DOM tree.
//!
//! Walks the DOM tree returned by `DOM.getDocument` and computes an XPath
//! string for every node. The resulting map is keyed by `backendNodeId`
//! (stable across AX‑tree refreshes) so it can be merged with accessibility
//! data to give each element a relocatable address.

use super::cdp::CdpConnection;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Depth attempts for `DOM.getDocument` — fall back to shallower depths
/// when Chrome returns a CBOR stack‑limit error on very large pages.
const DOM_DEPTH_ATTEMPTS: &[i64] = &[-1, 256, 128, 64, 32, 16, 8];

/// Build a map of `backendNodeId → XPath` for the current page.
///
/// The function calls `DOM.getDocument` to obtain the full (or partial) DOM
/// tree, then walks it iteratively to produce canonical XPath strings such as
/// `/html[1]/body[1]/div[2]/a[1]`.
pub async fn build_xpath_map(
    cdp: &CdpConnection,
    session_id: Option<&str>,
) -> Result<HashMap<i64, String>, String> {
    let root_node = get_dom_tree(cdp, session_id).await?;
    let mut map = HashMap::new();
    walk_dom_tree(&root_node, "", &mut map);
    Ok(map)
}

/// Try `DOM.getDocument` with progressively shallower depths.
async fn get_dom_tree(cdp: &CdpConnection, session_id: Option<&str>) -> Result<Value, String> {
    let mut last_err = String::new();

    for &depth in DOM_DEPTH_ATTEMPTS {
        match cdp
            .send_with_timeout(
                "DOM.getDocument",
                json!({ "depth": depth, "pierce": true }),
                session_id,
                15,
            )
            .await
        {
            Ok(result) => {
                if let Some(root) = result.get("root") {
                    return Ok(root.clone());
                }
                return Err("DOM.getDocument returned no root node".to_string());
            }
            Err(e) => {
                let msg = e.message.clone();
                if msg.contains("CBOR") || msg.contains("stack") {
                    log::warn!(
                        "[XPath] DOM.getDocument depth={} failed ({}), trying shallower",
                        depth,
                        msg
                    );
                    last_err = msg;
                    continue;
                }
                return Err(format!("DOM.getDocument failed: {}", msg));
            }
        }
    }

    Err(format!(
        "DOM.getDocument failed at all depths: {}",
        last_err
    ))
}

/// Iteratively walk a CDP DOM node tree and populate the XPath map.
///
/// Each entry maps `backendNodeId` → absolute XPath such as
/// `/html[1]/body[1]/div[3]`.
fn walk_dom_tree(root: &Value, parent_xpath: &str, map: &mut HashMap<i64, String>) {
    // Stack entries store (node_ref, this_node_full_xpath).
    // Children's xpaths are computed by the parent and pushed with the child,
    // so we must NOT recompute the xpath when popping — just use it directly.

    // Compute the initial xpath for the root node.
    let root_xpath = compute_root_xpath(root, parent_xpath);

    let mut stack: Vec<(&Value, String)> = vec![(root, root_xpath)];

    while let Some((node, self_xpath)) = stack.pop() {
        let node_type = node["nodeType"].as_i64().unwrap_or(0);
        let backend_id = node["backendNodeId"].as_i64();

        // Store mapping for element nodes.
        if node_type == 1 {
            if let Some(bid) = backend_id {
                map.insert(bid, self_xpath.clone());
            }
        }

        // Collect children and compute positional XPath for each.
        if let Some(children) = node["children"].as_array() {
            // Count same‑name siblings so we can produce `tag[n]`.
            let mut name_count: HashMap<String, usize> = HashMap::new();
            let mut child_entries: Vec<(usize, String)> = Vec::with_capacity(children.len());

            for (idx, child) in children.iter().enumerate() {
                let child_type = child["nodeType"].as_i64().unwrap_or(0);
                if child_type == 1 {
                    let local_name = child["localName"]
                        .as_str()
                        .or_else(|| child["nodeName"].as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if local_name.is_empty() {
                        child_entries.push((idx, self_xpath.clone()));
                    } else {
                        let pos = name_count.entry(local_name.clone()).or_insert(0);
                        *pos += 1;
                        child_entries.push((idx, format!("{self_xpath}/{local_name}[{pos}]")));
                    }
                } else {
                    // Non-element children (text, comment, doctype) inherit parent xpath
                    child_entries.push((idx, self_xpath.clone()));
                }
            }

            // Push children in reverse so we process them in document order.
            for (idx, xpath) in child_entries.into_iter().rev() {
                stack.push((&children[idx], xpath));
            }
        }

        // Handle shadow roots (pierce: true exposes them).
        if let Some(shadow_roots) = node["shadowRoots"].as_array() {
            for sr in shadow_roots.iter().rev() {
                stack.push((sr, self_xpath.clone()));
            }
        }

        // Handle content document (e.g. iframes).
        if let Some(content_doc) = node.get("contentDocument") {
            if content_doc.is_object() {
                stack.push((content_doc, self_xpath.clone()));
            }
        }
    }
}

/// Compute the xpath for the initial root node only.
/// Children have their xpaths computed by the parent loop above.
fn compute_root_xpath(root: &Value, parent_xpath: &str) -> String {
    let node_type = root["nodeType"].as_i64().unwrap_or(0);
    if node_type == 1 {
        let local_name = root["localName"]
            .as_str()
            .or_else(|| root["nodeName"].as_str())
            .unwrap_or("")
            .to_lowercase();
        if local_name.is_empty() {
            parent_xpath.to_string()
        } else if parent_xpath.is_empty() {
            format!("/{local_name}[1]")
        } else {
            format!("{parent_xpath}/{local_name}[1]")
        }
    } else {
        // Document nodes (type 9), doctype (type 10), etc. — no segment
        parent_xpath.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_walk_simple_tree() {
        // Minimal DOM: <html><body><div/><div/></body></html>
        let root = json!({
            "nodeType": 1,
            "localName": "html",
            "backendNodeId": 1,
            "children": [
                {
                    "nodeType": 1,
                    "localName": "body",
                    "backendNodeId": 2,
                    "children": [
                        {
                            "nodeType": 1,
                            "localName": "div",
                            "backendNodeId": 3,
                            "children": []
                        },
                        {
                            "nodeType": 1,
                            "localName": "div",
                            "backendNodeId": 4,
                            "children": []
                        },
                        {
                            "nodeType": 1,
                            "localName": "a",
                            "backendNodeId": 5,
                            "children": []
                        }
                    ]
                }
            ]
        });

        let mut map = HashMap::new();
        walk_dom_tree(&root, "", &mut map);

        assert_eq!(map.get(&1), Some(&"/html[1]".to_string()));
        assert_eq!(map.get(&2), Some(&"/html[1]/body[1]".to_string()));
        assert_eq!(map.get(&3), Some(&"/html[1]/body[1]/div[1]".to_string()));
        assert_eq!(map.get(&4), Some(&"/html[1]/body[1]/div[2]".to_string()));
        assert_eq!(map.get(&5), Some(&"/html[1]/body[1]/a[1]".to_string()));
    }
}
