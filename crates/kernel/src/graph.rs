use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::{KernelError, Result};

#[derive(Clone, Debug)]
pub struct PluginNode {
    pub id: &'static str,
    pub requires: &'static [&'static str],
}

/// 确定性拓扑排序。环或缺失依赖在启动期失败。
pub fn resolve_boot_order(nodes: &[PluginNode]) -> Result<Vec<&'static str>> {
    let mut ids = HashSet::new();
    for node in nodes {
        crate::ids::validate_plugin_id(node.id)?;
        if !ids.insert(node.id) {
            return Err(KernelError::DuplicatePlugin(node.id.to_string()));
        }
    }
    for node in nodes {
        for req in node.requires {
            if !ids.contains(req) {
                return Err(KernelError::MissingDependency(
                    node.id.to_string(),
                    (*req).to_string(),
                ));
            }
        }
    }

    let mut incoming: HashMap<&str, usize> = nodes.iter().map(|n| (n.id, 0usize)).collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in nodes {
        for req in node.requires {
            outgoing.entry(*req).or_default().push(node.id);
            *incoming.get_mut(node.id).expect("node") += 1;
        }
    }

    let mut ready: VecDeque<&str> =
        nodes.iter().filter(|n| incoming[&n.id] == 0).map(|n| n.id).collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(id) = ready.pop_front() {
        order.push(id);
        if let Some(children) = outgoing.get(id) {
            for child in children {
                let count = incoming.get_mut(child).expect("child");
                *count -= 1;
                if *count == 0 {
                    ready.push_back(child);
                }
            }
        }
    }
    if order.len() != nodes.len() {
        let cycle: Vec<_> =
            incoming.into_iter().filter(|(_, n)| *n > 0).map(|(id, _)| id).collect();
        return Err(KernelError::DependencyCycle(cycle.join(",")));
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_dependencies_first() {
        let nodes = [
            PluginNode { id: "asterism.sync", requires: &["asterism.domain"] },
            PluginNode { id: "asterism.domain", requires: &[] },
            PluginNode { id: "asterism.clipboard", requires: &["asterism.domain"] },
        ];
        let order = resolve_boot_order(&nodes).unwrap();
        assert_eq!(order[0], "asterism.domain");
        assert!(order.contains(&"asterism.sync"));
        assert!(order.contains(&"asterism.clipboard"));
    }

    #[test]
    fn rejects_cycles() {
        let nodes = [
            PluginNode { id: "asterism.a", requires: &["asterism.b"] },
            PluginNode { id: "asterism.b", requires: &["asterism.a"] },
        ];
        assert!(matches!(resolve_boot_order(&nodes), Err(KernelError::DependencyCycle(_))));
    }
}
