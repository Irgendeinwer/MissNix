mod sinks;

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use missnix_core::{AppPaths, Database, SyncEngine, is_deezer_preview_url_expired};

use self::sinks::{PcmSink, open_best_audio_sink};

#[derive(Debug, Clone)]
pub struct PreviewRequest {
    pub release_id: i64,
    pub disc_number: i32,
    pub track_number: i32,
    pub track_title: String,
    pub artist_name: String,
    pub release_title: String,
    pub preview_url: String,
}

#[derive(Debug, Clone)]
pub enum PreviewCommand {
    Play(PreviewRequest),
    TogglePause,
    Seek(f32),
    Stop,
}

#[derive(Debug, Clone)]
pub struct PreviewUiUpdate {
    pub active: bool,
    pub playing: bool,
    pub buffering: bool,
    pub release_id: i32,
    pub disc_number: i32,
    pub track_number: i32,
    pub track_title: String,
    pub artist_name: String,
    pub release_title: String,
    pub progress: f32,
    pub elapsed_label: String,
    pub duration_label: String,
    pub level: f32,
    pub error: Option<String>,
}

impl Default for PreviewUiUpdate {
    fn default() -> Self {
        Self {
            active: false,
            playing: false,
            buffering: false,
            release_id: 0,
            disc_number: 0,
            track_number: 0,
            track_title: String::new(),
            artist_name: String::new(),
            release_title: String::new(),
            progress: 0.0,
            elapsed_label: "0:00".to_string(),
            duration_label: "0:30".to_string(),
            level: 0.0,
            error: None,
        }
    }
}

#[derive(Clone)]
pub struct AudioPreviewHandle {
    tx: mpsc::Sender<PreviewCommand>,
    volume_pct: Arc<AtomicU8>,
    generation: Arc<AtomicU64>,
}

impl AudioPreviewHandle {
    pub fn spawn<F>(paths: AppPaths, engine: SyncEngine, initial_volume: u8, on_update: F) -> Self
    where
        F: Fn(PreviewUiUpdate) + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel::<PreviewCommand>();
        let volume_pct = Arc::new(AtomicU8::new(initial_volume.min(100)));
        let generation = Arc::new(AtomicU64::new(0));

        let vol_worker = volume_pct.clone();
        let gen_worker = generation.clone();
        let callback = Arc::new(on_update);

        thread::spawn(move || {
            run_audio_worker(paths, engine, rx, vol_worker, gen_worker, callback);
        });

        Self {
            tx,
            volume_pct,
            generation,
        }
    }

    pub fn play(&self, req: PreviewRequest) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(PreviewCommand::Play(req));
    }

    pub fn toggle_pause(&self) {
        let _ = self.tx.send(PreviewCommand::TogglePause);
    }

    pub fn seek(&self, fraction: f32) {
        let _ = self.tx.send(PreviewCommand::Seek(fraction.clamp(0.0, 1.0)));
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(PreviewCommand::Stop);
    }

    pub fn set_volume(&self, pct: u8) {
        self.volume_pct.store(pct.min(100), Ordering::Relaxed);
    }
}

struct DecodedPcm {
    sample_rate: u32,
    channels: u8,
    samples: Vec<f32>,
}

impl DecodedPcm {
    fn total_frames(&self) -> usize {
        let ch = usize::from(self.channels.max(1));
        self.samples.len() / ch
    }

    fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 30.0;
        }
        (self.total_frames() as f32) / (self.sample_rate as f32)
    }
}

struct ActiveTrackSession {
    req: PreviewRequest,
    pcm: DecodedPcm,
    cursor_frame: usize,
    paused: bool,
    sink: Box<dyn PcmSink>,
    last_ui_emit: Instant,
    smoothed_level: f32,
}

