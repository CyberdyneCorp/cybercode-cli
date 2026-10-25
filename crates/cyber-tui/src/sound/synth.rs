//! The built-in alert sounds as code (`tui` → Sound packs): each pack is a table of
//! notes per event, rendered to mono 16-bit 44.1 kHz PCM and written as a WAV file.
//!
//! Rendering is deterministic on every platform: phase and time come from the sample
//! index, and sine and exp are computed here from IEEE basic operations instead of the
//! platform's math library, whose last bits differ between systems. The same pack,
//! event, volume and [`SYNTH_VERSION`] therefore give the same bytes everywhere.

use super::{Alert, Pack};

/// Bumped whenever a table or the renderer changes, so cached files are rendered again.
pub const SYNTH_VERSION: u32 = 1;
pub const RATE: u32 = 44_100;
/// The normalised peak: −3 dBFS.
const PEAK: f64 = 0.707_945_784_384_137_9;
/// Silence before the first note and after the last release, so a player that clicks
/// on its first or last sample clicks on silence.
const PAD_MS: f64 = 3.0;
/// Partials of the band-limited waves stop below this, well under Nyquist.
const MAX_PARTIAL_HZ: f64 = 12_000.0;

const TAU: f64 = std::f64::consts::TAU;
const PI: f64 = std::f64::consts::PI;
const LN_2: f64 = std::f64::consts::LN_2;

/// A note's waveform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wave {
    Sine,
    /// A sine with an upper partial at `ratio` × the frequency, `level` loud, that
    /// fades with time constant `decay_ms`: a struck bar.
    Partial {
        ratio: f64,
        level: f64,
        decay_ms: f64,
    },
    /// A sine carrier modulated at `ratio` × its frequency; the modulation index starts
    /// at `index` and fades with time constant `decay_ms`, so the attack is bright and
    /// the tail pure.
    Fm {
        ratio: f64,
        index: f64,
        decay_ms: f64,
    },
    /// A band-limited pulse wave with this duty cycle.
    Square {
        duty: f64,
    },
    /// A band-limited triangle wave.
    Triangle,
}

/// One note: a pitch, when it starts and how long it holds, and its envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// MIDI note number; fractions detune (0.08 is 8 cents sharp).
    pub midi: f64,
    pub start_ms: f64,
    /// From the start to the beginning of the release.
    pub len_ms: f64,
    pub wave: Wave,
    pub gain: f64,
    pub attack_ms: f64,
    /// Time constant of the decay from the attack peak toward `sustain`.
    pub decay_ms: f64,
    pub sustain: f64,
    pub release_ms: f64,
    /// Semitones the pitch moves over the note's length (negative drops).
    pub glide: f64,
}

/// The shortest attack and release the envelope allows: shorter ones click.
const MIN_ATTACK_MS: f64 = 4.0;
const MIN_RELEASE_MS: f64 = 20.0;

impl Note {
    /// A note with the defaults of `wave`: struck bells and bars decay to silence, the
    /// 8-bit waves hold.
    fn new(midi: f64, start_ms: f64, len_ms: f64, wave: Wave) -> Self {
        let held = matches!(wave, Wave::Square { .. } | Wave::Triangle);
        Self {
            midi,
            start_ms,
            len_ms,
            wave,
            gain: 1.0,
            attack_ms: if held { 4.0 } else { 6.0 },
            decay_ms: if held { 90.0 } else { 160.0 },
            sustain: if held { 0.7 } else { 0.0 },
            release_ms: if held { 25.0 } else { 140.0 },
            glide: 0.0,
        }
    }

    fn gain(self, gain: f64) -> Self {
        Self { gain, ..self }
    }

    fn decay(self, decay_ms: f64) -> Self {
        Self { decay_ms, ..self }
    }

    fn release(self, release_ms: f64) -> Self {
        Self { release_ms, ..self }
    }

    fn glide(self, glide: f64) -> Self {
        Self { glide, ..self }
    }

    fn end_ms(&self) -> f64 {
        self.start_ms + self.len_ms + self.release_ms.max(MIN_RELEASE_MS)
    }

    fn hz(&self) -> f64 {
        440.0 * exp((self.midi - 69.0) / 12.0 * LN_2)
    }

