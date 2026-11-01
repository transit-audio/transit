#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn the_table_keeps_forward_edges_once_and_hashes_each_task_s_sources() {
    let g = TaskGraph::new(7, 5, &[(0, 2), (1, 2), (0, 2), (2, 4), (3, 1), (4, 9), (2, 2)]);
    assert_eq!((g.generation(), g.tasks(), g.edges()), (7, 5, 3), "backward, self, out-of-range and repeated edges dropped");
    assert_eq!((0..5).map(|t| g.inputs(t)).collect::<Vec<_>>(), [0, 0, 2, 0, 1]);
    assert_eq!(g.dependents(0), [2]);
    assert_eq!(g.dependents(1), [2]);
    assert_eq!(g.dependents(2), [4]);
    assert!(g.dependents(3).is_empty() && g.dependents(9).is_empty());
    assert_eq!(g.edge_hash(0), g.edge_hash(3), "no inputs, one hash");
    assert_ne!(g.edge_hash(2), g.edge_hash(4), "different sources, different hash");
    let other = TaskGraph::new(8, 5, &[(1, 2), (2, 4)]);
    assert_ne!(g.edge_hash(2), other.edge_hash(2), "a changed edge set changes the identity");
    assert_eq!(g.edge_hash(4), other.edge_hash(4), "an unchanged edge set keeps it");
}

#[test]
fn a_run_hands_out_ready_tasks_and_propagates_a_miss_with_its_origin() {
    // 0 -> 1 -> 2, 0 -> 3, 4 alone.
    let g = TaskGraph::new(1, 5, &[(0, 1), (1, 2), (0, 3)]);
    let mut run = GraphRun::new(8);
    assert!(run.begin(&g));
    assert_eq!(run.next(&g), Some(Step::Ready(0)));
    assert_eq!(run.next(&g), Some(Step::Ready(4)));
    assert_eq!(run.next(&g), None);
    run.succeed(&g, 4);
    assert_eq!(run.next(&g), None, "4 has no dependents");
    run.fail(&g, 0, 0);
    let mut missed = Vec::new();
    while let Some(s) = run.next(&g) {
        missed.push(s);
    }
    missed.sort_by_key(|s| match s {
        Step::Missed { task, .. } | Step::Ready(task) => *task,
    });
    assert_eq!(missed, [Step::Missed { task: 1, origin: 0 }, Step::Missed { task: 2, origin: 0 }, Step::Missed { task: 3, origin: 0 }]);
    assert_eq!(run.state(2), Some(TaskState::Missed { origin: 0 }), "transitively, with the first origin");
    assert!(!GraphRun::new(2).begin(&g), "a table larger than the run is refused");
}

#[test]
fn a_task_waits_for_every_input() {
    let g = TaskGraph::new(1, 3, &[(0, 2), (1, 2)]);
    let mut run = GraphRun::new(3);
    assert!(run.begin(&g));
    while run.next(&g).is_some() {}
    run.succeed(&g, 0);
    assert_eq!(run.next(&g), None, "one of two inputs");
    assert_eq!(run.state(2), Some(TaskState::Waiting));
    run.succeed(&g, 1);
    assert_eq!(run.next(&g), Some(Step::Ready(2)));
    run.fail(&g, 2, 2);
    assert_eq!(run.next(&g), None);
}
