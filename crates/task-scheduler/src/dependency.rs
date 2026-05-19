//! Task dependency management with cycle detection and execution ordering.
//!
//! This module provides:
//! - Validation of task dependencies (existence, acyclicity, depth limits)
//! - DFS-based cycle detection that identifies cycle members
//! - Dependency chain depth calculation with a maximum of 10
//! - Tracking of task completion status for dependency resolution
//! - Notification when dependent tasks become unblocked

use std::collections::{HashMap, HashSet, VecDeque};

use common::errors::TaskError;
use common::types::TaskId;

/// Maximum allowed depth for a dependency chain.
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Tracks task dependencies and completion status.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// Maps each task to its direct dependencies (tasks it depends on).
    edges: HashMap<TaskId, Vec<TaskId>>,
    /// Maps each task to its dependents (tasks that depend on it).
    reverse_edges: HashMap<TaskId, Vec<TaskId>>,
    /// Set of tasks that have completed successfully.
    completed: HashSet<TaskId>,
}

impl DependencyGraph {
    /// Create a new empty dependency graph.
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            completed: HashSet::new(),
        }
    }

    /// Validate and register a task's dependencies.
    ///
    /// This checks:
    /// 1. All dependency task IDs exist in the graph (or in `known_tasks`)
    /// 2. Adding this task does not create a cycle
    /// 3. The dependency chain depth does not exceed MAX_DEPENDENCY_DEPTH
    ///
    /// Returns `Ok(())` if the task can be safely added, or an appropriate
    /// `TaskError` if validation fails.
    pub fn add_task(
        &mut self,
        task_id: TaskId,
        dependencies: &[TaskId],
        known_tasks: &HashSet<TaskId>,
    ) -> Result<(), TaskError> {
        // Check that all dependencies reference existing tasks
        for dep_id in dependencies {
            if !known_tasks.contains(dep_id) && !self.edges.contains_key(dep_id) {
                return Err(TaskError::NotFound {
                    id: dep_id.to_string(),
                });
            }
        }

        // Temporarily add the edges to check for cycles
        self.edges.insert(task_id, dependencies.to_vec());
        for dep_id in dependencies {
            self.reverse_edges
                .entry(*dep_id)
                .or_default()
                .push(task_id);
        }

        // Check for cycles using DFS
        if let Some(cycle) = self.detect_cycle(task_id) {
            // Roll back the addition
            self.edges.remove(&task_id);
            for dep_id in dependencies {
                if let Some(dependents) = self.reverse_edges.get_mut(dep_id) {
                    dependents.retain(|id| *id != task_id);
                }
            }
            return Err(TaskError::DependencyCycle {
                cycle_members: cycle.into_iter().map(|id| id.to_string()).collect(),
            });
        }

        // Check dependency chain depth
        let depth = self.calculate_depth(task_id);
        if depth > MAX_DEPENDENCY_DEPTH {
            // Roll back the addition
            self.edges.remove(&task_id);
            for dep_id in dependencies {
                if let Some(dependents) = self.reverse_edges.get_mut(dep_id) {
                    dependents.retain(|id| *id != task_id);
                }
            }
            return Err(TaskError::DependencyTooDeep {
                depth,
                max_depth: MAX_DEPENDENCY_DEPTH,
            });
        }

        Ok(())
    }

    /// Remove a task from the dependency graph.
    pub fn remove_task(&mut self, task_id: &TaskId) {
        // Remove forward edges
        if let Some(deps) = self.edges.remove(task_id) {
            for dep_id in &deps {
                if let Some(dependents) = self.reverse_edges.get_mut(dep_id) {
                    dependents.retain(|id| id != task_id);
                }
            }
        }

        // Remove reverse edges (other tasks depending on this one)
        if let Some(dependents) = self.reverse_edges.remove(task_id) {
            for dependent_id in &dependents {
                if let Some(deps) = self.edges.get_mut(dependent_id) {
                    deps.retain(|id| id != task_id);
                }
            }
        }

        self.completed.remove(task_id);
    }

    /// Mark a task as successfully completed and return any tasks
    /// that are now unblocked (all their dependencies are satisfied).
    pub fn mark_completed(&mut self, task_id: TaskId) -> Vec<TaskId> {
        self.completed.insert(task_id);

        // Find tasks that depend on this one and check if they're now unblocked
        let dependents = self
            .reverse_edges
            .get(&task_id)
            .cloned()
            .unwrap_or_default();

        let mut unblocked = Vec::new();
        for dependent_id in dependents {
            if self.is_unblocked(&dependent_id) {
                unblocked.push(dependent_id);
            }
        }

        unblocked
    }

    /// Check if a task has all its dependencies satisfied.
    pub fn is_unblocked(&self, task_id: &TaskId) -> bool {
        match self.edges.get(task_id) {
            Some(deps) => deps.iter().all(|dep| self.completed.contains(dep)),
            None => true, // No dependencies means always unblocked
        }
    }

    /// Check if a task has any dependencies at all.
    pub fn has_dependencies(&self, task_id: &TaskId) -> bool {
        self.edges
            .get(task_id)
            .map(|deps| !deps.is_empty())
            .unwrap_or(false)
    }

    /// Get the direct dependencies of a task.
    pub fn dependencies_of(&self, task_id: &TaskId) -> &[TaskId] {
        self.edges.get(task_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get the tasks that directly depend on the given task.
    pub fn dependents_of(&self, task_id: &TaskId) -> &[TaskId] {
        self.reverse_edges
            .get(task_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Detect a cycle starting from the given task using DFS.
    /// Returns `Some(cycle_members)` if a cycle is found, `None` otherwise.
    fn detect_cycle(&self, start: TaskId) -> Option<Vec<TaskId>> {
        // Standard DFS-based cycle detection with path tracking
        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();
        let mut path = Vec::new();

        if self.dfs_cycle(start, &mut visited, &mut in_stack, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    /// Recursive DFS that detects cycles and records the cycle path.
    fn dfs_cycle(
        &self,
        node: TaskId,
        visited: &mut HashSet<TaskId>,
        in_stack: &mut HashSet<TaskId>,
        path: &mut Vec<TaskId>,
    ) -> bool {
        visited.insert(node);
        in_stack.insert(node);
        path.push(node);

        if let Some(deps) = self.edges.get(&node) {
            for &dep in deps {
                if !visited.contains(&dep) {
                    if self.dfs_cycle(dep, visited, in_stack, path) {
                        return true;
                    }
                } else if in_stack.contains(&dep) {
                    // Found a cycle — trim path to only include cycle members
                    if let Some(pos) = path.iter().position(|&id| id == dep) {
                        *path = path[pos..].to_vec();
                    }
                    return true;
                }
            }
        }

        path.pop();
        in_stack.remove(&node);
        false
    }

    /// Calculate the maximum depth of the dependency chain for a task.
    /// Depth is defined as the longest path from the task to a task with no dependencies.
    fn calculate_depth(&self, task_id: TaskId) -> usize {
        let mut memo: HashMap<TaskId, usize> = HashMap::new();
        self.depth_dfs(task_id, &mut memo)
    }

    /// DFS helper for depth calculation with memoization.
    fn depth_dfs(&self, task_id: TaskId, memo: &mut HashMap<TaskId, usize>) -> usize {
        if let Some(&cached) = memo.get(&task_id) {
            return cached;
        }

        let deps = match self.edges.get(&task_id) {
            Some(deps) if !deps.is_empty() => deps.clone(),
            _ => {
                memo.insert(task_id, 1);
                return 1;
            }
        };

        let max_dep_depth = deps
            .iter()
            .map(|dep| self.depth_dfs(*dep, memo))
            .max()
            .unwrap_or(0);

        let depth = max_dep_depth + 1;
        memo.insert(task_id, depth);
        depth
    }

    /// Get a valid execution order for all tasks (topological sort).
    /// Returns tasks in an order where dependencies come before dependents.
    pub fn topological_order(&self) -> Vec<TaskId> {
        let mut in_degree: HashMap<TaskId, usize> = HashMap::new();

        // Initialize in-degrees
        for &task_id in self.edges.keys() {
            in_degree.entry(task_id).or_insert(0);
        }
        for deps in self.edges.values() {
            for &dep in deps {
                // dep is depended upon, but we track in-degree of the task that has deps
                in_degree.entry(dep).or_insert(0);
            }
        }

        // Calculate in-degrees (number of dependencies each task has)
        for (task_id, deps) in &self.edges {
            *in_degree.entry(*task_id).or_insert(0) = deps.len();
        }

        // BFS with queue of tasks that have no unsatisfied dependencies
        let mut queue: VecDeque<TaskId> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut order = Vec::new();

        while let Some(task_id) = queue.pop_front() {
            order.push(task_id);

            if let Some(dependents) = self.reverse_edges.get(&task_id) {
                for &dependent in dependents {
                    if let Some(deg) = in_degree.get_mut(&dependent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dependent);
                        }
                    }
                }
            }
        }

        order
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_known_tasks(ids: &[TaskId]) -> HashSet<TaskId> {
        ids.iter().copied().collect()
    }

    #[test]
    fn test_add_task_no_dependencies() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let known = make_known_tasks(&[task_a]);

        let result = graph.add_task(task_a, &[], &known);
        assert!(result.is_ok());
        assert!(!graph.has_dependencies(&task_a));
        assert!(graph.is_unblocked(&task_a));
    }

    #[test]
    fn test_add_task_with_valid_dependency() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b]);

        // Add task_a first (no deps)
        graph.add_task(task_a, &[], &known).unwrap();

        // Add task_b depending on task_a
        let result = graph.add_task(task_b, &[task_a], &known);
        assert!(result.is_ok());
        assert!(graph.has_dependencies(&task_b));
        assert!(!graph.is_unblocked(&task_b));
    }

    #[test]
    fn test_add_task_with_nonexistent_dependency() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let nonexistent = TaskId::new();
        let known = make_known_tasks(&[task_a]);

        let result = graph.add_task(task_a, &[nonexistent], &known);
        assert!(matches!(result, Err(TaskError::NotFound { .. })));
    }

    #[test]
    fn test_detect_simple_cycle() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let task_c = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b, task_c]);

        // Build: A (no deps), B depends on A
        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[task_a], &known).unwrap();

        // Now try to add C depending on B, and also make A depend on C
        // But we can't modify A's deps after creation. Instead:
        // Try to add C that depends on B, then try to add a task that
        // creates a back-edge.
        graph.add_task(task_c, &[task_b], &known).unwrap();

        // Now try to add a new task D that depends on C,
        // and also have A depend on D — but we can't modify A.
        // The simplest cycle test: try to add a task that depends on
        // something that transitively depends on it.
        // Since A -> (nothing), B -> A, C -> B
        // If we try to make A depend on C: A -> C -> B -> A (cycle!)
        // But A is already in the graph. We need to remove and re-add.
        // However, remove cleans up B's dep on A.
        
        // Better approach: build the cycle in one shot.
        // Fresh graph: A depends on B, B depends on A
        let mut graph2 = DependencyGraph::new();
        let x = TaskId::new();
        let y = TaskId::new();
        let known2 = make_known_tasks(&[x, y]);

        // Add X with no deps
        graph2.add_task(x, &[], &known2).unwrap();
        // Add Y depending on X
        graph2.add_task(y, &[x], &known2).unwrap();

        // Now try to add a new task Z that depends on Y,
        // and Y already depends on X. No cycle yet.
        // To create a cycle, we need X to depend on Y (or something downstream).
        // Since X is already added with no deps, we can't change it.
        
        // The only way to create a cycle is if the NEW task being added
        // is already a dependency (direct or transitive) of one of its own dependencies.
        // Example: X (no deps), Y depends on X. Now add X2 that depends on Y
        // and is also a dependency of X. But X has no deps...
        
        // Actually, the cycle detection should catch: if we add task Z
        // with deps = [Y], and Y depends on X, and X depends on Z — but X has no deps.
        
        // The real scenario: self-dependency is already tested.
        // For a 2-node cycle: we need to add a task that depends on something
        // that already depends on it. This means the task being added must
        // already exist in the graph as a dependency of its own dependency.
        // But if it's being added for the first time, it can't be a dependency yet.
        
        // So a 2-node cycle can only happen if:
        // 1. Task A exists with dep on B
        // 2. Task B is added with dep on A
        // Let's test this:
        let mut graph3 = DependencyGraph::new();
        let p = TaskId::new();
        let q = TaskId::new();
        let known3 = make_known_tasks(&[p, q]);

        // Add P depending on Q (Q must exist in known_tasks)
        graph3.add_task(p, &[q], &known3).unwrap();
        // Now add Q depending on P — this should create a cycle: P -> Q -> P
        let result = graph3.add_task(q, &[p], &known3);
        assert!(matches!(result, Err(TaskError::DependencyCycle { .. })));
    }

    #[test]
    fn test_detect_three_node_cycle() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let task_c = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b, task_c]);

        // Build: A depends on B, B depends on C
        graph.add_task(task_a, &[task_b], &known).unwrap();
        graph.add_task(task_b, &[task_c], &known).unwrap();

        // Now try to add C depending on A — creates cycle: C -> A -> B -> C
        let result = graph.add_task(task_c, &[task_a], &known);
        assert!(matches!(result, Err(TaskError::DependencyCycle { .. })));

        if let Err(TaskError::DependencyCycle { cycle_members }) = result {
            // The cycle should contain at least 2 tasks
            assert!(cycle_members.len() >= 2);
        }
    }

    #[test]
    fn test_self_dependency_detected_as_cycle() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let known = make_known_tasks(&[task_a]);

        let result = graph.add_task(task_a, &[task_a], &known);
        assert!(matches!(result, Err(TaskError::DependencyCycle { .. })));
    }

    #[test]
    fn test_dependency_depth_within_limit() {
        let mut graph = DependencyGraph::new();
        let tasks: Vec<TaskId> = (0..10).map(|_| TaskId::new()).collect();
        let known: HashSet<TaskId> = tasks.iter().copied().collect();

        // Create a chain of depth 10: t0 <- t1 <- t2 <- ... <- t9
        graph.add_task(tasks[0], &[], &known).unwrap();
        for i in 1..10 {
            let result = graph.add_task(tasks[i], &[tasks[i - 1]], &known);
            assert!(result.is_ok(), "Failed at depth {}", i + 1);
        }
    }

    #[test]
    fn test_dependency_depth_exceeds_limit() {
        let mut graph = DependencyGraph::new();
        let tasks: Vec<TaskId> = (0..12).map(|_| TaskId::new()).collect();
        let known: HashSet<TaskId> = tasks.iter().copied().collect();

        // Create a chain of depth 11 (exceeds max of 10)
        graph.add_task(tasks[0], &[], &known).unwrap();
        for i in 1..10 {
            graph.add_task(tasks[i], &[tasks[i - 1]], &known).unwrap();
        }

        // The 11th task should fail (depth = 11)
        let result = graph.add_task(tasks[10], &[tasks[9]], &known);
        assert!(matches!(result, Err(TaskError::DependencyTooDeep { .. })));

        if let Err(TaskError::DependencyTooDeep { depth, max_depth }) = result {
            assert_eq!(depth, 11);
            assert_eq!(max_depth, MAX_DEPENDENCY_DEPTH);
        }
    }

    #[test]
    fn test_mark_completed_unblocks_dependents() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b]);

        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[task_a], &known).unwrap();

        assert!(!graph.is_unblocked(&task_b));

        let unblocked = graph.mark_completed(task_a);
        assert_eq!(unblocked, vec![task_b]);
        assert!(graph.is_unblocked(&task_b));
    }

    #[test]
    fn test_mark_completed_multiple_dependencies() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let task_c = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b, task_c]);

        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[], &known).unwrap();
        // C depends on both A and B
        graph.add_task(task_c, &[task_a, task_b], &known).unwrap();

        assert!(!graph.is_unblocked(&task_c));

        // Complete A — C still blocked (needs B)
        let unblocked = graph.mark_completed(task_a);
        assert!(unblocked.is_empty());
        assert!(!graph.is_unblocked(&task_c));

        // Complete B — C now unblocked
        let unblocked = graph.mark_completed(task_b);
        assert_eq!(unblocked, vec![task_c]);
        assert!(graph.is_unblocked(&task_c));
    }

    #[test]
    fn test_remove_task_cleans_up_edges() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b]);

        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[task_a], &known).unwrap();

        graph.remove_task(&task_a);

        // task_b should no longer have task_a as a dependency
        assert!(graph.dependencies_of(&task_b).is_empty());
    }

    #[test]
    fn test_topological_order_respects_dependencies() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let task_c = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b, task_c]);

        // C depends on B, B depends on A
        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[task_a], &known).unwrap();
        graph.add_task(task_c, &[task_b], &known).unwrap();

        let order = graph.topological_order();

        let pos_a = order.iter().position(|&id| id == task_a).unwrap();
        let pos_b = order.iter().position(|&id| id == task_b).unwrap();
        let pos_c = order.iter().position(|&id| id == task_c).unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_diamond_dependency_no_cycle() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let task_c = TaskId::new();
        let task_d = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b, task_c, task_d]);

        // Diamond: D depends on B and C, both B and C depend on A
        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[task_a], &known).unwrap();
        graph.add_task(task_c, &[task_a], &known).unwrap();
        let result = graph.add_task(task_d, &[task_b, task_c], &known);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dependents_of() {
        let mut graph = DependencyGraph::new();
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let task_c = TaskId::new();
        let known = make_known_tasks(&[task_a, task_b, task_c]);

        graph.add_task(task_a, &[], &known).unwrap();
        graph.add_task(task_b, &[task_a], &known).unwrap();
        graph.add_task(task_c, &[task_a], &known).unwrap();

        let dependents = graph.dependents_of(&task_a);
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&task_b));
        assert!(dependents.contains(&task_c));
    }
}