    /// The envelope `t` seconds into the note: a raised-cosine attack, an exponential
    /// decay toward the sustain level, then a raised-cosine release to zero.
    fn envelope(&self, t: f64) -> f64 {
        let attack = self.attack_ms.max(MIN_ATTACK_MS) / 1000.0;
        let release = self.release_ms.max(MIN_RELEASE_MS) / 1000.0;
        let len = self.len_ms / 1000.0;
        let held = |t: f64| {
            if t < attack {
                return 0.5 - 0.5 * cos(PI * t / attack);
            }
            let s = self.sustain;
            s + (1.0 - s) * exp(-(t - attack) / (self.decay_ms / 1000.0))
        };
        if t < len {
            held(t)
        } else if t < len + release {
            held(len) * (0.5 + 0.5 * cos(PI * (t - len) / release))
        } else {
            0.0
        }
    }

    /// The phase in radians `t` seconds into the note, with the glide integrated: the
    /// pitch moves exponentially from `hz` to `hz · 2^(glide/12)` over the length.
    fn phase(&self, t: f64) -> f64 {
        let hz = self.hz();
        if self.glide == 0.0 {
            return TAU * hz * t;
        }
        let rate = self.glide / 12.0 * LN_2 / (self.len_ms / 1000.0);
        TAU * hz * (exp(rate * t) - 1.0) / rate
    }

    fn sample(&self, t: f64) -> f64 {
        let theta = self.phase(t);
        let hz = self.hz();
        let tone = match self.wave {
            Wave::Sine => sin(theta),
            Wave::Partial {
                ratio,
                level,
                decay_ms,
            } => sin(theta) + level * exp(-t * 1000.0 / decay_ms) * sin(ratio * theta),
            Wave::Fm {
                ratio,
                index,
                decay_ms,
            } => {
                let index = index.min(fm_index_limit(hz, ratio));
                sin(theta + index * exp(-t * 1000.0 / decay_ms) * sin(ratio * theta))
            }
            Wave::Square { duty } => pulse(theta, hz, duty),
            Wave::Triangle => triangle(theta, hz),
        };
        tone * self.envelope(t) * self.gain
    }
}

/// The largest FM index whose strong sidebands (Carson's rule: up to `index + 1` of
/// them) stay under [`MAX_PARTIAL_HZ`]: a high chime gets a gentler attack instead of
/// sidebands folding back from above Nyquist.
fn fm_index_limit(hz: f64, ratio: f64) -> f64 {
    ((MAX_PARTIAL_HZ / hz - 1.0) / ratio - 1.0).max(0.2)
}

/// A band-limited pulse: its Fourier series up to [`MAX_PARTIAL_HZ`], without DC.
fn pulse(theta: f64, hz: f64, duty: f64) -> f64 {
    let partials = (MAX_PARTIAL_HZ / hz).floor().max(1.0) as u32;
    (1..=partials)
        .map(|n| {
            let n = f64::from(n);
            sin(PI * n * duty) / n * cos(n * theta - PI * n * duty)
        })
        .sum()
}

/// A band-limited triangle: odd partials falling with their square.
fn triangle(theta: f64, hz: f64) -> f64 {
    let partials = (MAX_PARTIAL_HZ / hz).floor().max(1.0) as u32;
    (1..=partials)
        .step_by(2)
        .map(|n| {
            let sign = if n % 4 == 1 { 1.0 } else { -1.0 };
            let n = f64::from(n);
            sign * sin(n * theta) / (n * n)
        })
        .sum()
}

/// `sin` from basic operations: reduced to [−π/2, π/2], then its Taylor series to x¹⁵
/// (error below 1e-10, far under a 16-bit step).
pub(crate) fn sin(x: f64) -> f64 {
    let mut x = x - TAU * (x / TAU).round();
    if x > PI / 2.0 {
        x = PI - x;
    } else if x < -PI / 2.0 {
        x = -PI - x;
    }
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    for k in 1..8 {
        let k = f64::from(k);
        term *= -x2 / ((2.0 * k) * (2.0 * k + 1.0));
        sum += term;
    }
    sum
}

fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

/// `exp` from basic operations: `2^k · e^r` with |r| ≤ ln 2 / 2 and a Taylor series.
pub(crate) fn exp(x: f64) -> f64 {
    if x < -700.0 {
        return 0.0;
    }
    let k = (x / LN_2).round();
    let r = x - k * LN_2;
    let mut term = 1.0;
    let mut sum = 1.0;
    for n in 1..14 {
        term *= r / f64::from(n);
        sum += term;
    }
    sum * f64::from_bits(((k as i64 + 1023) as u64) << 52)
}

const fn fm(index: f64) -> Wave {
    Wave::Fm {
        ratio: 3.5,
        index,
        decay_ms: 150.0,
    }
}

const BAR: Wave = Wave::Partial {
    ratio: 4.0,
    level: 0.3,
    decay_ms: 40.0,
};

