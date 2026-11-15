//! Brigadier usage strings, as `/help` prints them.

use super::{CommandDispatcher, CommandRuntime, NodeId, NodeKind};

impl<S, R> CommandDispatcher<S, R>
where
    R: CommandRuntime<S>,
{
    /// Brigadier `getSmartUsage`: one usage line for each child of `node`
    /// that `source` can use, in registration order, without the node
    /// itself.
    ///
    /// A child the source cannot use has no line. Children of an
    /// executable node are optional, so they are wrapped in `[` `]`.
    pub(crate) fn smart_usage(&self, node: NodeId, source: &S) -> Vec<(NodeId, String)> {
        let Some(parent) = self.node(node) else {
            return Vec::new();
        };
        let optional = parent.is_executable();
        self.children(node)
            .unwrap_or_default()
            .iter()
            .filter_map(|&child| {
                self.smart_usage_of(child, source, optional, false)
                    .map(|usage| (child, usage))
            })
            .collect()
    }

    /// Brigadier `getUsageText`: a literal's name, an argument's name in
    /// `<` `>`, and nothing for the root.
    fn usage_text(&self, node: NodeId) -> String {
        let Some(node) = self.node(node) else {
            return String::new();
        };
        match node.kind() {
            NodeKind::Root => String::new(),
            NodeKind::Literal => node.name().to_owned(),
            NodeKind::Argument => format!("<{}>", node.name()),
        }
    }

    fn smart_usage_of(
        &self,
        node_id: NodeId,
        source: &S,
        optional: bool,
        deep: bool,
    ) -> Option<String> {
        let node = self.node(node_id)?;
        if !node.allows(source) {
            return None;
        }
        let usage_text = self.usage_text(node_id);
        let own = if optional {
            format!("[{usage_text}]")
        } else {
            usage_text
        };
        if deep {
            return Some(own);
        }
        if let Some(target) = node.redirect() {
            let redirect = if target == self.root() {
                "...".to_owned()
            } else {
                format!("-> {}", self.usage_text(target))
            };
            return Some(format!("{own} {redirect}"));
        }

        let child_optional = node.is_executable();
        let children: Vec<NodeId> = self
            .children(node_id)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|&child| self.node(child).is_some_and(|child| child.allows(source)))
            .collect();
        match children.as_slice() {
            [] => {}
            [only] => {
                if let Some(usage) =
                    self.smart_usage_of(*only, source, child_optional, child_optional)
                {
                    return Some(format!("{own} {usage}"));
                }
            }
            _ => {
                let mut child_usages: Vec<String> = Vec::new();
                for &child in &children {
                    if let Some(usage) = self.smart_usage_of(child, source, child_optional, true)
                        && !child_usages.contains(&usage)
                    {
                        child_usages.push(usage);
                    }
                }
                if let [usage] = child_usages.as_slice() {
                    let usage = if child_optional {
                        format!("[{usage}]")
                    } else {
                        usage.clone()
                    };
                    return Some(format!("{own} {usage}"));
                }
                if child_usages.len() > 1 {
                    let (open, close) = if child_optional {
                        ("[", "]")
                    } else {
                        ("(", ")")
                    };
                    let alternatives = children
                        .iter()
                        .map(|&child| self.usage_text(child))
                        .collect::<Vec<_>>()
                        .join("|");
                    return Some(format!("{own} {open}{alternatives}{close}"));
                }
            }
        }
        Some(own)
    }
}
