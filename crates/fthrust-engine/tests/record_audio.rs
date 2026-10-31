//! The recording's audio clock against a simulated clock.

use std::sync::Mutex;

use fthrust_engine::audio::{AudioConfig, AudioProvider, AudioSourceKind};
use fthrust_engine::save::{AudioCursor, RECORD_AUDIO_LAG_NS, RECORD_AUDIO_STEP_MS, pump_audio};
use fthrust_ipc::{AudioStatus, SourceStatus};

const SECOND: i64 = 1_000_000_000;
const RATE: i64 = 48_000;
/// The AAC encoder's priming, in frames.
const PRIMING: i64 = 1024;
/// One AAC frame, the tolerance the recording promises.
const AAC_FRAME: i64 = 1024;

/// A source that serves any window it is asked for, except during a
/// reconnect, and remembers what was asked.
struct ScriptedAudio {
    asked: Mutex<Vec<(i64, u32)>>,
    /// Capture times with no audio (a PipeWire reconnect).
    gap: (i64, i64),
}

impl AudioProvider for ScriptedAudio {
    fn start(&self, _: &AudioConfig) -> Vec<(String, String)> {
        Vec::new()
    }
    fn window(&self, _: AudioSourceKind, end_ns: i64, duration_ms: u32) -> Option<Vec<f32>> {
        self.asked.lock().unwrap().push((end_ns, duration_ms));
        let start_ns = end_ns - i64::from(duration_ms) * 1_000_000;
        if start_ns >= self.gap.0 && end_ns <= self.gap.1 {
            return Some(Vec::new());
        }
        Some(vec![0.25; duration_ms as usize * 48 * 2])
    }
    fn status(&self) -> AudioStatus {
        AudioStatus {
            system: SourceStatus::Streaming,
            microphone: SourceStatus::Disabled,
        }
    }
    fn stop(&self) {}
}

#[test]
fn ten_minutes_of_recording_audio_match_the_video_length() {
    // The record thread wakes every 20-70 ms and pulls audio 200 ms behind
    // the clock in steps of at least 100 ms. Each step used to ask for
    // floor(elapsed ms) but move the cursor by the whole elapsed time,
    // dropping ~0.45% (2.7 s per 10 min); empty windows were skipped, and
    // the audio started when the thread did rather than at the first
    // keyframe.
    let keyframe_ns = 7 * SECOND + 123_456;
    let priming_ns = PRIMING * SECOND / RATE;
    let audio = ScriptedAudio {
        asked: Mutex::new(Vec::new()),
        gap: (keyframe_ns + 200 * SECOND, keyframe_ns + 203 * SECOND),
    };
    let mut cursor = AudioCursor::new(keyframe_ns + priming_ns);
    let mut frames = 0i64;
    let mut write = |_track: usize, pcm: &[f32]| -> Result<(), String> {
        frames += (pcm.len() / 2) as i64;
        Ok(())
    };
    // A deterministic jittery wake-up schedule.
    let mut now = keyframe_ns;
    let mut seed = 0x2545_f491_u64;
    let stop_ns = keyframe_ns + 600 * SECOND;
    while now < stop_ns {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        now += 20_000_000 + (seed >> 33) as i64 % 50_000_000;
        pump_audio(
            &mut cursor,
            &audio,
            &[AudioSourceKind::System],
            now - RECORD_AUDIO_LAG_NS,
            RECORD_AUDIO_STEP_MS,
            &mut write,
        )
        .unwrap();
    }
    // The stop pulls the rest, up to the end of the last frame.
    pump_audio(
        &mut cursor,
        &audio,
        &[AudioSourceKind::System],
        stop_ns,
        1,
        &mut write,
    )
    .unwrap();
    let track = PRIMING + frames;
    let video = (stop_ns - keyframe_ns) * RATE / SECOND;
    assert!(
        (track - video).abs() <= AAC_FRAME,
        "the audio track holds {track} frames, the video {video}"
    );
    let asked = audio.asked.lock().unwrap();
    let (first_end, first_ms) = asked[0];
    assert_eq!(
        first_end - i64::from(first_ms) * 1_000_000,
        keyframe_ns + priming_ns,
        "the first sample is the one captured at the first keyframe, after the priming"
    );
    for pair in asked.windows(2) {
        let (prev_end, _) = pair[0];
        let (end, ms) = pair[1];
        assert_eq!(
            end - i64::from(ms) * 1_000_000,
            prev_end,
            "each window starts where the last ended"
        );
    }
}

#[test]
fn a_reconnect_gap_is_filled_with_silence() {
    let audio = ScriptedAudio {
        asked: Mutex::new(Vec::new()),
        gap: (SECOND, 2 * SECOND),
    };
    let mut cursor = AudioCursor::new(0);
    let mut pcm = Vec::new();
    pump_audio(
        &mut cursor,
        &audio,
        &[AudioSourceKind::System],
        3 * SECOND,
        RECORD_AUDIO_STEP_MS,
        |_, s: &[f32]| -> Result<(), String> {
            pcm.extend_from_slice(s);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(pcm.len(), 3 * 48_000 * 2, "three seconds, gap included");
    assert_eq!(cursor.frames(), 3 * 48_000);
    let at = |s: usize| pcm[s * 48_000 * 2 + 1000];
    assert_eq!(at(0), 0.25);
    assert_eq!(at(1), 0.0, "silence where the source had nothing");
    assert_eq!(at(2), 0.25, "and the audio after it keeps its place");
}

#[test]
fn the_cursor_pulls_whole_milliseconds_and_nothing_early() {
    let mut c = AudioCursor::new(1_000_000_500);
    assert_eq!(c.due(1_000_000_500 + 99_999_999, 100), None);
    assert_eq!(
        c.due(1_000_000_500 + 100_400_000, 100),
        Some((1_100_000_500, 100))
    );
    c.advance(100);
    assert_eq!(c.frames(), 4_800);
    assert_eq!(c.next_ns(), 1_100_000_500);
    // A long catch-up comes in pieces of at most a second.
    assert_eq!(
        c.due(1_100_000_500 + 5 * SECOND, 1),
        Some((2_100_000_500, 1000))
    );
    // Before the start: nothing.
    assert_eq!(AudioCursor::new(SECOND).due(0, 1), None);
}