fn run_audio_worker<F>(
    paths: AppPaths,
    engine: SyncEngine,
    rx: mpsc::Receiver<PreviewCommand>,
    volume_pct: Arc<AtomicU8>,
    generation: Arc<AtomicU64>,
    on_update: Arc<F>,
) where
    F: Fn(PreviewUiUpdate) + Send + Sync + 'static,
{
    let mut session: Option<ActiveTrackSession> = None;

    loop {
        // When idle or paused, block until the next command arrives; when actively playing,
        // poll non-blocking between ~23ms PCM chunks (`1024` frames @ 44.1kHz).
        let next_cmd = match session.as_ref() {
            Some(s) if !s.paused => rx.try_recv().ok(),
            _ => match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break,
            },
        };

        if let Some(cmd) = next_cmd {
            match cmd {
                PreviewCommand::Play(req) => {
                    // If clicking the exact same track that's already loaded, toggle pause/resume!
                    if let Some(ref mut cur) = session
                        && cur.req.release_id == req.release_id
                        && cur.req.disc_number == req.disc_number
                        && cur.req.track_number == req.track_number
                    {
                        cur.paused = !cur.paused;
                        if cur.paused {
                            cur.sink.flush();
                            cur.smoothed_level = 0.0;
                        }
                        emit_session_update(cur, &on_update);
                        continue;
                    }

                    // Drop previous audio sink immediately
                    session = None;
                    let start_gen = generation.load(Ordering::SeqCst);

                    on_update(PreviewUiUpdate {
                        active: true,
                        playing: false,
                        buffering: true,
                        release_id: req.release_id as i32,
                        disc_number: req.disc_number,
                        track_number: req.track_number,
                        track_title: req.track_title.clone(),
                        artist_name: req.artist_name.clone(),
                        release_title: req.release_title.clone(),
                        progress: 0.0,
                        elapsed_label: "0:00".to_string(),
                        duration_label: "0:30".to_string(),
                        level: 0.0,
                        error: None,
                    });

                    match load_and_decode_preview(&paths, &engine, &req) {
                        Ok(pcm) => {
                            if generation.load(Ordering::SeqCst) != start_gen {
                                continue;
                            }
                            match open_best_audio_sink(pcm.sample_rate, pcm.channels) {
                                Ok(sink) => {
                                    let dur = format_mm_ss(pcm.duration_secs());
                                    let new_session = ActiveTrackSession {
                                        req: req.clone(),
                                        pcm,
                                        cursor_frame: 0,
                                        paused: false,
                                        sink,
                                        last_ui_emit: Instant::now()
                                            .checked_sub(Duration::from_millis(200))
                                            .unwrap_or_else(Instant::now),
                                        smoothed_level: 0.25,
                                    };
                                    on_update(PreviewUiUpdate {
                                        active: true,
                                        playing: true,
                                        buffering: false,
                                        release_id: req.release_id as i32,
                                        disc_number: req.disc_number,
                                        track_number: req.track_number,
                                        track_title: req.track_title,
                                        artist_name: req.artist_name,
                                        release_title: req.release_title,
                                        progress: 0.0,
                                        elapsed_label: "0:00".to_string(),
                                        duration_label: dur,
                                        level: 0.25,
                                        error: None,
                                    });
                                    session = Some(new_session);
                                }
                                Err(err) => {
                                    on_update(PreviewUiUpdate {
                                        active: false,
                                        error: Some(err),
                                        ..PreviewUiUpdate::default()
                                    });
                                }
                            }
                        }
                        Err(err) => {
                            if generation.load(Ordering::SeqCst) != start_gen {
                                continue;
                            }
                            on_update(PreviewUiUpdate {
                                active: false,
                                error: Some(err),
                                ..PreviewUiUpdate::default()
                            });
                        }
                    }
                }
                PreviewCommand::TogglePause => {
                    if let Some(ref mut cur) = session {
                        cur.paused = !cur.paused;
                        if cur.paused {
                            cur.sink.flush();
                            cur.smoothed_level = 0.0;
                        }
                        emit_session_update(cur, &on_update);
                    }
                }
                PreviewCommand::Seek(frac) => {
                    if let Some(ref mut cur) = session {
                        let total = cur.pcm.total_frames();
                        cur.cursor_frame = ((total as f32) * frac.clamp(0.0, 0.995)) as usize;
                        cur.sink.flush();
                        emit_session_update(cur, &on_update);
                    }
                }
                PreviewCommand::Stop => {
                    if let Some(mut cur) = session.take() {
                        cur.sink.flush();
                    }
                    on_update(PreviewUiUpdate::default());
                }
            }
            continue;
        }

        // Write next ~1024-frame PCM slice to the active audio sink
        let mut finished = false;
        let mut sink_err: Option<String> = None;

        if let Some(ref mut cur) = session
            && !cur.paused
        {
            let ch = usize::from(cur.pcm.channels.max(1));
            let total_frames = cur.pcm.total_frames();
            if cur.cursor_frame >= total_frames {
                cur.sink.drain();
                finished = true;
            } else {
                let chunk_frames = 1024usize.min(total_frames - cur.cursor_frame);
                let start_sample = cur.cursor_frame * ch;
                let end_sample = start_sample + chunk_frames * ch;
                let raw_slice = &cur.pcm.samples[start_sample..end_sample];

                let vol = f32::from(volume_pct.load(Ordering::Relaxed)) / 100.0;
                // Perceptual cubic-ish gain curve so 50% sounds naturally half-loud
                let gain = (vol * vol * 0.65 + vol * 0.35).clamp(0.0, 1.0);

                let mut s16_buf = Vec::with_capacity(raw_slice.len());
                let mut sum_sq = 0.0f32;
                for &sample in raw_slice {
                    let scaled = (sample * gain).clamp(-1.0, 1.0);
                    sum_sq += scaled * scaled;
                    s16_buf.push((scaled * 32767.0) as i16);
                }

                let rms = if raw_slice.is_empty() {
                    0.0
                } else {
                    (sum_sq / (raw_slice.len() as f32)).sqrt()
                };
                let visual_level = (rms * 3.6).clamp(0.06, 1.0);
                cur.smoothed_level = cur.smoothed_level * 0.55 + visual_level * 0.45;

                if let Err(e) = cur.sink.write_s16le(&s16_buf, chunk_frames) {
                    sink_err = Some(e);
                } else {
                    cur.cursor_frame += chunk_frames;
                    if cur.last_ui_emit.elapsed() >= Duration::from_millis(65) {
                        cur.last_ui_emit = Instant::now();
                        emit_session_update(cur, &on_update);
                    }
                }
            }
        }

        if let Some(err) = sink_err {
            session = None;
            on_update(PreviewUiUpdate {
                active: false,
                error: Some(err),
                ..PreviewUiUpdate::default()
            });
        } else if finished && let Some(cur) = session.take() {
            // Keep the mini-player bar visible in a clean paused-at-start state so the user
            // can replay with a single click or dismiss it.
            let dur = format_mm_ss(cur.pcm.duration_secs());
            on_update(PreviewUiUpdate {
                active: true,
                playing: false,
                buffering: false,
                release_id: cur.req.release_id as i32,
                disc_number: cur.req.disc_number,
                track_number: cur.req.track_number,
                track_title: cur.req.track_title.clone(),
                artist_name: cur.req.artist_name.clone(),
                release_title: cur.req.release_title.clone(),
                progress: 0.0,
                elapsed_label: "0:00".to_string(),
                duration_label: dur,
                level: 0.0,
                error: None,
            });
            // Re-park at frame 0 paused so clicking Play replays immediately from cache!
            session = Some(ActiveTrackSession {
                cursor_frame: 0,
                paused: true,
                smoothed_level: 0.0,
                ..cur
            });
        }
    }
}