const PULSE: Wave = Wave::Square { duty: 0.25 };

/// MIDI note numbers.
mod pitch {
    pub const G3: f64 = 55.0;
    pub const A3: f64 = 57.0;
    pub const C4: f64 = 60.0;
    pub const D4: f64 = 62.0;
    pub const E4: f64 = 64.0;
    pub const G4: f64 = 67.0;
    pub const A4: f64 = 69.0;
    pub const C5: f64 = 72.0;
    pub const E5: f64 = 76.0;
    pub const G5: f64 = 79.0;
    pub const A5: f64 = 81.0;
    pub const C6: f64 = 84.0;
    pub const D6: f64 = 86.0;
    pub const E6: f64 = 88.0;
    pub const G6: f64 = 91.0;
    pub const A6: f64 = 93.0;
    pub const B6: f64 = 95.0;
    pub const C7: f64 = 96.0;
    pub const E7: f64 = 100.0;
}

/// The notes of one pack's sound for `event`; empty for `bell`.
pub fn notes(pack: Pack, event: Alert) -> Vec<Note> {
    match pack {
        Pack::Cyber => cyber(event),
        Pack::Soft => soft(event),
        Pack::Retro => retro(event),
        Pack::Bell => Vec::new(),
    }
}

/// FM chimes: inharmonic at the attack, pure in the tail.
fn cyber(event: Alert) -> Vec<Note> {
    use pitch::*;
    let chime = |midi, start, len| Note::new(midi, start, len, fm(1.8));
    match event {
        // A "question": up a fifth and back.
        Alert::Input => vec![
            chime(E6, 0.0, 90.0).decay(120.0),
            chime(B6, 90.0, 90.0).decay(120.0),
            chime(E6, 180.0, 90.0).decay(160.0),
        ],
        Alert::Done => vec![
            chime(C6, 0.0, 140.0),
            chime(G6, 140.0, 320.0).decay(240.0).release(160.0),
        ],
        // Falling, with a voice 8 cents sharp beating against each note.
        Alert::Error => {
            let dark = |midi, start, len, gain| {
                Note::new(midi, start, len, fm(1.2)).decay(220.0).gain(gain)
            };
            vec![
                dark(A4, 0.0, 170.0, 0.6),
                dark(A4 + 0.08, 0.0, 170.0, 0.4),
                dark(E4, 180.0, 300.0, 0.6),
                dark(E4 + 0.08, 180.0, 300.0, 0.4),
            ]
        }
        Alert::Workflow => vec![
            chime(C6, 0.0, 120.0),
            chime(E6, 70.0, 120.0),
            chime(G6, 140.0, 120.0),
            chime(C7, 210.0, 300.0).decay(240.0).release(160.0),
        ],
        Alert::WorkflowFailed => vec![
            chime(G4, 0.0, 150.0).decay(200.0),
            chime(D4, 160.0, 150.0).decay(200.0),
            chime(A3, 320.0, 260.0).decay(260.0),
        ],
        // A sparkle and its pure echo.
        Alert::Message => vec![
            chime(A6, 0.0, 120.0),
            Note::new(A6, 150.0, 120.0, Wave::Sine).gain(0.35),
        ],
    }
}

/// A marimba: a sine with its fourth partial, struck and left to ring.
fn soft(event: Alert) -> Vec<Note> {
    use pitch::*;
    let bar = |midi, start, len| Note::new(midi, start, len, BAR).decay(180.0).release(80.0);
    match event {
        Alert::Input => vec![bar(G5, 0.0, 200.0), bar(D6, 130.0, 300.0)],
        Alert::Done => vec![
            bar(C5, 0.0, 200.0),
            bar(E5, 110.0, 200.0),
            bar(G5, 220.0, 350.0).decay(240.0),
        ],
        Alert::Error => vec![
            bar(E4, 0.0, 250.0).decay(220.0),
            bar(C4, 220.0, 400.0).decay(260.0),
        ],
        Alert::Workflow => vec![
            bar(C5, 0.0, 200.0),
            bar(G5, 120.0, 200.0),
            bar(C6, 240.0, 400.0).decay(260.0),
        ],
        Alert::WorkflowFailed => vec![
            bar(D4, 0.0, 260.0).decay(220.0),
            bar(A3, 240.0, 450.0).decay(300.0),
        ],
        Alert::Message => vec![bar(A5, 0.0, 350.0).decay(220.0)],
    }
}

