//! Deterministic topological sorting with before and after constraints.

use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Key of the owning system, with a type and name for sorting and diagnostics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SystemKey {
    pub(crate) type_id: TypeId,
    pub(crate) name: &'static str,
}

impl SystemKey {
    pub(crate) fn of<S: 'static>() -> SystemKey {
        SystemKey {
            type_id: TypeId::of::<S>(),
            name: std::any::type_name::<S>(),
        }
    }
}

/// A node with its value (subscription or system), owner, and ordering constraints.
#[derive(Clone, Debug)]
pub(crate) struct OrderNode {
    pub(crate) owner: SystemKey,
    /// Registration order breaks ties after the owner's name.
    pub(crate) seq: usize,
    pub(crate) before: Vec<SystemKey>,
    pub(crate) after: Vec<SystemKey>,
}

/// Sorts nodes to satisfy all constraints. Among unconstrained nodes, the one with the smallest
/// (owner name, registration number) comes first. The result depends on the set of nodes
/// and constraints regardless of the registration order of differently named systems.
///
/// A constraint on a system with no nodes in this set has no effect.
/// On a cycle, returns the names of the owners in traversal order.
pub(crate) fn sort(nodes: &[OrderNode]) -> Result<Vec<usize>, Vec<&'static str>> {
    let n = nodes.len();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree = vec![0usize; n];
    let mut add_edge = |from: usize, to: usize, succ: &mut Vec<Vec<usize>>| {
        if !succ[from].contains(&to) {
            succ[from].push(to);
            indegree[to] += 1;
        }
    };
    for (i, node) in nodes.iter().enumerate() {
        for target in &node.after {
            for (j, other) in nodes.iter().enumerate() {
                if other.owner.type_id == target.type_id {
                    add_edge(j, i, &mut successors);
                }
            }
        }
        for target in &node.before {
            for (j, other) in nodes.iter().enumerate() {
                if other.owner.type_id == target.type_id {
                    add_edge(i, j, &mut successors);
                }
            }
        }
    }

    let key = |i: usize| Reverse((nodes[i].owner.name, nodes[i].seq, i));
    let mut ready: BinaryHeap<_> = (0..n).filter(|&i| indegree[i] == 0).map(key).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(Reverse((_, _, i))) = ready.pop() {
        order.push(i);
        for &next in &successors[i] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(key(next));
            }
        }
    }
    if order.len() == n {
        return Ok(order);
    }
    Err(find_cycle(nodes, &successors, &indegree))
}

/// Finds a cycle among the unsorted nodes. Each has an incoming edge from another unsorted node.
fn find_cycle(
    nodes: &[OrderNode],
    successors: &[Vec<usize>],
    indegree: &[usize],
) -> Vec<&'static str> {
    let left: Vec<bool> = indegree.iter().map(|&d| d > 0).collect();
    let mut predecessor = vec![None; nodes.len()];
    for (from, succ) in successors.iter().enumerate() {
        if !left[from] {
            continue;
        }
        for &to in succ {
            if left[to] && predecessor[to].is_none() {
                predecessor[to] = Some(from);
            }
        }
    }
    let start = left
        .iter()
        .position(|&l| l)
        .expect("there is an unsorted node");
    // Follow predecessors until a node repeats. The repeated node is on the cycle.
    let mut seen = vec![false; nodes.len()];
    let mut current = start;
    while !seen[current] {
        seen[current] = true;
        current = predecessor[current].expect("a node on the cycle has a predecessor");
    }
    let mut cycle = vec![current];
    let mut walk = predecessor[current].expect("predecessor");
    while walk != current {
        cycle.push(walk);
        walk = predecessor[walk].expect("predecessor");
    }
    cycle.reverse();
    let mut names: Vec<&'static str> = Vec::new();
    for i in cycle {
        let name = nodes[i].owner.name;
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;
    struct C;

    fn node(
        owner: SystemKey,
        seq: usize,
        before: Vec<SystemKey>,
        after: Vec<SystemKey>,
    ) -> OrderNode {
        OrderNode {
            owner,
            seq,
            before,
            after,
        }
    }

    #[test]
    fn after_and_before_are_respected() {
        let (a, b, c) = (
            SystemKey::of::<A>(),
            SystemKey::of::<B>(),
            SystemKey::of::<C>(),
        );
        // A sorts before C by name, but the constraint puts C first.
        let nodes = vec![
            node(a, 0, vec![], vec![c]),
            node(b, 1, vec![c], vec![]),
            node(c, 2, vec![], vec![]),
        ];
        let order = sort(&nodes).unwrap();
        let names: Vec<_> = order.iter().map(|&i| nodes[i].owner.name).collect();
        assert_eq!(names, vec![b.name, c.name, a.name]);
    }

    #[test]
    fn cycle_names_participants_only() {
        let (a, b, c) = (
            SystemKey::of::<A>(),
            SystemKey::of::<B>(),
            SystemKey::of::<C>(),
        );
        let nodes = vec![
            node(c, 0, vec![a], vec![]),
            node(a, 1, vec![], vec![b]),
            node(b, 2, vec![], vec![a]),
        ];
        let cycle = sort(&nodes).unwrap_err();
        assert_eq!(cycle.len(), 2);
        assert!(cycle.contains(&a.name) && cycle.contains(&b.name));
    }

    #[test]
    fn self_reference_is_a_cycle() {
        let a = SystemKey::of::<A>();
        let cycle = sort(&[node(a, 0, vec![], vec![a])]).unwrap_err();
        assert_eq!(cycle, vec![a.name]);
    }
}