fn emit_session_update<F>(cur: &ActiveTrackSession, on_update: &Arc<F>)
where
    F: Fn(PreviewUiUpdate) + Send + Sync + 'static,
{
    let total = cur.pcm.total_frames().max(1);
    let progress = ((cur.cursor_frame as f32) / (total as f32)).clamp(0.0, 1.0);
    let elapsed_secs = if cur.pcm.sample_rate > 0 {
        (cur.cursor_frame as f32) / (cur.pcm.sample_rate as f32)
    } else {
        progress * 30.0
    };

    on_update(PreviewUiUpdate {
        active: true,
        playing: !cur.paused,
        buffering: false,
        release_id: cur.req.release_id as i32,
        disc_number: cur.req.disc_number,
        track_number: cur.req.track_number,
        track_title: cur.req.track_title.clone(),
        artist_name: cur.req.artist_name.clone(),
        release_title: cur.req.release_title.clone(),
        progress,
        elapsed_label: format_mm_ss(elapsed_secs),
        duration_label: format_mm_ss(cur.pcm.duration_secs()),
        level: if cur.paused { 0.0 } else { cur.smoothed_level },
        error: None,
    });
}

fn load_and_decode_preview(
    paths: &AppPaths,
    engine: &SyncEngine,
    req: &PreviewRequest,
) -> Result<DecodedPcm, String> {
    let cache_key = format!(
        "rel_{}_d{}_t{}",
        req.release_id, req.disc_number, req.track_number
    );

    let mut target_url = req.preview_url.clone();
    if is_deezer_preview_url_expired(&target_url)
        && let Some(fresh) = refresh_single_track_url(&paths.db_path, engine, req)
    {
        target_url = fresh;
    }

    let mp3_bytes =
        match engine.fetch_preview_mp3_cached(&paths.preview_cache_dir, &cache_key, &target_url) {
            Ok(bytes) => bytes,
            Err(first_err) => {
                // Signed Deezer CDN URLs can expire; refresh tracklist once and retry!
                if let Some(fresh_url) = refresh_single_track_url(&paths.db_path, engine, req) {
                    engine
                        .fetch_preview_mp3_cached(&paths.preview_cache_dir, &cache_key, &fresh_url)
                        .map_err(|e| format!("Preview download failed: {e}"))?
                } else {
                    return Err(format!("Preview download failed: {first_err}"));
                }
            }
        };

    decode_mp3_bytes(&mp3_bytes)
}