/// 8-bit: a 25 % pulse for the melody, a triangle for the low notes.
fn retro(event: Alert) -> Vec<Note> {
    use pitch::*;
    let step = |midi, start, len| Note::new(midi, start, len, PULSE);
    match event {
        Alert::Input => [C6, E6, G6, C7, C6, E6, G6, C7]
            .iter()
            .enumerate()
            .map(|(i, &midi)| step(midi, 40.0 * i as f64, 36.0))
            .collect(),
        Alert::Done => vec![
            step(C5, 0.0, 66.0),
            step(E5, 70.0, 66.0),
            step(G5, 140.0, 66.0),
            step(C6, 210.0, 200.0).decay(160.0),
        ],
        Alert::Error => vec![
            Note::new(C4, 0.0, 150.0, Wave::Triangle),
            Note::new(G3, 160.0, 300.0, Wave::Triangle)
                .glide(-5.0)
                .decay(200.0),
            step(C4, 0.0, 150.0).gain(0.35),
            step(G3, 160.0, 300.0).glide(-5.0).decay(200.0).gain(0.35),
        ],
        Alert::Workflow => vec![
            step(G5, 0.0, 80.0),
            step(C6, 90.0, 80.0),
            step(E6, 180.0, 80.0),
            step(G6, 270.0, 300.0).decay(200.0),
        ],
        Alert::WorkflowFailed => vec![
            step(G4, 0.0, 120.0),
            step(D4, 130.0, 120.0),
            step(A3, 260.0, 300.0).glide(-2.0).decay(200.0),
        ],
        Alert::Message => vec![step(E7, 0.0, 30.0), step(E7, 70.0, 30.0)],
    }
}

/// How loud a pack plays next to the others at the same peak: a square wave carries
/// far more energy than a decaying sine.
fn pack_level(pack: Pack) -> f64 {
    match pack {
        Pack::Retro => 0.85,
        _ => 1.0,
    }
}

/// The mixed samples of `notes`, normalised to −3 dBFS, then scaled by the pack level
/// and `volume` (0–100).
pub fn render(pack: Pack, event: Alert, volume: u8) -> Vec<i16> {
    let notes = notes(pack, event);
    let end_ms = notes.iter().map(Note::end_ms).fold(0.0, f64::max);
    let len = ((end_ms + 2.0 * PAD_MS) / 1000.0 * f64::from(RATE)).ceil() as usize;
    let mut mix = vec![0.0; len];
    let pad = PAD_MS / 1000.0;
    for note in &notes {
        let start = note.start_ms / 1000.0 + pad;
        let first = (start * f64::from(RATE)).ceil() as usize;
        let last = (((note.end_ms() / 1000.0 + pad) * f64::from(RATE)).ceil() as usize).min(len);
        for (i, out) in mix.iter_mut().enumerate().take(last).skip(first) {
            *out += note.sample(i as f64 / f64::from(RATE) - start);
        }
    }
    let peak = mix.iter().fold(0.0_f64, |peak, s| peak.max(s.abs()));
    if peak == 0.0 {
        return vec![0; len];
    }
    let scale = PEAK / peak * pack_level(pack) * f64::from(volume.min(100)) / 100.0;
    mix.iter()
        .map(|s| (s * scale * 32767.0).round() as i16)
        .collect()
}

/// A mono 16-bit PCM WAV file: the 44-byte RIFF header, then the samples.
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    const PACKS: [Pack; 3] = [Pack::Cyber, Pack::Soft, Pack::Retro];

    fn dbfs(sample: f64) -> f64 {
        20.0 * (sample / 32768.0).log10()
    }

    fn peak(samples: &[i16]) -> f64 {
        samples
            .iter()
            .map(|s| f64::from(s.unsigned_abs()))
            .fold(0.0, f64::max)
    }

    #[test]
    fn math_matches_the_platform_closely() {
        for i in -2000..2000 {
            let x = f64::from(i) * 0.0137;
            assert!((sin(x) - x.sin()).abs() < 1e-9, "sin {x}");
            assert!((exp(x / 100.0) - (x / 100.0).exp()).abs() < 1e-9 * (x / 100.0).exp());
        }
        assert_eq!(exp(-800.0), 0.0);
    }

    /// The bytes of every sound at volume 60, pinned: a change to a table or the
    /// renderer must bump `SYNTH_VERSION` and update these.
    #[test]
    fn every_sound_is_pinned() {
        let mut got = Vec::new();
        for pack in PACKS {
            for event in Alert::ALL {
                let digest = Sha256::digest(wav(&render(pack, event, 60)));
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                got.push(format!("{}-{} {hex}", pack.name(), event.name()));
            }
        }
        assert_eq!(
            got.join("\n"),
            PINNED.trim(),
            "SYNTH_VERSION {SYNTH_VERSION}"
        );
    }

    const PINNED: &str = "
