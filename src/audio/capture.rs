//! WASAPI loopback capture (SPEC §6.2): process loopback on the Spotify
//! process tree, or the default render device as fallback.

use std::mem::ManuallyDrop;
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{BLOB, CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{GUID, Interface, Ref, Result, implement};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
/// 200 ms shared-mode buffer (100 ns units).
const BUFFER_DURATION: i64 = 2_000_000;
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Only the Spotify process tree.
    Process,
    /// Everything on the default output device.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleType {
    F32,
    I16,
    I32,
}

#[derive(Debug, Clone, Copy)]
pub struct Format {
    pub sample_rate: u32,
    channels: usize,
    sample: SampleType,
}

pub struct Capture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    pub format: Format,
    pub source: Source,
    event: Option<HANDLE>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: stopping a started client and closing our own event handle.
        unsafe {
            let _ = self.client.Stop();
            if let Some(e) = self.event.take() {
                let _ = CloseHandle(e);
            }
        }
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    done: mpsc::Sender<()>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> Result<()> {
        let _ = self.done.send(());
        Ok(())
    }
}

impl Capture {
    /// Process loopback on `pid` including its child processes. Requires
    /// Windows 10 2004+; fails cleanly otherwise.
    pub fn process(pid: u32) -> Result<Self> {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        // The PROPVARIANT only borrows `params`; ManuallyDrop prevents any
        // attempt to free the blob.
        let prop = ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
                        },
                    },
                }),
            },
        });

        let (tx, rx) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationHandler { done: tx }.into();
        // SAFETY: `prop` and `params` outlive the activation (we wait for it below).
        let client: IAudioClient = unsafe {
            let op = ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*prop as *const PROPVARIANT),
                &handler,
            )?;
            rx.recv_timeout(ACTIVATION_TIMEOUT).map_err(|_| {
                windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)
            })?;
            let mut hr = windows::core::HRESULT(0);
            let mut unknown = None;
            op.GetActivateResult(&mut hr, &mut unknown)?;
            hr.ok()?;
            unknown.ok_or_else(windows::core::Error::empty)?.cast()?
        };

        // Process loopback has no mix format; we request float stereo 48 kHz.
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // SAFETY: valid client; format and event live long enough.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_DURATION,
                0,
                &format,
                None,
            )?;
            let event = CreateEventW(None, false, false, None)?;
            client.SetEventHandle(event)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self {
                client,
                capture,
                format: Format {
                    sample_rate: 48_000,
                    channels: 2,
                    sample: SampleType::F32,
                },
                source: Source::Process,
                event: Some(event),
            })
        }
    }

    /// Loopback of the default render device (fallback).
    pub fn system() -> Result<Self> {
        // SAFETY: standard WASAPI setup; the mix format is freed after use.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mix = client.GetMixFormat()?;
            let format = parse_format(mix);
            let init = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                BUFFER_DURATION,
                0,
                mix,
                None,
            );
            CoTaskMemFree(Some(mix as *const _));
            init?;
            let format = format.ok_or_else(windows::core::Error::empty)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self {
                client,
                capture,
                format,
                source: Source::System,
                event: None,
            })
        }
    }

    /// Waits for data (event or ~10 ms polling).
    pub fn wait(&self) {
        match self.event {
            // SAFETY: our own event handle.
            Some(e) => unsafe {
                let r = WaitForSingleObject(e, 100);
                let _ = r == WAIT_OBJECT_0;
            },
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    /// Appends all available audio as mono samples to `out`.
    pub fn read(&self, out: &mut Vec<f32>) -> Result<()> {
        // SAFETY: GetBuffer/ReleaseBuffer pairs; the buffer is only read
        // within its reported size.
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    out.extend(std::iter::repeat_n(0.0, n));
                } else {
                    let ch = self.format.channels.max(1);
                    let bytes = match self.format.sample {
                        SampleType::F32 | SampleType::I32 => 4,
                        SampleType::I16 => 2,
                    };
                    let buf = std::slice::from_raw_parts(data, n * ch * bytes);
                    mix_to_mono(buf, self.format.sample, ch, out);
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

/// Reads a (possibly extensible) WAVEFORMATEX.
///
/// # Safety
/// `fmt` must point to a valid WAVEFORMATEX (and WAVEFORMATEXTENSIBLE if the
/// tag says so).
unsafe fn parse_format(fmt: *const WAVEFORMATEX) -> Option<Format> {
    // SAFETY: guaranteed by the caller.
    let f = unsafe { std::ptr::read_unaligned(fmt) };
    let tag = f.wFormatTag;
    let bits = f.wBitsPerSample;
    let sample = match tag {
        WAVE_FORMAT_IEEE_FLOAT if bits == 32 => SampleType::F32,
        WAVE_FORMAT_PCM if bits == 16 => SampleType::I16,
        WAVE_FORMAT_EXTENSIBLE => {
            // SAFETY: extensible tag implies the larger struct.
            let ext = unsafe { std::ptr::read_unaligned(fmt.cast::<WAVEFORMATEXTENSIBLE>()) };
            let sub = ext.SubFormat;
            match (sub, bits) {
                (s, 32) if s == SUBTYPE_FLOAT => SampleType::F32,
                (s, 16) if s == SUBTYPE_PCM => SampleType::I16,
                (s, 32) if s == SUBTYPE_PCM => SampleType::I32,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(Format {
        sample_rate: f.nSamplesPerSec,
        channels: f.nChannels as usize,
        sample,
    })
}

/// Interleaved little-endian samples -> mono f32.
fn mix_to_mono(buf: &[u8], sample: SampleType, channels: usize, out: &mut Vec<f32>) {
    let bytes = match sample {
        SampleType::F32 | SampleType::I32 => 4,
        SampleType::I16 => 2,
    };
    let frame = bytes * channels;
    let scale = 1.0 / channels as f32;
    for f in buf.chunks_exact(frame) {
        let mut sum = 0.0f32;
        for s in f.chunks_exact(bytes) {
            sum += match sample {
                SampleType::F32 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                SampleType::I16 => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                SampleType::I32 => {
                    i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0
                }
            };
        }
        out.push(sum * scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual diagnostic: `cargo test capture_diag -- --ignored --nocapture`
    /// with Spotify playing.
    #[test]
    #[ignore]
    fn capture_diag() {
        let _ = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        };
        let pid = crate::audio::process::spotify_root_pid();
        println!("spotify pid {pid:?}");
        for (name, cap) in [
            ("process", pid.map(Capture::process)),
            ("system", Some(Capture::system())),
        ] {
            let Some(cap) = cap else { continue };
            let cap = match cap {
                Ok(c) => c,
                Err(e) => {
                    println!("{name}: error {e}");
                    continue;
                }
            };
            let mut total = 0usize;
            let mut sumsq = 0.0f64;
            let end = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < end {
                cap.wait();
                let mut buf = Vec::new();
                cap.read(&mut buf).unwrap();
                total += buf.len();
                sumsq += buf.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
            }
            println!(
                "{name}: {total} samples, rms {:.5}",
                (sumsq / total.max(1) as f64).sqrt()
            );
        }

        // Beat statistics on 6 s of the Spotify process.
        let Some(Ok(cap)) = pid.map(Capture::process) else {
            return;
        };
        let mut samples = Vec::new();
        let end = std::time::Instant::now() + Duration::from_secs(6);
        while std::time::Instant::now() < end {
            cap.wait();
            cap.read(&mut samples).unwrap();
        }
        for sens in [0.6f32, 1.0] {
            let mut d = crate::audio::beat::BeatDetector::new(48_000);
            d.set_sensitivity(sens);
            let mut beats = Vec::new();
            let mut fluxes = Vec::new();
            let mut hop = 0;
            d.push(&samples, |r| {
                hop += 1;
                fluxes.push((r.flux, r.threshold));
                if r.beat {
                    beats.push(hop as f32 * 512.0 / 48_000.0);
                }
            });
            let mut sorted: Vec<f32> = fluxes.iter().map(|f| f.0).collect();
            sorted.sort_by(f32::total_cmp);
            let pct = |p: f32| sorted[((sorted.len() - 1) as f32 * p) as usize];
            let thr_med = {
                let mut t: Vec<f32> = fluxes.iter().map(|f| f.1).collect();
                t.sort_by(f32::total_cmp);
                t[t.len() / 2]
            };
            println!(
                "sens {sens}: {} beats {beats:.2?}\n  flux p50 {:.2} p90 {:.2} p97 {:.2} p99 {:.2} max {:.2}, threshold median {:.2}",
                beats.len(),
                pct(0.5),
                pct(0.9),
                pct(0.97),
                pct(0.99),
                pct(1.0),
                thr_med
            );
        }
    }

    #[test]
    fn mono_mix() {
        let mut out = Vec::new();
        let stereo: Vec<u8> = [0.5f32, -0.5, 1.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        mix_to_mono(&stereo, SampleType::F32, 2, &mut out);
        assert_eq!(out, vec![0.0, 0.5]);

        out.clear();
        let pcm: Vec<u8> = [16384i16, 16384]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        mix_to_mono(&pcm, SampleType::I16, 2, &mut out);
        assert_eq!(out, vec![0.5]);
    }
}