fn refresh_single_track_url(
    db_path: &Path,
    engine: &SyncEngine,
    req: &PreviewRequest,
) -> Option<String> {
    let mut db = Database::open(db_path).ok()?;
    let rel = db.get_release(req.release_id).ok()??;
    let fresh_tracks = engine.refresh_release_tracks(&mut db, &rel).ok()?;
    fresh_tracks
        .into_iter()
        .find(|t| t.disc_number == req.disc_number && t.track_number == req.track_number)
        .and_then(|t| t.preview_url)
        .filter(|u| !u.is_empty())
}

fn decode_mp3_bytes(mp3: &[u8]) -> Result<DecodedPcm, String> {
    if mp3.len() < 128 {
        return Err("Preview audio buffer is empty or truncated".to_string());
    }

    let decoded = nanomp3::decode_all::<f32>(mp3);
    if decoded.samples.is_empty() || decoded.sample_rate == 0 {
        return Err("Unable to decode MP3 preview stream".to_string());
    }

    let channels = decoded.channels.map_or(2, |c| c.num()).max(1);
    Ok(DecodedPcm {
        sample_rate: decoded.sample_rate,
        channels,
        samples: decoded.samples,
    })
}

fn format_mm_ss(secs: f32) -> String {
    let total = secs.max(0.0).round() as u32;
    format!("{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_valid_mpeg1_layer3_frames_and_formats_timestamps() {
        // Minimal valid silent MPEG-1 Layer III 128kbps 44.1kHz stereo frames (417 bytes/frame)
        let mut frame = vec![0u8; 417];
        frame[0] = 0xFF;
        frame[1] = 0xFB;
        frame[2] = 0x90;
        frame[3] = 0x64;

        let mut mp3 = Vec::with_capacity(417 * 8);
        for _ in 0..8 {
            mp3.extend_from_slice(&frame);
        }

        let decoded = decode_mp3_bytes(&mp3).expect("valid MP3 stream decodes");
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 2);
        assert!(decoded.total_frames() >= 1152);
        assert_eq!(format_mm_ss(0.0), "0:00");
        assert_eq!(format_mm_ss(29.8), "0:30");
    }
}
