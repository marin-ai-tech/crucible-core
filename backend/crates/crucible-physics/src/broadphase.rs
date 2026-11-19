//! Broad phase using a dynamic AABB tree based on the Box2D algorithm.
//! It uses perimeter-based insertion costs, rotations for balancing, and enlarged bounds
//! to avoid rebuilding the tree every substep.
//!
//! Nodes share a vector and free list. Queries reuse their traversal stack, so the tree
//! allocates no memory after warm-up. Query order depends on tree shape, but each physics
//! step sorts the pairs to make its results independent of tree shape.

use crate::math::Vec2;
use crate::shape::Aabb;

pub(crate) const NULL_NODE: u32 = u32::MAX;

/// AABB expansion margin in meters.
const AABB_EXTENSION: f32 = 0.1;
/// Multiplier for prediction based on displacement in one substep.
const AABB_MULTIPLIER: f32 = 4.0;

/// Leaf data containing a body and fixture index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProxyData {
    pub body: u32,
    pub fixture: u32,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    aabb: Aabb,
    /// Parent index, or the next free node for an unused slot.
    parent: u32,
    child1: u32,
    child2: u32,
    /// Subtree height, 0 for leaves and −1 for free nodes.
    height: i32,
    data: ProxyData,
}

impl Node {
    #[inline]
    fn is_leaf(&self) -> bool {
        self.child1 == NULL_NODE
    }
}

/// Dynamic AABB tree.
#[derive(Clone, Debug)]
pub(crate) struct DynamicTree {
    nodes: Vec<Node>,
    root: u32,
    free_list: u32,
    stack: Vec<u32>,
}

impl Default for DynamicTree {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            root: NULL_NODE,
            free_list: NULL_NODE,
            stack: Vec::new(),
        }
    }
}

impl DynamicTree {
    fn allocate_node(&mut self) -> u32 {
        if self.free_list == NULL_NODE {
            let id = self.nodes.len() as u32;
            self.nodes.push(Node {
                aabb: Aabb::default(),
                parent: NULL_NODE,
                child1: NULL_NODE,
                child2: NULL_NODE,
                height: 0,
                data: ProxyData::default(),
            });
            return id;
        }
        let id = self.free_list;
        let n = &mut self.nodes[id as usize];
        self.free_list = n.parent;
        n.parent = NULL_NODE;
        n.child1 = NULL_NODE;
        n.child2 = NULL_NODE;
        n.height = 0;
        id
    }

    fn free_node(&mut self, id: u32) {
        let n = &mut self.nodes[id as usize];
        n.parent = self.free_list;
        n.height = -1;
        self.free_list = id;
    }

    /// Creates a leaf for the exact `aabb` and stores expanded bounds.
    pub fn create_proxy(&mut self, aabb: Aabb, data: ProxyData) -> u32 {
        let id = self.allocate_node();
        {
            let n = &mut self.nodes[id as usize];
            n.aabb = aabb.enlarged(AABB_EXTENSION);
            n.data = data;
            n.height = 0;
        }
        self.insert_leaf(id);
        id
    }

    pub fn destroy_proxy(&mut self, id: u32) {
        self.remove_leaf(id);
        self.free_node(id);
    }

    /// Updates the leaf if its exact bounds leave the expanded bounds. Uses substep
    /// `displacement` to extend the new bounds. Returns whether the leaf was rebuilt.
    pub fn move_proxy(&mut self, id: u32, aabb: Aabb, displacement: Vec2) -> bool {
        if self.nodes[id as usize].aabb.contains(&aabb) {
            return false;
        }
        self.remove_leaf(id);
        let mut b = aabb.enlarged(AABB_EXTENSION);
        let d = displacement * AABB_MULTIPLIER;
        if d.x < 0.0 {
            b.lower.x += d.x;
        } else {
            b.upper.x += d.x;
        }
        if d.y < 0.0 {
            b.lower.y += d.y;
        } else {
            b.upper.y += d.y;
        }
        self.nodes[id as usize].aabb = b;
        self.insert_leaf(id);
        true
    }

    #[inline]
    pub fn data(&self, id: u32) -> ProxyData {
        self.nodes[id as usize].data
    }

    /// Calls `f` for each leaf whose expanded bounds overlap `aabb`.
    pub fn query(&mut self, aabb: &Aabb, mut f: impl FnMut(u32, ProxyData)) {
        self.stack.clear();
        if self.root != NULL_NODE {
            self.stack.push(self.root);
        }
        while let Some(id) = self.stack.pop() {
            let n = self.nodes[id as usize];
            if !n.aabb.overlaps(aabb) {
                continue;
            }
            if n.is_leaf() {
                f(id, n.data);
            } else {
                self.stack.push(n.child1);
                self.stack.push(n.child2);
            }
        }
    }

