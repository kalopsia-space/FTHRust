use std::time::{Duration, Instant};

use fthrust_wayland::backend::repeat_deadline;

const PERIOD: Duration = Duration::from_micros(16_667);

#[test]
fn a_fresh_frame_due_within_a_period_is_not_lost_to_a_repeat() {
    // The engine asks for the next frame when it needs one, about a period
    // after the last fresh frame arrived. A source running at the capture
    // rate delivers within a period of that call, with some jitter.
    let fresh_at = Instant::now();
    for call_after in [Duration::ZERO, PERIOD / 2, PERIOD, PERIOD * 3] {
        let call = fresh_at + call_after;
        let deadline = repeat_deadline(call, fresh_at, true, PERIOD);
        assert!(
            deadline >= call + PERIOD + PERIOD / 4,
            "called {call_after:?} after a fresh frame: repeats {:?} after the call",
            deadline - call
        );
    }
}

#[test]
fn repeats_of_a_static_screen_keep_the_frame_rate() {
    // After a repeat, the next one is due a period after it, however late
    // the call comes inside that period.
    let repeated_at = Instant::now();
    for call_after in [Duration::ZERO, PERIOD / 2] {
        let deadline = repeat_deadline(repeated_at + call_after, repeated_at, false, PERIOD);
        assert_eq!(deadline, repeated_at + PERIOD);
    }
    // A call that comes after the next repeat was due still waits a little.
    let late = repeated_at + PERIOD * 2;
    assert_eq!(
        repeat_deadline(late, repeated_at, false, PERIOD),
        late + Duration::from_millis(1)
    );
}
