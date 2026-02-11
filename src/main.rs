use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use rodio::{buffer::SamplesBuffer, OutputStream, OutputStreamHandle, Sink, Source};
use symphonia::core::{
    audio::{AudioBufferRef, SampleBuffer, Signal},
    codecs::DecoderOptions,
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

const BASE_MIDI_NOTE: i32 = 60; // C4
const PIANO_START_MIDI: i32 = 48; // C3
const PIANO_END_MIDI: i32 = 72; // C5
const BASE_NOTE_SECONDS: f32 = 1.0;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "OpenWah - Sample Piano",
        options,
        Box::new(|_cc| {
            let audio = AudioEngine::new().unwrap_or_else(|err| {
                eprintln!("audio initialization failed: {err:#}");
                AudioEngine::silent_fallback()
            });
            Ok(Box::new(SamplePianoApp::new(audio)))
        }),
    )
}

struct SampleClip {
    channels: u16,
    sample_rate: u32,
    samples: Arc<Vec<f32>>,
}

impl SampleClip {
    fn from_file(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("failed to open selected file: {}", path.display()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|x| x.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?;

        let mut format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| anyhow!("no playable audio track found"))?;

        let codec_params = &track.codec_params;
        let mut decoder =
            symphonia::default::get_codecs().make(codec_params, &DecoderOptions::default())?;

        let mut sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| anyhow!("audio file missing sample rate"))?;
        let mut channels = codec_params
            .channels
            .ok_or_else(|| anyhow!("audio file missing channel information"))?
            .count() as u16;

        let target_frames = (sample_rate as f32 * BASE_NOTE_SECONDS) as usize;
        let mut out: Vec<f32> = Vec::with_capacity(target_frames * channels as usize);

        while out.len() / (channels as usize) < target_frames {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(_)) => break,
                Err(err) => return Err(err.into()),
            };

            let decoded = match decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(err) => return Err(err.into()),
            };

            sample_rate = decoded.spec().rate;
            channels = decoded.spec().channels.count() as u16;

            let mut sample_buffer =
                SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            sample_buffer.copy_interleaved_ref(decoded);
            let decoded_samples = sample_buffer.samples();

            let frames_left = target_frames.saturating_sub(out.len() / channels as usize);
            if frames_left == 0 {
                break;
            }
            let to_take = (frames_left * channels as usize).min(decoded_samples.len());
            out.extend_from_slice(&decoded_samples[..to_take]);
        }

        if out.is_empty() {
            return Err(anyhow!("failed to decode audio samples from selected file"));
        }

        let required_len = target_frames * channels as usize;
        if out.len() < required_len {
            out.resize(required_len, 0.0);
        } else {
            out.truncate(required_len);
        }

        Ok(Self {
            channels,
            sample_rate,
            samples: Arc::new(out),
        })
    }
}

struct AudioEngine {
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
}

impl AudioEngine {
    fn new() -> Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().context("no default audio output device found")?;
        Ok(Self {
            _stream: Some(stream),
            handle: Some(handle),
        })
    }

    fn silent_fallback() -> Self {
        Self {
            _stream: None,
            handle: None,
        }
    }

    fn play_note(&self, clip: &SampleClip, midi_note: i32) -> Result<()> {
        let Some(handle) = &self.handle else {
            return Ok(());
        };

        let ratio = 2.0f32.powf((midi_note - BASE_MIDI_NOTE) as f32 / 12.0);
        let source = SamplesBuffer::new(clip.channels, clip.sample_rate, (*clip.samples).clone())
            .speed(ratio)
            .amplify(0.7);

        let sink = Sink::try_new(handle)?;
        sink.append(source);
        sink.detach();
        Ok(())
    }
}

struct SamplePianoApp {
    audio: AudioEngine,
    sample: Option<SampleClip>,
    selected_path: Option<PathBuf>,
    status: String,
}

impl SamplePianoApp {
    fn new(audio: AudioEngine) -> Self {
        Self {
            audio,
            sample: None,
            selected_path: None,
            status: "Load any sound clip to build your 1-second base note.".to_string(),
        }
    }

