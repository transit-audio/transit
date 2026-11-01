//! #219: the process-input pool covers every admitted source of parameter values.
use super::*;

#[test]
fn the_first_processed_block_has_room_for_all_values_timed_points_and_controls() {
    let count = MAX_PARAMS as usize + 2 * EVENT_CAP;
    let pc = ParamChanges::new(PARAM_QUEUE_CAP, PARAM_POINTS);
    let mut points: Vec<_> = (0..count).map(|i| ParamPoint { time: 0, id: i as u32, class: 0, seq: 0, value: 0.75, release: false }).collect();
    assert_eq!(pc.fill(&mut points, 0), 0, "every admitted ID needs a queue and a slab point");
    for (i, queue) in pc.queues.iter().enumerate().take(count) {
        assert_eq!(queue.id.load(Ordering::Relaxed), i as u32);
        assert_eq!(queue.point(0), Some((0, 0.75)));
    }
}
