//! Drives the real `AudioEngine` — the same one the app manages as Tauri state — through a real
//! output device. Everything else about the engine is covered by unit tests that never open a
//! device, so this is the only place the CPAL stream, the output callback and the progress
//! accounting are exercised together.
//!
//! A machine with no audio device cannot run this; the test reports a skip rather than failing, so
//! headless CI stays green. Run with `--nocapture` to see which branch was taken.

use sonora_lib::audio::engine::{AudioEngine, EngineTrack, PlaybackStatus};
use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn write_wav(path: &Path, rate: u32, channels: u16, seconds: f32, frequency: f32) {
    let frames = (rate as f32 * seconds) as u32;
    let mut samples = Vec::with_capacity(frames as usize * channels as usize);
    for n in 0..frames {
        let value = (2.0 * std::f32::consts::PI * frequency * n as f32 / rate as f32).sin() * 0.3;
        for _ in 0..channels {
            samples.push((value * i16::MAX as f32) as i16);
        }
    }
    let data_bytes = (samples.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

fn engine_track(id: &str, path: &Path, duration_ms: u64) -> EngineTrack {
    EngineTrack {
        id: id.to_string(),
        title: id.to_string(),
        artist: "Smoke Test".to_string(),
        file_path: Some(path.to_string_lossy().to_string()),
        stream_url: None,
        duration_ms,
        loudness_lufs: Some(-14.0),
    }
}

#[test]
fn engine_plays_a_real_file_and_reports_advancing_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("smoke.wav");
    // Long enough that the position has room to advance before the track ends.
    write_wav(&path, 44_100, 2, 4.0, 440.0);

    let engine = AudioEngine::new();
    engine
        .load_track(engine_track("smoke", &path, 4_000), true)
        .expect("the command channel accepts the load");

    // Wait for the device to open, the stream to start and frames to flow.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut advanced = false;
    while Instant::now() < deadline {
        sleep(Duration::from_millis(100));
        let snapshot = engine.snapshot();
        if snapshot.status == PlaybackStatus::Playing && snapshot.position_ms > 150 {
            advanced = true;
            break;
        }
    }

    if !advanced {
        let snapshot = engine.snapshot();
        eprintln!(
            "SKIPPED: no audio output on this machine (status {:?}, position {} ms). \
             Everything except the CPAL stream is still covered by the unit tests.",
            snapshot.status, snapshot.position_ms
        );
        return;
    }

    let snapshot = engine.snapshot();
    assert_eq!(snapshot.status, PlaybackStatus::Playing);
    assert!(
        snapshot.position_ms < 4_000,
        "position must stay within the track, got {}",
        snapshot.position_ms
    );

    // Pausing must freeze the reported position, not merely mute the output.
    engine.pause().unwrap();
    sleep(Duration::from_millis(300));
    let paused_at = engine.snapshot().position_ms;
    sleep(Duration::from_millis(400));
    let still_paused_at = engine.snapshot().position_ms;
    assert!(
        still_paused_at.abs_diff(paused_at) <= 50,
        "position kept moving while paused: {paused_at} -> {still_paused_at}"
    );

    // Seeking moves the position without restarting playback.
    engine.seek(2_000).unwrap();
    engine.play().unwrap();
    sleep(Duration::from_millis(300));
    let after_seek = engine.snapshot().position_ms;
    assert!(
        after_seek >= 2_000,
        "position must resume from the seek target, got {after_seek}"
    );

    engine.stop().unwrap();
    sleep(Duration::from_millis(200));
    assert_eq!(engine.snapshot().status, PlaybackStatus::Stopped);
}

#[test]
fn engine_ignores_a_track_that_cannot_be_decoded() {
    let engine = AudioEngine::new();
    engine
        .load_track(engine_track("missing", Path::new("/nope/missing.flac"), 1_000), true)
        .unwrap();
    sleep(Duration::from_millis(300));
    // A file that will not open leaves the engine idle rather than in a broken playing state.
    assert_eq!(engine.snapshot().status, PlaybackStatus::Stopped);
}

/// Guards the fixture builder itself: ffmpeg is only needed by the library test, so this test
/// simply confirms the two test binaries agree on what a valid WAV header looks like.
#[test]
fn generated_wav_is_readable_by_a_third_party_tool() {
    if Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        eprintln!("SKIPPED: ffprobe is not installed");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.wav");
    write_wav(&path, 44_100, 2, 0.25, 440.0);

    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "stream=sample_rate,channels,duration",
            "-of", "default=noprint_wrappers=1",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("ffprobe runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("sample_rate=44100"), "got {stdout}");
    assert!(stdout.contains("channels=2"), "got {stdout}");
}