    fn insert_leaf(&mut self, leaf: u32) {
        if self.root == NULL_NODE {
            self.root = leaf;
            self.nodes[leaf as usize].parent = NULL_NODE;
            return;
        }

        // Choose the best sibling by perimeter cost.
        let leaf_aabb = self.nodes[leaf as usize].aabb;
        let mut index = self.root;
        while !self.nodes[index as usize].is_leaf() {
            let n = self.nodes[index as usize];
            let area = n.aabb.perimeter();
            let combined_area = n.aabb.union(&leaf_aabb).perimeter();
            // Cost of a new parent for this node and the leaf.
            let cost = 2.0 * combined_area;
            // Minimum cost of descending further.
            let inheritance_cost = 2.0 * (combined_area - area);
            let child_cost = |c: u32| {
                let cn = &self.nodes[c as usize];
                let merged = leaf_aabb.union(&cn.aabb).perimeter();
                if cn.is_leaf() {
                    merged + inheritance_cost
                } else {
                    merged - cn.aabb.perimeter() + inheritance_cost
                }
            };
            let cost1 = child_cost(n.child1);
            let cost2 = child_cost(n.child2);
            if cost < cost1 && cost < cost2 {
                break;
            }
            index = if cost1 < cost2 { n.child1 } else { n.child2 };
        }
        let sibling = index;

        // Create the new parent.
        let old_parent = self.nodes[sibling as usize].parent;
        let new_parent = self.allocate_node();
        {
            let sibling_aabb = self.nodes[sibling as usize].aabb;
            let sibling_height = self.nodes[sibling as usize].height;
            let np = &mut self.nodes[new_parent as usize];
            np.parent = old_parent;
            np.data = ProxyData::default();
            np.aabb = leaf_aabb.union(&sibling_aabb);
            np.height = sibling_height + 1;
            np.child1 = sibling;
            np.child2 = leaf;
        }
        if old_parent != NULL_NODE {
            let op = &mut self.nodes[old_parent as usize];
            if op.child1 == sibling {
                op.child1 = new_parent;
            } else {
                op.child2 = new_parent;
            }
        } else {
            self.root = new_parent;
        }
        self.nodes[sibling as usize].parent = new_parent;
        self.nodes[leaf as usize].parent = new_parent;

        // Walk upward, repairing heights and bounds.
        self.fix_upwards(new_parent);
    }

    fn remove_leaf(&mut self, leaf: u32) {
        if leaf == self.root {
            self.root = NULL_NODE;
            return;
        }
        let parent = self.nodes[leaf as usize].parent;
        let grand_parent = self.nodes[parent as usize].parent;
        let p = self.nodes[parent as usize];
        let sibling = if p.child1 == leaf { p.child2 } else { p.child1 };

        if grand_parent != NULL_NODE {
            let gp = &mut self.nodes[grand_parent as usize];
            if gp.child1 == parent {
                gp.child1 = sibling;
            } else {
                gp.child2 = sibling;
            }
            self.nodes[sibling as usize].parent = grand_parent;
            self.free_node(parent);
            self.fix_upwards(grand_parent);
        } else {
            self.root = sibling;
            self.nodes[sibling as usize].parent = NULL_NODE;
            self.free_node(parent);
        }
    }

    fn fix_upwards(&mut self, start: u32) {
        let mut index = start;
        while index != NULL_NODE {
            index = self.balance(index);
            let n = self.nodes[index as usize];
            let c1 = self.nodes[n.child1 as usize];
            let c2 = self.nodes[n.child2 as usize];
            let node = &mut self.nodes[index as usize];
            node.height = 1 + c1.height.max(c2.height);
            node.aabb = c1.aabb.union(&c2.aabb);
            index = node.parent;
        }
    }

    /// Rotates a subtree when child heights differ by more than 1. Returns its new root.
    fn balance(&mut self, a: u32) -> u32 {
        let na = self.nodes[a as usize];
        if na.is_leaf() || na.height < 2 {
            return a;
        }
        let b = na.child1;
        let c = na.child2;
        let balance = self.nodes[c as usize].height - self.nodes[b as usize].height;
        if balance > 1 {
            self.rotate_up(a, c, b, true)
        } else if balance < -1 {
            self.rotate_up(a, b, c, false)
        } else {
            a
        }
    }

