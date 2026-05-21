//! Property-based tests for task dependency management.
//!
//! **Validates: Requirements 3.5, 3.6**
//!
//! Property 6: Task Dependency Cycle Detection
//! Property 7: Task Dependency Execution Order
//!
//! These tests generate arbitrary directed graphs and verify that:
//! - Cycle detection correctly identifies and rejects cyclic graphs
//! - Acyclic graphs with depth ≤ 10 are accepted
//! - Topological ordering respects all dependency edges
//! - mark_completed() correctly unblocks dependent tasks

use std::collections::{HashMap, HashSet};

use common::errors::TaskError;
use common::types::TaskId;
use proptest::prelude::*;
use task_scheduler::{DependencyGraph, MAX_DEPENDENCY_DEPTH};

// ============================================================================
// Strategies for generating arbitrary directed graphs
// ============================================================================

/// Generate a DAG with bounded depth (≤ MAX_DEPENDENCY_DEPTH).
/// We ensure depth is bounded by assigning each node a "level" (1..=MAX_DEPENDENCY_DEPTH)
/// and only allowing edges from higher levels to lower levels. Each node can only
/// depend on nodes at level - 1 or below, ensuring the longest path ≤ MAX_DEPENDENCY_DEPTH.
fn bounded_depth_dag_strategy(
    min_nodes: usize,
    max_nodes: usize,
) -> impl Strategy<Value = (Vec<TaskId>, Vec<Vec<usize>>)> {
    (min_nodes..=max_nodes).prop_flat_map(move |n| {
        // Assign each node a level from 1 to MAX_DEPENDENCY_DEPTH
        let level_strategies = proptest::collection::vec(1..=MAX_DEPENDENCY_DEPTH, n);

        level_strategies.prop_flat_map(move |levels| {
            // Sort nodes by level to determine valid dependency targets
            // Node i can only depend on nodes j where levels[j] < levels[i]
            let levels_clone = levels.clone();
            let edge_strategies: Vec<_> = (0..n)
                .map(|i| {
                    let my_level = levels_clone[i];
                    if my_level == 1 {
                        // Level 1 nodes have no dependencies
                        Just(vec![]).boxed()
                    } else {
                        // Can depend on any node with a strictly lower level
                        let valid_targets: Vec<usize> = (0..n)
                            .filter(|&j| j != i && levels_clone[j] < my_level)
                            .collect();

                        if valid_targets.is_empty() {
                            Just(vec![]).boxed()
                        } else {
                            // Select a random subset of valid targets (up to 3 to keep it manageable)
                            let max_deps = valid_targets.len().min(3);
                            proptest::collection::vec(
                                proptest::sample::select(valid_targets),
                                0..=max_deps,
                            )
                            .prop_map(|deps| {
                                // Deduplicate
                                let mut unique: Vec<usize> = deps;
                                unique.sort();
                                unique.dedup();
                                unique
                            })
                            .boxed()
                        }
                    }
                })
                .collect();

            edge_strategies.prop_map(move |edges| {
                let ids: Vec<TaskId> = (0..n).map(|_| TaskId::new()).collect();
                (ids, edges)
            })
        })
    })
}

/// Generate a cyclic graph by taking a DAG and adding a back-edge.
/// Returns: (task_ids, edges_as_dep_indices, back_edge: (from_idx, to_idx))
fn cyclic_graph_strategy() -> impl Strategy<Value = (Vec<TaskId>, Vec<Vec<usize>>, (usize, usize))>
{
    // Need at least 2 nodes to form a cycle
    (2usize..=8).prop_flat_map(|n| {
        let edge_strategies: Vec<_> = (0..n)
            .map(|i| {
                if i == 0 {
                    Just(vec![]).boxed()
                } else {
                    // Create a simple chain: each node depends on the previous
                    Just(vec![i - 1]).boxed()
                }
            })
            .collect();

        // The back-edge goes from a lower-indexed node to a higher-indexed node
        // (i.e., node `from` will depend on node `to` where from < to)
        // This creates a cycle: to -> ... -> from -> to
        // We use 0..(n-1) for `from` to ensure (from+1)..n is never empty.
        let back_edge_strategy =
            (0..(n - 1)).prop_flat_map(move |from| ((from + 1)..n).prop_map(move |to| (from, to)));

        (Just(n), edge_strategies, back_edge_strategy).prop_map(|(n, edges, back_edge)| {
            let ids: Vec<TaskId> = (0..n).map(|_| TaskId::new()).collect();
            (ids, edges, back_edge)
        })
    })
}

