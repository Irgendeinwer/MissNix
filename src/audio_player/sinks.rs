use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::process::{Child, Command, Stdio};

pub(super) trait PcmSink: Send {
    fn write_s16le(&mut self, interleaved: &[i16], frames: usize) -> Result<(), String>;
    fn flush(&mut self);
    fn drain(&mut self);
}

pub(super) fn open_best_audio_sink(
    sample_rate: u32,
    channels: u8,
) -> Result<Box<dyn PcmSink>, String> {
    if let Ok(pulse) = PulseSimpleSink::open(sample_rate, channels) {
        return Ok(Box::new(pulse));
    }
    if let Ok(alsa) = AlsaPcmSink::open(sample_rate, channels) {
        return Ok(Box::new(alsa));
    }
    if let Ok(pipe) = SubprocessPcmSink::open(sample_rate, channels) {
        return Ok(Box::new(pipe));
    }
    Err("No audio output backend available (checked PulseAudio/PipeWire, ALSA, pw-cat/sox)".into())
}

#[repr(C)]
struct PaSampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct PaBufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

type PaSimpleNewFn = unsafe extern "C" fn(
    server: *const c_char,
    name: *const c_char,
    dir: c_int,
    dev: *const c_char,
    stream_name: *const c_char,
    ss: *const PaSampleSpec,
    map: *const c_void,
    attr: *const PaBufferAttr,
    error: *mut c_int,
) -> *mut c_void;
type PaSimpleWriteFn = unsafe extern "C" fn(
    s: *mut c_void,
    data: *const c_void,
    bytes: usize,
    error: *mut c_int,
) -> c_int;
type PaSimpleFlushFn = unsafe extern "C" fn(s: *mut c_void, error: *mut c_int) -> c_int;
type PaSimpleDrainFn = unsafe extern "C" fn(s: *mut c_void, error: *mut c_int) -> c_int;
type PaSimpleFreeFn = unsafe extern "C" fn(s: *mut c_void);

struct PulseSimpleSink {
    _lib: libloading::Library,
    handle: *mut c_void,
    fn_write: PaSimpleWriteFn,
    fn_flush: PaSimpleFlushFn,
    fn_drain: PaSimpleDrainFn,
    fn_free: PaSimpleFreeFn,
}

// Safety: `pa_simple` handle is accessed exclusively from the single dedicated audio worker thread.
unsafe impl Send for PulseSimpleSink {}

impl PulseSimpleSink {
    fn open(sample_rate: u32, channels: u8) -> Result<Self, String> {
        const PA_STREAM_PLAYBACK: c_int = 1;
        const PA_SAMPLE_S16LE: c_int = 3;

        unsafe {
            let lib = libloading::Library::new("libpulse-simple.so.0")
                .or_else(|_| libloading::Library::new("libpulse-simple.so"))
                .map_err(|e| e.to_string())?;

            let fn_new: PaSimpleNewFn = *lib.get(c"pa_simple_new").map_err(|e| e.to_string())?;
            let fn_write: PaSimpleWriteFn =
                *lib.get(c"pa_simple_write").map_err(|e| e.to_string())?;
            let fn_flush: PaSimpleFlushFn =
                *lib.get(c"pa_simple_flush").map_err(|e| e.to_string())?;
            let fn_drain: PaSimpleDrainFn =
                *lib.get(c"pa_simple_drain").map_err(|e| e.to_string())?;
            let fn_free: PaSimpleFreeFn = *lib.get(c"pa_simple_free").map_err(|e| e.to_string())?;

            let spec = PaSampleSpec {
                format: PA_SAMPLE_S16LE,
                rate: sample_rate,
                channels,
            };
            let bytes_per_sec = sample_rate
                .saturating_mul(u32::from(channels))
                .saturating_mul(2);
            let attr = PaBufferAttr {
                maxlength: u32::MAX,
                tlength: (bytes_per_sec / 10).max(4096), // ~100ms responsive buffer
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: u32::MAX,
            };
            let mut err: c_int = 0;
            let handle = fn_new(
                std::ptr::null(),
                c"MissNix".as_ptr(),
                PA_STREAM_PLAYBACK,
                std::ptr::null(),
                c"30s Preview".as_ptr(),
                &spec,
                std::ptr::null(),
                &attr,
                &mut err,
            );
            if handle.is_null() {
                return Err(format!("pa_simple_new failed (code {err})"));
            }

            Ok(Self {
                _lib: lib,
                handle,
                fn_write,
                fn_flush,
                fn_drain,
                fn_free,
            })
        }
    }
}