cyber-input ee67b667bb876d70353894538b2f446db22a7285863be78f3815b3eca6e186ce
cyber-error 2196d2d4c076947e0235bde30752ec26e175bdb31ed32a6172673e1a3d34cb4a
cyber-workflow_failed 6fe4f1aba3ca7be3253d82f01146858ffe265a6d37ddc4e0e07d6a1699ddee83
cyber-done 147a1006ff342f51b52a97c5a65bad232354ae19e90f627c2a1088143ee2b5e5
cyber-workflow 22167b955fcaa9d0fed45dda7cddfaf590f502cee1e07b7a473aebe8aea59222
cyber-message 23e34c64ef7ce7d7c9a39f4af138b425ead6cfcbe1279e3d894015e540a65f9b
soft-input be446ca7abca72957d7f67f00d9c1afc50f1e337c6db41798df4eda567e0e6ab
soft-error 3d86a8654037c16c12d5e3571414ef23be079076e491b31305c5ca5cac8eee4d
soft-workflow_failed 3f0d5c7dd80930e53ac8f0a5c3637d88e799d36851c9f9f1058330ebc296854c
soft-done 78a3527e46c092f7353d73905fb1fc21e98b08ca0a65da5bd0c0ae968c431d54
soft-workflow 9cf4b72030cba6f60cb011400892e38f454f445159c45ce357c59421ec8734a3
soft-message e0ba1df66014e3e8be9ad13d45fcf22f2254dc4b703b46e61c1b629b10adbcd2
retro-input 2855b6a0299d4b79db43c1d3eda94b57fb5b3f064ad4b2e18e1352e8671b0ea5
retro-error aa3e3b6215816b984c134f30bde162298c3e35d386a4681c5ab30d5a23116204
retro-workflow_failed 6de98b8c9455579d43c650c57c7852af1ce19f40cd59add6073c0e95ca74d7a3
retro-done 4b5fd8959f39f4763c37b288cd5689334853b03a20b649e7c3b98f2248c778ce
retro-workflow fea8b0f8fa19e0ee67306e86a703903075988843c35bb2a4db6e5d9d7b789171
retro-message 49d44cb0665f30b401f6c68888f86e26289779b4411789d573e8b595a17ce88e
";

    /// Scenario "Deterministic, click-free sounds", for every sound: byte-identical
    /// across renders, at most −1 dBFS, silent over the first and last 2 ms, and at
    /// most 0.9 s long.
    #[test]
    fn sounds_are_short_quiet_at_the_edges_and_never_clip() {
        let edge = (RATE as usize) * 2 / 1000;
        for pack in PACKS {
            for event in Alert::ALL {
                let name = format!("{}-{}", pack.name(), event.name());
                let samples = render(pack, event, 100);
                assert_eq!(samples, render(pack, event, 100), "{name}");
                assert!(dbfs(peak(&samples)) <= -1.0, "{name}");
                for part in [&samples[..edge], &samples[samples.len() - edge..]] {
                    assert!(dbfs(peak(part).max(1e-9)) < -40.0, "{name}");
                }
                let seconds = samples.len() as f64 / f64::from(RATE);
                assert!(seconds <= 0.9, "{name}: {seconds}");
            }
        }
    }

    #[test]
    fn each_pack_has_six_distinct_sounds() {
        for pack in PACKS {
            let sounds: Vec<Vec<i16>> = Alert::ALL
                .iter()
                .map(|&event| render(pack, event, 60))
                .collect();
            for (i, a) in sounds.iter().enumerate() {
                for b in &sounds[i + 1..] {
                    assert_ne!(a, b, "{}", pack.name());
                }
            }
        }
        assert!(notes(Pack::Bell, Alert::Done).is_empty());
    }

    #[test]
    fn volume_scales_and_zero_is_silence() {
        let full = peak(&render(Pack::Cyber, Alert::Done, 100));
        let half = peak(&render(Pack::Cyber, Alert::Done, 50));
        assert!((half / full - 0.5).abs() < 0.001, "{half} / {full}");
        assert!(render(Pack::Soft, Alert::Input, 0).iter().all(|&s| s == 0));
    }

    #[test]
    fn the_wav_header_describes_mono_16_bit_44_1_khz() {
        let bytes = wav(&[1, -1, 0]);
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1, "mono");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), RATE);
        assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16, "bits");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(&bytes[44..], [1, 0, 0xff, 0xff, 0, 0]);
    }
}