// ============================================================================
// Helper functions
// ============================================================================

/// Build a DependencyGraph from task IDs and edge indices.
/// Returns Ok(graph) if all tasks were added successfully, or Err with the error.
fn build_graph(ids: &[TaskId], edges: &[Vec<usize>]) -> Result<DependencyGraph, TaskError> {
    let mut graph = DependencyGraph::new();
    let known: HashSet<TaskId> = ids.iter().copied().collect();

    for (i, deps_indices) in edges.iter().enumerate() {
        let deps: Vec<TaskId> = deps_indices.iter().map(|&j| ids[j]).collect();
        graph.add_task(ids[i], &deps, &known)?;
    }

    Ok(graph)
}

// ============================================================================
// Property 6: Task Dependency Cycle Detection
//
// For any directed graph of task dependencies, the Task_Scheduler SHALL accept
// the configuration if and only if the graph is acyclic with maximum depth ≤ 10,
// and SHALL reject configurations containing cycles while identifying the tasks
// that form the cycle.
//
// **Validates: Requirements 3.5, 3.6**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 3.5, 3.6**
    ///
    /// Property 6: Any acyclic graph with depth ≤ 10 SHALL be accepted.
    #[test]
    fn prop_acyclic_graph_within_depth_is_accepted(
        (ids, edges) in bounded_depth_dag_strategy(1, 12)
    ) {
        let result = build_graph(&ids, &edges);

        // A bounded-depth DAG should always be accepted
        prop_assert!(
            result.is_ok(),
            "Acyclic graph with bounded depth should be accepted, got error: {:?}",
            result.err()
        );
    }

    /// **Validates: Requirements 3.5, 3.6**
    ///
    /// Property 6: Any graph containing a cycle SHALL be rejected with
    /// DependencyCycle error identifying cycle members.
    #[test]
    fn prop_cyclic_graph_is_rejected_with_cycle_members(
        (ids, edges, (back_from, back_to)) in cyclic_graph_strategy()
    ) {
        let mut graph = DependencyGraph::new();
        let known: HashSet<TaskId> = ids.iter().copied().collect();

        // Build the chain: 0 <- 1 <- 2 <- ... <- (n-1)
        // But skip the `back_from` node — we'll add it last with the back-edge.
        for (i, deps_indices) in edges.iter().enumerate() {
            if i == back_from {
                continue; // Skip this node, we'll add it with the cycle-creating edge
            }
            let deps: Vec<TaskId> = deps_indices.iter().map(|&j| ids[j]).collect();
            let _ = graph.add_task(ids[i], &deps, &known);
        }

        // Now add the `back_from` node with its original dependencies PLUS
        // a dependency on `back_to`. Since the chain goes back_from <- ... <- back_to
        // (back_to transitively depends on back_from via the chain), adding
        // back_from -> back_to creates a cycle.
        let mut deps_for_back_from: Vec<TaskId> = edges[back_from]
            .iter()
            .map(|&j| ids[j])
            .collect();
        deps_for_back_from.push(ids[back_to]);

        let result = graph.add_task(ids[back_from], &deps_for_back_from, &known);

        prop_assert!(
            matches!(result, Err(TaskError::DependencyCycle { .. })),
            "Graph with cycle should be rejected, got: {:?}",
            result
        );

        if let Err(TaskError::DependencyCycle { cycle_members }) = result {
            prop_assert!(
                !cycle_members.is_empty(),
                "Cycle members should not be empty"
            );
            prop_assert!(
                cycle_members.len() >= 2,
                "A cycle must involve at least 2 tasks, got: {:?}",
                cycle_members
            );
        }
    }

    /// **Validates: Requirements 3.5, 3.6**
    ///
    /// Property 6: Self-dependency (task depends on itself) SHALL always be
    /// rejected as a cycle.
    #[test]
    fn prop_self_dependency_is_always_rejected(
        num_tasks in 1usize..=5
    ) {
        let ids: Vec<TaskId> = (0..num_tasks).map(|_| TaskId::new()).collect();
        let known: HashSet<TaskId> = ids.iter().copied().collect();
        let mut graph = DependencyGraph::new();

        // Add some tasks without deps first
        for i in 0..num_tasks.saturating_sub(1) {
            let _ = graph.add_task(ids[i], &[], &known);
        }

        // Try to add the last task with a self-dependency
        let self_dep_task = ids[num_tasks - 1];
        let result = graph.add_task(self_dep_task, &[self_dep_task], &known);

        prop_assert!(
            matches!(result, Err(TaskError::DependencyCycle { .. })),
            "Self-dependency should be rejected as a cycle, got: {:?}",
            result
        );
    }

    /// **Validates: Requirements 3.5, 3.6**
    ///
    /// Property 6: A dependency chain exceeding depth 10 SHALL be rejected
    /// with DependencyTooDeep error.
    #[test]
    fn prop_deep_chain_exceeding_limit_is_rejected(
        extra_depth in 1usize..=5
    ) {
        let chain_len = MAX_DEPENDENCY_DEPTH + extra_depth;
        let ids: Vec<TaskId> = (0..=chain_len).map(|_| TaskId::new()).collect();
        let known: HashSet<TaskId> = ids.iter().copied().collect();
        let mut graph = DependencyGraph::new();

        // Build a chain of exactly MAX_DEPENDENCY_DEPTH
        graph.add_task(ids[0], &[], &known).unwrap();
        for i in 1..MAX_DEPENDENCY_DEPTH {
            graph.add_task(ids[i], &[ids[i - 1]], &known).unwrap();
        }

        // Now try to extend beyond the limit
        let result = graph.add_task(ids[MAX_DEPENDENCY_DEPTH], &[ids[MAX_DEPENDENCY_DEPTH - 1]], &known);

        prop_assert!(
            matches!(result, Err(TaskError::DependencyTooDeep { .. })),
            "Chain exceeding depth {} should be rejected, got: {:?}",
            MAX_DEPENDENCY_DEPTH,
            result
        );

        if let Err(TaskError::DependencyTooDeep { depth, max_depth }) = result {
            prop_assert_eq!(max_depth, MAX_DEPENDENCY_DEPTH);
            prop_assert!(
                depth > MAX_DEPENDENCY_DEPTH,
                "Reported depth {} should exceed max {}",
                depth,
                MAX_DEPENDENCY_DEPTH
            );
        }
    }

    /// **Validates: Requirements 3.5, 3.6**
    ///
    /// Property 6: A chain of exactly MAX_DEPENDENCY_DEPTH SHALL be accepted.
    #[test]
    fn prop_chain_at_exact_depth_limit_is_accepted(
        _dummy in 0..10u8  // Run multiple times with different random TaskIds
    ) {
        let ids: Vec<TaskId> = (0..MAX_DEPENDENCY_DEPTH).map(|_| TaskId::new()).collect();
        let known: HashSet<TaskId> = ids.iter().copied().collect();
        let mut graph = DependencyGraph::new();

        graph.add_task(ids[0], &[], &known).unwrap();
        for i in 1..MAX_DEPENDENCY_DEPTH {
            let result = graph.add_task(ids[i], &[ids[i - 1]], &known);
            prop_assert!(
                result.is_ok(),
                "Chain of depth {} (≤ {}) should be accepted at step {}, got: {:?}",
                i + 1,
                MAX_DEPENDENCY_DEPTH,
                i,
                result.err()
            );
        }
    }
}

