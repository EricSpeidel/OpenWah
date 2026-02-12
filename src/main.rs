use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context, Result};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use rodio::{buffer::SamplesBuffer, OutputStream, OutputStreamHandle, Sink, Source};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};

const BASE_MIDI_NOTE: i32 = 60; // C4
const PIANO_START_MIDI: i32 = 48; // C3
const PIANO_END_MIDI: i32 = 84; // C6
const DEFAULT_BITE_MS: u32 = 500;
const MIN_BITE_MS: u32 = 500;
const MAX_BITE_MS: u32 = 5_000;

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
    sample_rate: u32,
    mono_samples: Arc<Vec<f32>>,
}

impl SampleClip {
    fn from_file(path: &Path, duration_ms: u32) -> Result<Self> {
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

        let target_frames = (sample_rate as f32 * duration_ms as f32 / 1_000.0) as usize;
        let mut out_mono: Vec<f32> = Vec::with_capacity(target_frames);

        while out_mono.len() < target_frames {
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
            let channels = decoded.spec().channels.count().max(1);

            let mut sample_buffer =
                SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            sample_buffer.copy_interleaved_ref(decoded);
            let decoded_samples = sample_buffer.samples();

            for frame in decoded_samples.chunks(channels) {
                let mixed = frame.iter().copied().sum::<f32>() / channels as f32;
                out_mono.push(mixed);
                if out_mono.len() >= target_frames {
                    break;
                }
            }
        }

        if out_mono.is_empty() {
            return Err(anyhow!("failed to decode audio samples from selected file"));
        }

        if out_mono.len() < target_frames {
            out_mono.resize(target_frames, 0.0);
        } else {
            out_mono.truncate(target_frames);
        }

        Ok(Self {
            sample_rate,
            mono_samples: Arc::new(out_mono),
        })
    }

    fn generated_test_tone(duration_ms: u32) -> Self {
        let sample_rate = 44_100;
        let target_frames = (sample_rate as f32 * duration_ms as f32 / 1_000.0) as usize;
        let mut out_mono = Vec::with_capacity(target_frames);

        for i in 0..target_frames {
            let t = i as f32 / sample_rate as f32;
            let envelope = (1.0 - t).max(0.0).powf(2.0);
            let fundamental = (2.0 * std::f32::consts::PI * 261.63 * t).sin();
            let overtone = (2.0 * std::f32::consts::PI * 523.25 * t).sin() * 0.35;
            let sub = (2.0 * std::f32::consts::PI * 130.81 * t).sin() * 0.15;
            let sample = (fundamental + overtone + sub) * envelope * 0.6;
            out_mono.push(sample.clamp(-1.0, 1.0));
        }

        Self {
            sample_rate,
            mono_samples: Arc::new(out_mono),
        }
    }
}

struct AudioEngine {
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    current_sink: Mutex<Option<Sink>>,
}

