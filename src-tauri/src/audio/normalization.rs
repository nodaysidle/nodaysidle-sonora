//! EBU R128 / ITU-R BS.1770 loudness measurement and gain staging.

use std::collections::VecDeque;

/// Desktop/streaming music target.
pub const TARGET_LUFS: f32 = -14.0;
/// Absolute gate from BS.1770-4: blocks quieter than this are silence, not signal.
const ABSOLUTE_GATE_LUFS: f32 = -70.0;

/// Computes the linear gain multiplier required to bring a track at `measured_lufs`
/// to `target_lufs`. G = 10 ^ ((target - measured) / 20).
pub fn calculate_gain(measured_lufs: f32, target_lufs: f32) -> f32 {
    if !measured_lufs.is_finite() {
        return 1.0;
    }
    let diff_db = target_lufs - measured_lufs;
    // Clamped so a malformed tag cannot produce deafening gain or total silence.
    (10.0f32.powf(diff_db / 20.0)).clamp(0.1, 4.0)
}

/// Applies gain factor to an interleaved slice of 32-bit floating point samples.
pub fn apply_gain_in_place(samples: &mut [f32], gain: f32) {
    if (gain - 1.0).abs() < f32::EPSILON {
        return;
    }
    for sample in samples.iter_mut() {
        *sample = *sample * gain;
    }
}

/// Direct-form-I biquad, used for the two BS.1770 K-weighting stages.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Stage 1: the shelving pre-filter that models the head's acoustic response.
fn k_weighting_prefilter(sample_rate: f32) -> Biquad {
    const F0: f32 = 1681.974_450_955_533;
    const G: f32 = 3.999_843_853_973_347;
    const Q: f32 = 0.707_175_236_955_419_6;

    let k = (std::f32::consts::PI * F0 / sample_rate).tan();
    let vh = 10.0f32.powf(G / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / Q + k * k;
    Biquad {
        b0: (vh + vb * k / Q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / Q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / Q + k * k) / a0,
        ..Default::default()
    }
}

/// Stage 2: the RLB high-pass that removes sub-audible rumble from the measurement.
fn k_weighting_highpass(sample_rate: f32) -> Biquad {
    const F0: f32 = 38.135_470_876_024_44;
    const Q: f32 = 0.500_327_037_323_877_3;

    let k = (std::f32::consts::PI * F0 / sample_rate).tan();
    let denom = 1.0 + k / Q + k * k;
    Biquad {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / denom,
        a2: (1.0 - k / Q + k * k) / denom,
        ..Default::default()
    }
}

/// Streaming loudness estimate. Maintains overlapping 400 ms K-weighted blocks and reports the
/// gated mean of the most recent window.
///
/// ponytail: BS.1770 integrated loudness is defined over a whole programme with a relative gate, so
/// it cannot be produced while the programme is still playing. This keeps a rolling ~3 s window,
/// which is what adaptive per-track gain actually needs. Swap in the full `ebur128` crate if a
/// scan-time "true" integrated value is ever wanted.
#[derive(Clone)]
pub struct LoudnessMeter {
    pre: Vec<Biquad>,
    rlb: Vec<Biquad>,
    channels: usize,
    frames_per_block: usize,
    frame_in_block: usize,
    sum_squares: Vec<f64>,
    window: VecDeque<f32>,
    window_blocks: usize,
}

impl LoudnessMeter {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            pre: vec![k_weighting_prefilter(sample_rate as f32); channels],
            rlb: vec![k_weighting_highpass(sample_rate as f32); channels],
            channels,
            // 400 ms blocks, the BS.1770 gating granularity.
            frames_per_block: ((sample_rate as f64 * 0.4) as usize).max(1),
            frame_in_block: 0,
            sum_squares: vec![0.0; channels],
            window: VecDeque::with_capacity(16),
            window_blocks: 8,
        }
    }

    pub fn reset(&mut self) {
        for b in self.pre.iter_mut().chain(self.rlb.iter_mut()) {
            b.reset();
        }
        self.frame_in_block = 0;
        self.sum_squares.iter_mut().for_each(|s| *s = 0.0);
        self.window.clear();
    }

    /// Feeds interleaved samples. Any trailing partial frame is ignored.
    pub fn push_interleaved(&mut self, samples: &[f32]) {
        let frames = samples.len() / self.channels;
        for frame in 0..frames {
            for ch in 0..self.channels {
                let x = samples[frame * self.channels + ch];
                let weighted = self.rlb[ch].process(self.pre[ch].process(x));
                self.sum_squares[ch] += (weighted as f64) * (weighted as f64);
            }
            self.frame_in_block += 1;
            if self.frame_in_block >= self.frames_per_block {
                self.close_block();
            }
        }
    }

    fn close_block(&mut self) {
        // BS.1770 channel weights are 1.0 for L/R and 1.41 for surround; stereo music only ever
        // hits the L/R case, so the weighted sum is the plain mean-square sum.
        let mean_square: f64 = self.sum_squares.iter().sum::<f64>() / self.frames_per_block as f64;
        if mean_square > 0.0 {
            let lufs = (-0.691 + 10.0 * mean_square.log10()) as f32;
            if lufs > ABSOLUTE_GATE_LUFS {
                self.window.push_back(lufs);
                if self.window.len() > self.window_blocks {
                    self.window.pop_front();
                }
            }
        }
        self.sum_squares.iter_mut().for_each(|s| *s = 0.0);
        self.frame_in_block = 0;
    }

    /// Loudness of the recent window, or `None` while too few blocks have accumulated to trust it.
    pub fn measured_lufs(&self) -> Option<f32> {
        if self.window.len() < 2 {
            return None;
        }
        let mean_energy: f64 = self
            .window
            .iter()
            .map(|l| 10f64.powf((*l as f64 + 0.691) / 10.0))
            .sum::<f64>()
            / self.window.len() as f64;
        Some((-0.691 + 10.0 * mean_energy.log10()) as f32)
    }
}