    /// Promotes child `up` of `a` into `a`'s position. `other` is the other child.
    /// `up_is_child2` indicates whether `up` was the second child.
    fn rotate_up(&mut self, a: u32, up: u32, other: u32, up_is_child2: bool) -> u32 {
        let nu = self.nodes[up as usize];
        let f = nu.child1;
        let g = nu.child2;
        let a_parent = self.nodes[a as usize].parent;

        // Make `up` the parent of `a`.
        self.nodes[up as usize].child1 = a;
        self.nodes[up as usize].parent = a_parent;
        self.nodes[a as usize].parent = up;
        if a_parent != NULL_NODE {
            let ap = &mut self.nodes[a_parent as usize];
            if ap.child1 == a {
                ap.child1 = up;
            } else {
                ap.child2 = up;
            }
        } else {
            self.root = up;
        }

        // Keep the taller grandchild under `up` and move the shorter one to `a`.
        let (keep, give) = if self.nodes[f as usize].height > self.nodes[g as usize].height {
            (f, g)
        } else {
            (g, f)
        };
        self.nodes[up as usize].child2 = keep;
        if up_is_child2 {
            self.nodes[a as usize].child2 = give;
        } else {
            self.nodes[a as usize].child1 = give;
        }
        self.nodes[give as usize].parent = a;

        let other_aabb = self.nodes[other as usize].aabb;
        let other_h = self.nodes[other as usize].height;
        let give_aabb = self.nodes[give as usize].aabb;
        let give_h = self.nodes[give as usize].height;
        let a_aabb = other_aabb.union(&give_aabb);
        let a_h = 1 + other_h.max(give_h);
        self.nodes[a as usize].aabb = a_aabb;
        self.nodes[a as usize].height = a_h;
        let keep_aabb = self.nodes[keep as usize].aabb;
        let keep_h = self.nodes[keep as usize].height;
        self.nodes[up as usize].aabb = a_aabb.union(&keep_aabb);
        self.nodes[up as usize].height = 1 + a_h.max(keep_h);
        up
    }

    /// Tests parent links, heights, and whether parents' bounds contain their children.
    #[cfg(test)]
    fn validate(&self) -> usize {
        let mut leaves = 0;
        if self.root == NULL_NODE {
            return 0;
        }
        let mut stack = vec![self.root];
        assert_eq!(self.nodes[self.root as usize].parent, NULL_NODE);
        while let Some(id) = stack.pop() {
            let n = self.nodes[id as usize];
            if n.is_leaf() {
                assert_eq!(n.height, 0);
                leaves += 1;
                continue;
            }
            let c1 = self.nodes[n.child1 as usize];
            let c2 = self.nodes[n.child2 as usize];
            assert_eq!(c1.parent, id);
            assert_eq!(c2.parent, id);
            assert_eq!(n.height, 1 + c1.height.max(c2.height));
            assert!(n.aabb.contains(&c1.aabb) && n.aabb.contains(&c2.aabb));
            stack.push(n.child1);
            stack.push(n.child2);
        }
        leaves
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: f32, y: f32) -> Aabb {
        Aabb::new(Vec2::new(x, y), Vec2::new(x + 1.0, y + 1.0))
    }

    #[test]
    fn query_finds_exactly_overlapping_leaves() {
        let mut t = DynamicTree::default();
        let mut ids = Vec::new();
        for i in 0..40 {
            let x = (i % 8) as f32 * 2.0;
            let y = (i / 8) as f32 * 2.0;
            ids.push(t.create_proxy(
                cell(x, y),
                ProxyData {
                    body: i,
                    fixture: 0,
                },
            ));
        }
        assert_eq!(t.validate(), 40);
        let probe = Aabb::new(Vec2::new(1.5, 1.5), Vec2::new(4.5, 2.5));
        let mut found = Vec::new();
        t.query(&probe, |_, d| found.push(d.body));
        found.sort_unstable();
        // With the 0.1 margin, only bounds at (2, 2) and (4, 2) overlap the probe.
        assert_eq!(found, vec![9, 10]);

        // Removal and movement preserve the tree invariants.
        for id in ids.iter().step_by(3) {
            t.destroy_proxy(*id);
        }
        assert_eq!(t.validate(), 40 - 14);
        let moved = t.move_proxy(ids[1], cell(30.0, 30.0), Vec2::new(1.0, 0.0));
        assert!(moved);
        assert!(!t.move_proxy(ids[1], cell(30.0, 30.0), Vec2::ZERO));
        assert_eq!(t.validate(), 40 - 14);
        let mut far = Vec::new();
        t.query(&cell(30.5, 30.5), |_, d| far.push(d.body));
        assert_eq!(far, vec![1]);
    }
}
