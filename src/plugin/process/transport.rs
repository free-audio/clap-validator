use crate::cli::tracing::{Recordable, Recorder};
use clap_sys::events::*;
use clap_sys::fixedpoint::*;

/// The current transport state. This can be modified between process calls to simulate
/// transport changes.
#[derive(Debug, Clone, Default)]
pub struct TransportState {
    /// The current sample position.
    pub sample_pos: Option<u64>,

    /// When true, `null` is passed as the transport pointer to the plugin.
    pub is_freerun: bool,

    /// Whether playback is active. Sets [`CLAP_TRANSPORT_IS_PLAYING`] flag.
    pub is_playing: bool,

    /// Whether recording is active. Sets [`CLAP_TRANSPORT_IS_RECORDING`] flag.
    pub is_recording: bool,

    /// Whether the transport is currently within the preroll section. Sets [`CLAP_TRANSPORT_IS_WITHIN_PRE_ROLL`] flag.
    pub is_within_preroll: bool,

    /// Current tempo in BPM and its increment per sample. Sets [`CLAP_TRANSPORT_HAS_TEMPO`] flag.
    pub tempo: Option<(f64, f64)>,

    /// Current time signature as (numerator, denominator). Sets [`CLAP_TRANSPORT_HAS_TIME_SIGNATURE`] flag.
    pub time_signature: Option<(u16, u16)>,

    /// Current position in beats. Sets [`CLAP_TRANSPORT_HAS_BEATS_TIMELINE`] flag.
    pub position_beats: Option<f64>,

    /// Current position in seconds. Sets [`CLAP_TRANSPORT_HAS_SECONDS_TIMELINE`] flag.
    pub position_seconds: Option<f64>,
}

impl TransportState {
    /// Create a dummy transport state with reasonable default values.
    /// Used for most tests as "default" transport state.
    ///
    /// Use [`TransportState::default()`] if you want an "empty" transport state instead.
    pub fn dummy() -> Self {
        TransportState {
            sample_pos: Some(0),
            is_freerun: false,
            is_playing: false,
            is_recording: false,
            is_within_preroll: false,
            tempo: Some((120.0, 0.0)),
            time_signature: Some((4, 4)),
            position_beats: Some(0.0),
            position_seconds: Some(0.0),
        }
    }

    /// Advance the transport state by the given number of samples at the specified sample rate.
    pub fn advance(&mut self, samples: i64, sample_rate: f64) {
        if let Some(sample_pos) = &mut self.sample_pos {
            *sample_pos = sample_pos.saturating_add_signed(samples);
        }

        if self.is_playing
            && let Some(position_seconds) = &mut self.position_seconds
        {
            *position_seconds += samples as f64 / sample_rate;
        }

        if let Some((tempo, tempo_inc)) = &mut self.tempo {
            let tempo_start = *tempo;
            let tempo_end = tempo_start + (*tempo_inc * samples as f64);
            *tempo = tempo_end;

            if self.is_playing
                && let Some(position_beats) = &mut self.position_beats
            {
                // Integrate tempo over the sample block using the trapezoidal rule
                *position_beats += (samples as f64 * (tempo_end + tempo_start) / 60.0 * 0.5) / sample_rate;
            }
        }
    }

    /// Convert the transport state to a CLAP transport event.
    pub fn as_clap_transport(&self, offset: u32) -> clap_event_transport {
        let mut flags = 0;
        flags |= self.is_playing as u32 * CLAP_TRANSPORT_IS_PLAYING;
        flags |= self.is_recording as u32 * CLAP_TRANSPORT_IS_RECORDING;
        flags |= self.is_within_preroll as u32 * CLAP_TRANSPORT_IS_WITHIN_PRE_ROLL;
        flags |= self.position_beats.is_some() as u32 * CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
        flags |= self.position_seconds.is_some() as u32 * CLAP_TRANSPORT_HAS_SECONDS_TIMELINE;
        flags |= self.tempo.is_some() as u32 * CLAP_TRANSPORT_HAS_TEMPO;
        flags |= self.time_signature.is_some() as u32 * CLAP_TRANSPORT_HAS_TIME_SIGNATURE;

        clap_event_transport {
            flags,
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_transport>() as u32,
                time: offset,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_TRANSPORT,
                flags: 0,
            },

            // sending intentional invalid values when the info is not available
            // the plugin **must** check the flags to see what info is valid
            song_pos_beats: self
                .position_beats
                .map(|b| (b * CLAP_BEATTIME_FACTOR as f64).round() as i64)
                .unwrap_or(i64::MIN),
            song_pos_seconds: self
                .position_seconds
                .map(|s| (s * CLAP_SECTIME_FACTOR as f64).round() as i64)
                .unwrap_or(i64::MIN),
            tempo: self.tempo.map(|(t, _)| t).unwrap_or(f64::NAN),
            tempo_inc: self.tempo.map(|(_, ti)| ti).unwrap_or(f64::NAN),
            loop_start_beats: i64::MAX,
            loop_end_beats: i64::MIN,
            loop_start_seconds: i64::MAX,
            loop_end_seconds: i64::MIN,
            bar_start: 0,
            bar_number: 0, // TODO: implement those 2
            tsig_num: self.time_signature.map(|(n, _)| n).unwrap_or(u16::MAX),
            tsig_denom: self.time_signature.map(|(_, d)| d).unwrap_or(0),
        }
    }
}