/// Smoothly ramps toward a target gain and guarantees the output never clips.
#[derive(Clone)]
pub struct GainStage {
    target: f32,
    current: f32,
    ramp_per_frame: f32,
    meter: Option<LoudnessMeter>,
    channels: usize,
    /// Frames remaining before an unmeasured track is re-evaluated from the meter.
    reevaluate_in: usize,
}

/// A 200 ms ramp is long enough to be inaudible as a fade and short enough not to feel laggy.
const RAMP_MS: f32 = 200.0;
/// Unmeasured tracks re-derive their gain no more than twice a second.
const REEVALUATE_FRAMES: usize = 24_000;

impl GainStage {
    pub fn new(channels: usize) -> Self {
        Self {
            target: 1.0,
            current: 1.0,
            ramp_per_frame: 0.0,
            meter: None,
            channels: channels.max(1),
            reevaluate_in: REEVALUATE_FRAMES,
        }
    }

    /// Starts a track. `known_lufs` comes from ReplayGain tags or a previous scan.
    pub fn begin_track(&mut self, known_lufs: Option<f32>, enabled: bool, sample_rate: u32) {
        self.meter = if enabled && known_lufs.is_none() {
            Some(LoudnessMeter::new(sample_rate, self.channels))
        } else {
            None
        };
        self.reevaluate_in = REEVALUATE_FRAMES;
        match known_lufs.filter(|v| v.is_finite()) {
            Some(lufs) if enabled => {
                self.set_target(calculate_gain(lufs, TARGET_LUFS), sample_rate)
            }
            _ => self.set_target(1.0, sample_rate),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool, sample_rate: u32) {
        if !enabled {
            self.meter = None;
            self.set_target(1.0, sample_rate);
        }
    }

    fn set_target(&mut self, target: f32, sample_rate: u32) {
        self.target = target.clamp(0.1, 4.0);
        let frames = (sample_rate as f32 * RAMP_MS / 1000.0).max(1.0);
        self.ramp_per_frame = (self.target - self.current) / frames;
    }

    pub fn is_ramping(&self) -> bool {
        (self.current - self.target).abs() > 1e-4
    }

    /// Applies the ramp and a peak-safe limiter to one interleaved chunk in place.
    pub fn process(&mut self, samples: &mut [f32], sample_rate: u32) {
        if let Some(meter) = self.meter.as_mut() {
            meter.push_interleaved(samples);
            let frames = samples.len() / self.channels;
            self.reevaluate_in = self.reevaluate_in.saturating_sub(frames);
            if self.reevaluate_in == 0 {
                self.reevaluate_in = REEVALUATE_FRAMES;
                if let Some(lufs) = meter.measured_lufs() {
                    self.set_target(calculate_gain(lufs, TARGET_LUFS), sample_rate);
                }
            }
        }

        if (self.current - 1.0).abs() < 1e-4 && !self.is_ramping() {
            return;
        }

        let channels = self.channels;
        let frames = samples.len() / channels;
        let mut peak = 0.0f32;
        for frame in 0..frames {
            let gain = self.current;
            for ch in 0..channels {
                let index = frame * channels + ch;
                let value = samples[index] * gain;
                samples[index] = value;
                peak = peak.max(value.abs());
            }
            self.current += self.ramp_per_frame;
            if (self.ramp_per_frame > 0.0 && self.current > self.target)
                || (self.ramp_per_frame < 0.0 && self.current < self.target)
            {
                self.current = self.target;
            }
        }

        // True-peak guard: if the ramped output would clip, back the whole chunk off by the
        // overshoot instead of hard-clipping individual samples. Recovery is on the next ramp.
        if peak > 1.0 {
            let scale = 1.0 / peak;
            for sample in samples.iter_mut() {
                *sample *= scale;
            }
            self.current *= scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_moves_a_quiet_track_up_and_a_loud_track_down() {
        assert!(
            calculate_gain(-20.0, TARGET_LUFS) > 1.0,
            "quiet track needs gain"
        );
        assert!(
            calculate_gain(-8.0, TARGET_LUFS) < 1.0,
            "loud track needs attenuation"
        );
        // A track already at target is left alone.
        assert!((calculate_gain(TARGET_LUFS, TARGET_LUFS) - 1.0).abs() < 1e-6);
        // Malformed input must not produce a destructive gain.
        assert_eq!(calculate_gain(f32::NAN, TARGET_LUFS), 1.0);
        assert_eq!(calculate_gain(-1000.0, TARGET_LUFS), 4.0);
    }

    /// BS.1770's own reference anchor: a 997 Hz sine at full scale in a *single* channel reads
    /// -3.01 LKFS. This is the published figure the standard is calibrated against, so it pins
    /// down the filter coefficients independently of anything else in this file.
    #[test]
    fn meter_matches_the_bs1770_reference_tone() {
        let rate = 48_000u32;
        let mut meter = LoudnessMeter::new(rate, 1);
        let samples: Vec<f32> = (0..rate as usize * 3)
            .map(|n| (2.0 * std::f32::consts::PI * 997.0 * n as f32 / rate as f32).sin())
            .collect();
        meter.push_interleaved(&samples);

        let lufs = meter.measured_lufs().expect("3 s of tone yields a reading");
        assert!(
            (lufs - (-3.01)).abs() < 0.35,
            "expected the -3.01 LKFS reference tone, measured {lufs}"
        );
    }

    /// The same tone in both channels of a stereo pair sums to +3 LU, and a half-scale tone is
    /// 6 dB quieter — the meter has to reproduce both relationships.
    #[test]
    fn meter_scales_with_channel_count_and_amplitude() {
        let rate = 48_000u32;
        let tone = |amplitude: f32, channels: usize| {
            let mut meter = LoudnessMeter::new(rate, channels);
            let mut samples = Vec::new();
            for n in 0..rate as usize * 3 {
                let v =
                    (2.0 * std::f32::consts::PI * 997.0 * n as f32 / rate as f32).sin() * amplitude;
                for _ in 0..channels {
                    samples.push(v);
                }
            }
            meter.push_interleaved(&samples);
            meter.measured_lufs().expect("a reading")
        };

        let mono = tone(1.0, 1);
        let stereo = tone(1.0, 2);
        assert!(
            (stereo - mono - 3.01).abs() < 0.35,
            "stereo pair should read +3 LU over mono: {mono} vs {stereo}"
        );

        let quiet = tone(0.5, 2);
        assert!(
            (stereo - quiet - 6.02).abs() < 0.35,
            "halving amplitude should drop 6 dB: {stereo} vs {quiet}"
        );
    }

    #[test]
    fn meter_ignores_silence() {
        let mut meter = LoudnessMeter::new(48_000, 2);
        meter.push_interleaved(&vec![0.0; 48_000 * 2 * 2]);
        assert_eq!(meter.measured_lufs(), None);
    }

    #[test]
    fn gain_stage_limits_instead_of_clipping() {
        let mut stage = GainStage::new(2);
        stage.begin_track(Some(-30.0), true, 48_000);
        // A near-full-scale signal with a +16 dB target would clamp hard without the limiter.
        let mut samples = vec![0.9f32; 48_000];
        stage.process(&mut samples, 48_000);
        assert!(
            samples.iter().all(|s| s.abs() <= 1.0),
            "limiter must keep every sample in range"
        );
        assert!(
            samples.iter().any(|s| s.abs() > 0.5),
            "limiter must not collapse the signal to silence"
        );
    }

    #[test]
    fn gain_stage_ramps_rather_than_jumping() {
        let mut stage = GainStage::new(2);
        stage.begin_track(Some(-24.0), true, 48_000);
        let mut samples = vec![0.001f32; 20];
        stage.process(&mut samples, 48_000);
        // First chunk is only 20 frames, far shorter than the 200 ms ramp, so the applied gain is
        // still close to unity rather than the full +10 dB target.
        let applied = samples[0] / 0.001;
        assert!(applied < 1.2, "gain jumped instantly: {applied}");
        assert!(stage.is_ramping(), "ramp should still be in flight");
    }

    #[test]
    fn disabled_normalization_passes_audio_through_untouched() {
        let mut stage = GainStage::new(2);
        stage.begin_track(Some(-30.0), false, 48_000);
        let original = vec![0.25f32; 64];
        let mut samples = original.clone();
        stage.process(&mut samples, 48_000);
        assert_eq!(samples, original);
    }
}