impl PcmSink for PulseSimpleSink {
    fn write_s16le(&mut self, interleaved: &[i16], _frames: usize) -> Result<(), String> {
        let mut err: c_int = 0;
        let byte_len = std::mem::size_of_val(interleaved);
        let rc = unsafe {
            (self.fn_write)(
                self.handle,
                interleaved.as_ptr().cast::<c_void>(),
                byte_len,
                &mut err,
            )
        };
        if rc < 0 {
            Err(format!("PulseAudio write error ({err})"))
        } else {
            Ok(())
        }
    }

    fn flush(&mut self) {
        let mut err: c_int = 0;
        unsafe {
            let _ = (self.fn_flush)(self.handle, &mut err);
        }
    }

    fn drain(&mut self) {
        let mut err: c_int = 0;
        unsafe {
            let _ = (self.fn_drain)(self.handle, &mut err);
        }
    }
}

impl Drop for PulseSimpleSink {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            let mut err: c_int = 0;
            unsafe {
                let _ = (self.fn_flush)(self.handle, &mut err);
                (self.fn_free)(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

type SndPcmOpenFn = unsafe extern "C" fn(
    pcmp: *mut *mut c_void,
    name: *const c_char,
    stream: c_int,
    mode: c_int,
) -> c_int;
type SndPcmSetParamsFn = unsafe extern "C" fn(
    pcm: *mut c_void,
    format: c_int,
    access: c_int,
    channels: c_uint,
    rate: c_uint,
    soft_resample: c_int,
    latency: c_uint,
) -> c_int;
type SndPcmWriteiFn =
    unsafe extern "C" fn(pcm: *mut c_void, buffer: *const c_void, size: c_ulong) -> c_long;
type SndPcmRecoverFn = unsafe extern "C" fn(pcm: *mut c_void, err: c_int, silent: c_int) -> c_int;
type SndPcmPrepareFn = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;
type SndPcmDropFn = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;
type SndPcmDrainFn = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;
type SndPcmCloseFn = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;

struct AlsaPcmSink {
    _lib: libloading::Library,
    pcm: *mut c_void,
    fn_writei: SndPcmWriteiFn,
    fn_recover: SndPcmRecoverFn,
    fn_prepare: SndPcmPrepareFn,
    fn_drop: SndPcmDropFn,
    fn_drain: SndPcmDrainFn,
    fn_close: SndPcmCloseFn,
}

unsafe impl Send for AlsaPcmSink {}

impl AlsaPcmSink {
    fn open(sample_rate: u32, channels: u8) -> Result<Self, String> {
        const SND_PCM_STREAM_PLAYBACK: c_int = 0;
        const SND_PCM_FORMAT_S16_LE: c_int = 2;
        const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

        unsafe {
            let lib = libloading::Library::new("libasound.so.2")
                .or_else(|_| libloading::Library::new("libasound.so"))
                .map_err(|e| e.to_string())?;

            let fn_open: SndPcmOpenFn = *lib.get(c"snd_pcm_open").map_err(|e| e.to_string())?;
            let fn_set_params: SndPcmSetParamsFn =
                *lib.get(c"snd_pcm_set_params").map_err(|e| e.to_string())?;
            let fn_writei: SndPcmWriteiFn =
                *lib.get(c"snd_pcm_writei").map_err(|e| e.to_string())?;
            let fn_recover: SndPcmRecoverFn =
                *lib.get(c"snd_pcm_recover").map_err(|e| e.to_string())?;
            let fn_prepare: SndPcmPrepareFn =
                *lib.get(c"snd_pcm_prepare").map_err(|e| e.to_string())?;
            let fn_drop: SndPcmDropFn = *lib.get(c"snd_pcm_drop").map_err(|e| e.to_string())?;
            let fn_drain: SndPcmDrainFn = *lib.get(c"snd_pcm_drain").map_err(|e| e.to_string())?;
            let fn_close: SndPcmCloseFn = *lib.get(c"snd_pcm_close").map_err(|e| e.to_string())?;

            let mut pcm: *mut c_void = std::ptr::null_mut();
            let rc = fn_open(&mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if rc < 0 || pcm.is_null() {
                return Err(format!("snd_pcm_open failed ({rc})"));
            }

            let rc_params = fn_set_params(
                pcm,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                c_uint::from(channels),
                sample_rate,
                1,
                90_000,
            );
            if rc_params < 0 {
                let _ = fn_close(pcm);
                return Err(format!("snd_pcm_set_params failed ({rc_params})"));
            }

            Ok(Self {
                _lib: lib,
                pcm,
                fn_writei,
                fn_recover,
                fn_prepare,
                fn_drop,
                fn_drain,
                fn_close,
            })
        }
    }
}

impl PcmSink for AlsaPcmSink {
    fn write_s16le(&mut self, interleaved: &[i16], frames: usize) -> Result<(), String> {
        let ch = interleaved.len().checked_div(frames).unwrap_or(2);
        let mut written = 0usize;
        while written < frames {
            let rem = frames - written;
            let ptr = unsafe { interleaved.as_ptr().add(written * ch).cast::<c_void>() };
            let res = unsafe { (self.fn_writei)(self.pcm, ptr, rem as c_ulong) };
            if res < 0 {
                let rec = unsafe { (self.fn_recover)(self.pcm, res as c_int, 1) };
                if rec < 0 {
                    return Err(format!("ALSA playback error ({rec})"));
                }
            } else {
                written += res as usize;
            }
        }
        Ok(())
    }

    fn flush(&mut self) {
        unsafe {
            let _ = (self.fn_drop)(self.pcm);
            let _ = (self.fn_prepare)(self.pcm);
        }
    }

    fn drain(&mut self) {
        unsafe {
            let _ = (self.fn_drain)(self.pcm);
            let _ = (self.fn_prepare)(self.pcm);
        }
    }
}

impl Drop for AlsaPcmSink {
    fn drop(&mut self) {
        if !self.pcm.is_null() {
            unsafe {
                let _ = (self.fn_drop)(self.pcm);
                let _ = (self.fn_close)(self.pcm);
            }
            self.pcm = std::ptr::null_mut();
        }
    }
}

struct SubprocessPcmSink {
    sample_rate: u32,
    channels: u8,
    child: Option<Child>,
}

impl SubprocessPcmSink {
    fn open(sample_rate: u32, channels: u8) -> Result<Self, String> {
        let mut s = Self {
            sample_rate,
            channels,
            child: None,
        };
        s.ensure_spawned()?;
        Ok(s)
    }

    fn ensure_spawned(&mut self) -> Result<(), String> {
        if self.child.is_some() {
            return Ok(());
        }
        let rate_s = self.sample_rate.to_string();
        let ch_s = self.channels.to_string();

        let candidates: [(&str, &[&str]); 3] = [
            (
                "pw-cat",
                &[
                    "--playback",
                    "--raw",
                    "--format",
                    "s16",
                    "--rate",
                    &rate_s,
                    "--channels",
                    &ch_s,
                    "-",
                ],
            ),
            (
                "paplay",
                &[
                    "--raw",
                    "--format=s16le",
                    &format!("--rate={rate_s}"),
                    &format!("--channels={ch_s}"),
                ],
            ),
            (
                "play",
                &[
                    "-q", "-t", "raw", "-r", &rate_s, "-e", "signed", "-b", "16", "-c", &ch_s, "-",
                ],
            ),
        ];

        for (bin, args) in candidates {
            if let Ok(child) = Command::new(bin)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                self.child = Some(child);
                return Ok(());
            }
        }
        Err("Could not spawn fallback audio player".to_string())
    }
}

impl PcmSink for SubprocessPcmSink {
    fn write_s16le(&mut self, interleaved: &[i16], _frames: usize) -> Result<(), String> {
        use std::io::Write;
        self.ensure_spawned()?;
        let Some(ref mut child) = self.child else {
            return Err("Audio subprocess unavailable".to_string());
        };
        let Some(ref mut stdin) = child.stdin else {
            return Err("Audio subprocess stdin closed".to_string());
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                interleaved.as_ptr().cast::<u8>(),
                std::mem::size_of_val(interleaved),
            )
        };
        stdin
            .write_all(bytes)
            .map_err(|e| format!("Audio stream write error: {e}"))
    }

    fn flush(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn drain(&mut self) {
        if let Some(mut child) = self.child.take() {
            drop(child.stdin.take());
            let _ = child.wait();
        }
    }
}

impl Drop for SubprocessPcmSink {
    fn drop(&mut self) {
        self.flush();
    }
}