/// A constant mask for audio processing. Each bit represents whether the corresponding audio channel
/// is constant (1) or not (0).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ConstantMask(pub u64);

impl ConstantMask {
    pub const DYNAMIC: Self = ConstantMask(0);
    pub const CONSTANT: Self = ConstantMask(u64::MAX);

    pub fn with_channel_constant(mut self, channel: u32) -> Self {
        self.0 |= 1u64.unbounded_shl(channel);
        self
    }

    /// Check if the specified channel marked as constant.
    pub fn is_channel_constant(&self, channel: u32) -> bool {
        self.0 & 1u64.unbounded_shl(channel) != 0
    }

    pub fn are_all_channels_constant(&self, n: u32) -> bool {
        let mask = (1u64.unbounded_shl(n)).wrapping_sub(1);
        (self.0 & mask) == mask
    }
}

impl std::fmt::Debug for ConstantMask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConstantMask(0b{:064b})", self.0)
    }
}

impl Recordable for clap_event_transport {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("flags.is_playing", self.flags & CLAP_TRANSPORT_IS_PLAYING != 0);
        record.record("flags.is_recording", self.flags & CLAP_TRANSPORT_IS_RECORDING != 0);
        record.record(
            "flags.is_within_preroll",
            self.flags & CLAP_TRANSPORT_IS_WITHIN_PRE_ROLL != 0,
        );
        record.record("flags.is_loop_active", self.flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE != 0);
        record.record(
            "flags.has_beats_timeline",
            self.flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0,
        );
        record.record(
            "flags.has_seconds_timeline",
            self.flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE != 0,
        );
        record.record(
            "flags.has_time_signature",
            self.flags & CLAP_TRANSPORT_HAS_TIME_SIGNATURE != 0,
        );
        record.record("flags.has_tempo", self.flags & CLAP_TRANSPORT_HAS_TEMPO != 0);

        if self.flags & CLAP_TRANSPORT_HAS_TEMPO != 0 {
            record.record("tempo", self.tempo);
            record.record("tempo_inc", self.tempo_inc);
        }

        if self.flags & CLAP_TRANSPORT_HAS_TIME_SIGNATURE != 0 {
            record.record("time_signature", format_args!("{}/{}", self.tsig_num, self.tsig_denom));
        }

        if self.flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0 {
            record.record("bar_start", self.bar_start);
            record.record("bar_number", self.bar_number);

            record.record(
                "song_pos_beats",
                self.song_pos_beats as f64 / CLAP_BEATTIME_FACTOR as f64,
            );

            if self.flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE != 0 {
                record.record(
                    "loop_start_beats",
                    self.loop_start_beats as f64 / CLAP_BEATTIME_FACTOR as f64,
                );
                record.record(
                    "loop_end_beats",
                    self.loop_end_beats as f64 / CLAP_BEATTIME_FACTOR as f64,
                );
            }
        }

        if self.flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE != 0 {
            record.record(
                "song_pos_seconds",
                self.song_pos_seconds as f64 / CLAP_SECTIME_FACTOR as f64,
            );

            if self.flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE != 0 {
                record.record(
                    "loop_start_seconds",
                    self.loop_start_seconds as f64 / CLAP_SECTIME_FACTOR as f64,
                );
                record.record(
                    "loop_end_seconds",
                    self.loop_end_seconds as f64 / CLAP_SECTIME_FACTOR as f64,
                );
            }
        }
    }
}