// ============================================================================
// Property 7: Task Dependency Execution Order
//
// For any valid DAG of task dependencies, tasks SHALL execute in an order that
// respects all dependency edges (no task executes before all its dependencies
// have completed successfully).
//
// **Validates: Requirements 3.5**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 3.5**
    ///
    /// Property 7: For any valid DAG, topological_order() SHALL produce an
    /// ordering where every task appears after all of its dependencies.
    #[test]
    fn prop_topological_order_respects_all_edges(
        (ids, edges) in bounded_depth_dag_strategy(2, 15)
    ) {
        let graph = build_graph(&ids, &edges);
        prop_assert!(graph.is_ok(), "Valid DAG should build successfully");
        let graph = graph.unwrap();

        let order = graph.topological_order();

        // Build position map
        let positions: HashMap<TaskId, usize> = order
            .iter()
            .enumerate()
            .map(|(pos, &id)| (id, pos))
            .collect();

        // Verify: for every edge (task depends on dep), dep appears before task
        for (i, deps_indices) in edges.iter().enumerate() {
            let task_id = ids[i];
            if let Some(&task_pos) = positions.get(&task_id) {
                for &dep_idx in deps_indices {
                    let dep_id = ids[dep_idx];
                    if let Some(&dep_pos) = positions.get(&dep_id) {
                        prop_assert!(
                            dep_pos < task_pos,
                            "Dependency {:?} (pos {}) should appear before task {:?} (pos {}) in topological order",
                            dep_id,
                            dep_pos,
                            task_id,
                            task_pos
                        );
                    }
                }
            }
        }
    }

    /// **Validates: Requirements 3.5**
    ///
    /// Property 7: For any valid DAG, topological_order() SHALL include all
    /// tasks that were added to the graph.
    #[test]
    fn prop_topological_order_includes_all_tasks(
        (ids, edges) in bounded_depth_dag_strategy(1, 12)
    ) {
        let graph = build_graph(&ids, &edges);
        prop_assert!(graph.is_ok(), "Valid DAG should build successfully");
        let graph = graph.unwrap();

        let order = graph.topological_order();
        let order_set: HashSet<TaskId> = order.iter().copied().collect();

        for &id in &ids {
            prop_assert!(
                order_set.contains(&id),
                "Task {:?} should appear in topological order",
                id
            );
        }

        prop_assert_eq!(
            order.len(),
            ids.len(),
            "Topological order should contain exactly as many tasks as were added"
        );
    }

    /// **Validates: Requirements 3.5**
    ///
    /// Property 7: mark_completed() SHALL unblock a dependent task if and only
    /// if ALL of its dependencies have been completed.
    #[test]
    fn prop_mark_completed_unblocks_only_when_all_deps_satisfied(
        (ids, edges) in bounded_depth_dag_strategy(3, 10)
    ) {
        let graph = build_graph(&ids, &edges);
        prop_assert!(graph.is_ok(), "Valid DAG should build successfully");
        let mut graph = graph.unwrap();

        // Execute tasks in topological order, verifying unblocking behavior
        let order = graph.topological_order();

        let mut completed: HashSet<TaskId> = HashSet::new();

        for &task_id in &order {
            // Before completing this task, verify it's unblocked
            // (all its deps should already be completed since we follow topo order)
            let deps = edges[ids.iter().position(|&id| id == task_id).unwrap()].clone();
            let dep_ids: Vec<TaskId> = deps.iter().map(|&j| ids[j]).collect();

            let all_deps_completed = dep_ids.iter().all(|dep| completed.contains(dep));
            prop_assert_eq!(
                graph.is_unblocked(&task_id),
                all_deps_completed,
                "Task should be unblocked iff all deps are completed"
            );

            // Mark this task as completed
            let unblocked = graph.mark_completed(task_id);
            completed.insert(task_id);

            // Verify that any task reported as unblocked truly has all deps satisfied
            for &unblocked_id in &unblocked {
                prop_assert!(
                    graph.is_unblocked(&unblocked_id),
                    "Task reported as unblocked should actually be unblocked"
                );

                // Verify all dependencies of the unblocked task are in completed set
                let unblocked_idx = ids.iter().position(|&id| id == unblocked_id).unwrap();
                let unblocked_deps: Vec<TaskId> =
                    edges[unblocked_idx].iter().map(|&j| ids[j]).collect();
                for dep in &unblocked_deps {
                    prop_assert!(
                        completed.contains(dep),
                        "Unblocked task's dependency {:?} should be completed",
                        dep
                    );
                }
            }
        }
    }

    /// **Validates: Requirements 3.5**
    ///
    /// Property 7: A task with multiple dependencies SHALL NOT be unblocked
    /// until ALL dependencies are completed (not just one).
    #[test]
    fn prop_partial_completion_does_not_unblock(
        num_deps in 2usize..=6,
        completion_count in 1usize..=5
    ) {
        // Ensure we don't complete all deps
        let completion_count = completion_count.min(num_deps - 1);

        let deps: Vec<TaskId> = (0..num_deps).map(|_| TaskId::new()).collect();
        let target = TaskId::new();
        let mut all_ids = deps.clone();
        all_ids.push(target);
        let known: HashSet<TaskId> = all_ids.iter().copied().collect();

        let mut graph = DependencyGraph::new();

        // Add all dependency tasks (no deps of their own)
        for &dep_id in &deps {
            graph.add_task(dep_id, &[], &known).unwrap();
        }

        // Add target task depending on all deps
        graph.add_task(target, &deps, &known).unwrap();

        // Complete only some of the dependencies
        for i in 0..completion_count {
            graph.mark_completed(deps[i]);
        }

        // Target should still be blocked
        prop_assert!(
            !graph.is_unblocked(&target),
            "Task with {}/{} deps completed should still be blocked",
            completion_count,
            num_deps
        );

        // Complete the remaining dependencies
        for i in completion_count..num_deps {
            graph.mark_completed(deps[i]);
        }

        // Now target should be unblocked
        prop_assert!(
            graph.is_unblocked(&target),
            "Task with all deps completed should be unblocked"
        );
    }
}