impl AudioEngine {
    fn new() -> Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().context("no default audio output device found")?;
        Ok(Self {
            _stream: Some(stream),
            handle: Some(handle),
            current_sink: Mutex::new(None),
        })
    }

    fn silent_fallback() -> Self {
        Self {
            _stream: None,
            handle: None,
            current_sink: Mutex::new(None),
        }
    }

    fn play_note(&self, clip: &SampleClip, midi_note: i32) -> Result<()> {
        let Some(handle) = &self.handle else {
            return Ok(());
        };

        let ratio = 2.0f32.powf((midi_note - BASE_MIDI_NOTE) as f32 / 12.0);
        let source = SamplesBuffer::new(1, clip.sample_rate, (*clip.mono_samples).clone())
            .speed(ratio)
            .amplify(0.75);

        let sink = Sink::try_new(handle)?;
        sink.append(source);

        let mut active_sink = self
            .current_sink
            .lock()
            .map_err(|_| anyhow!("audio sink lock poisoned"))?;
        if let Some(previous) = active_sink.take() {
            previous.stop();
        }
        *active_sink = Some(sink);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct PianoKey {
    midi: i32,
    is_black: bool,
    x: f32,
    width: f32,
}

struct SamplePianoApp {
    audio: AudioEngine,
    sample: Option<SampleClip>,
    selected_path: Option<PathBuf>,
    status: String,
    bite_ms: u32,
}

impl SamplePianoApp {
    fn new(audio: AudioEngine) -> Self {
        Self {
            audio,
            sample: Some(SampleClip::generated_test_tone(DEFAULT_BITE_MS)),
            selected_path: None,
            status: "Loaded generated 500 ms test tone. Open a file to replace it.".to_string(),
            bite_ms: DEFAULT_BITE_MS,
        }
    }

    fn load_clip(&mut self, path: PathBuf) {
        match SampleClip::from_file(&path, self.bite_ms) {
            Ok(sample) => {
                self.status = format!(
                    "Loaded {} ({} Hz). First {} ms is now mapped across C3–C6.",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("clip"),
                    sample.sample_rate,
                    self.bite_ms,
                );
                self.sample = Some(sample);
                self.selected_path = Some(path);
            }
            Err(err) => {
                self.status = format!("Could not load clip: {err:#}");
            }
        }
    }

    fn refresh_clip_for_duration(&mut self) {
        if let Some(path) = self.selected_path.clone() {
            self.load_clip(path);
        } else {
            self.sample = Some(SampleClip::generated_test_tone(self.bite_ms));
            self.status = format!(
                "Loaded generated {} ms test tone. Open a file to replace it.",
                self.bite_ms
            );
        }
    }

    fn try_play(&mut self, midi_note: i32) {
        if let Some(sample) = &self.sample {
            if let Err(err) = self.audio.play_note(sample, midi_note) {
                self.status = format!("Playback error: {err:#}");
            }
        }
    }

    fn piano_keys() -> Vec<PianoKey> {
        let white_width = 44.0;
        let black_width = 28.0;
        let mut keys = Vec::new();
        let mut white_index = 0;

        for midi in PIANO_START_MIDI..=PIANO_END_MIDI {
            if is_black_key(midi) {
                let x = (white_index as f32 * white_width) - black_width * 0.5;
                keys.push(PianoKey {
                    midi,
                    is_black: true,
                    x,
                    width: black_width,
                });
            } else {
                let x = white_index as f32 * white_width;
                keys.push(PianoKey {
                    midi,
                    is_black: false,
                    x,
                    width: white_width,
                });
                white_index += 1;
            }
        }

        keys
    }

    fn draw_piano(&mut self, ui: &mut egui::Ui) {
        let keys = Self::piano_keys();
        let white_height = 180.0;
        let black_height = 112.0;
        let total_width = keys
            .iter()
            .filter(|k| !k.is_black)
            .map(|k| k.width)
            .sum::<f32>();

        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(total_width, white_height), Sense::hover());
        let painter = ui.painter_at(rect);

        for key in keys.iter().filter(|k| !k.is_black) {
            let key_rect = Rect::from_min_size(
                Pos2::new(rect.left() + key.x, rect.top()),
                Vec2::new(key.width, white_height),
            );
            let response =
                ui.interact(key_rect, egui::Id::new(("white", key.midi)), Sense::click());
            painter.rect_filled(key_rect, 0.0, Color32::WHITE);
            painter.rect_stroke(key_rect, 0.0, Stroke::new(1.0, Color32::BLACK));
            painter.text(
                key_rect.center_bottom() + Vec2::new(0.0, -8.0),
                egui::Align2::CENTER_BOTTOM,
                midi_note_name(key.midi),
                FontId::proportional(12.0),
                Color32::BLACK,
            );
            if response.clicked() {
                self.try_play(key.midi);
            }
        }

        for key in keys.iter().filter(|k| k.is_black) {
            let key_rect = Rect::from_min_size(
                Pos2::new(rect.left() + key.x, rect.top()),
                Vec2::new(key.width, black_height),
            );
            let response =
                ui.interact(key_rect, egui::Id::new(("black", key.midi)), Sense::click());
            painter.rect_filled(key_rect, 2.0, Color32::from_rgb(20, 20, 20));
            painter.text(
                key_rect.center_bottom() + Vec2::new(0.0, -6.0),
                egui::Align2::CENTER_BOTTOM,
                midi_note_name(key.midi),
                FontId::proportional(10.0),
                Color32::WHITE,
            );
            if response.clicked() {
                self.try_play(key.midi);
            }
        }
    }
}

impl eframe::App for SamplePianoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.heading("OpenWah – Soundbite Piano");
            ui.label(
                "1) Set bite duration  2) Load any clip  3) The chosen slice becomes base note (C4).",
            );

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

            let slider_changed = ui
                .add(
                    egui::Slider::new(&mut self.bite_ms, MIN_BITE_MS..=MAX_BITE_MS)
                        .text("Sound bite (ms)"),
                )
                .changed();
            if slider_changed {
                self.refresh_clip_for_duration();
            }

            ui.label(RichText::new(&self.status).color(Color32::LIGHT_BLUE));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.separator();
            ui.label("Piano (C3 → C6)");
            self.draw_piano(ui);

            if self.selected_path.is_none() {
                ui.colored_label(
                    Color32::YELLOW,
                    "Using generated test tone. Load a clip to replace it.",
                );
            }

            ui.add_space(8.0);
            ui.label("Keyboard shortcuts: A W S E D F T G Y H U J K");
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

fn is_black_key(midi: i32) -> bool {
    matches!(midi.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
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