    fn load_clip(&mut self, path: PathBuf) {
        match SampleClip::from_file(&path) {
            Ok(sample) => {
                self.status = format!(
                    "Loaded {} ({} Hz, {} channel(s)). First second is now mapped across the keyboard.",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("clip"),
                    sample.sample_rate,
                    sample.channels
                );
                self.sample = Some(sample);
                self.selected_path = Some(path);
            }
            Err(err) => {
                self.status = format!("Could not load clip: {err:#}");
            }
        }
    }

    fn try_play(&mut self, midi_note: i32) {
        if let Some(sample) = &self.sample {
            if let Err(err) = self.audio.play_note(sample, midi_note) {
                self.status = format!("Playback error: {err:#}");
            }
        }
    }
}

impl eframe::App for SamplePianoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.heading("OpenWah – Soundbite Piano");
            ui.label("1) Load any clip  2) We trim/use ~1 second as base note (C4)  3) Click keys to play.");

            ui.horizontal(|ui| {
                if ui.button("Open Sound Clip...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        self.load_clip(path);
                    }
                }
                if let Some(path) = &self.selected_path {
                    ui.label(format!("Current: {}", path.display()));
                }
            });

            ui.label(RichText::new(&self.status).color(Color32::LIGHT_BLUE));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.separator();
            ui.label("Piano (C3 to C5)");

            let white_keys = [0, 2, 4, 5, 7, 9, 11];
            let key_width = 50.0;
            let key_height = 180.0;

            ui.horizontal(|ui| {
                for octave in 0..3 {
                    for semitone in white_keys {
                        let midi = PIANO_START_MIDI + octave * 12 + semitone;
                        if midi > PIANO_END_MIDI {
                            continue;
                        }
                        let note_name = midi_note_name(midi);
                        let button = egui::Button::new(note_name)
                            .fill(Color32::WHITE)
                            .stroke(egui::Stroke::new(1.0, Color32::BLACK))
                            .min_size(Vec2::new(key_width, key_height));
                        if ui.add(button).clicked() {
                            self.try_play(midi);
                        }
                    }
                }
            });

            ui.add_space(8.0);
            ui.label("Black keys");
            let black_offsets = [1, 3, 6, 8, 10];
            ui.horizontal(|ui| {
                for octave in 0..3 {
                    for semitone in black_offsets {
                        let midi = PIANO_START_MIDI + octave * 12 + semitone;
                        if midi > PIANO_END_MIDI {
                            continue;
                        }
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(32.0, 120.0), Sense::click());
                        ui.painter().rect_filled(rect, 2.0, Color32::BLACK);
                        ui.painter().text(
                            rect.center_bottom() + Vec2::new(0.0, -8.0),
                            egui::Align2::CENTER_BOTTOM,
                            midi_note_name(midi),
                            egui::TextStyle::Small.resolve(ui.style()),
                            Color32::WHITE,
                        );
                        if response.clicked() {
                            self.try_play(midi);
                        }
                    }
                }
            });

            if self.sample.is_none() {
                ui.colored_label(Color32::YELLOW, "Load a clip to enable sound.");
            }
        });

        for (key, midi) in [
            (egui::Key::A, 60),
            (egui::Key::W, 61),
            (egui::Key::S, 62),
            (egui::Key::E, 63),
            (egui::Key::D, 64),
            (egui::Key::F, 65),
            (egui::Key::T, 66),
            (egui::Key::G, 67),
            (egui::Key::Y, 68),
            (egui::Key::H, 69),
            (egui::Key::U, 70),
            (egui::Key::J, 71),
            (egui::Key::K, 72),
        ] {
            if ctx.input(|i| i.key_pressed(key)) {
                self.try_play(midi);
            }
        }
    }
}

fn midi_note_name(midi: i32) -> String {
    let note = match midi.rem_euclid(12) {
        0 => "C",
        1 => "C#",
        2 => "D",
        3 => "D#",
        4 => "E",
        5 => "F",
        6 => "F#",
        7 => "G",
        8 => "G#",
        9 => "A",
        10 => "A#",
        _ => "B",
    };
    let octave = midi / 12 - 1;
    format!("{note}{octave}")
}
