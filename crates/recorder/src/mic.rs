// The microphone as a sound source of a recording: a WASAPI capture of one
// input in shared mode, which Windows converts to 48 kHz stereo float. It is
// opened only for a recording the user asked the microphone into, on the
// recording's thread, and closed with it. Windows shows its own microphone
// indicator while it is open and applies its privacy switch: a refusal is an
// error here, and the recording goes on without the track's sound.

use windows::core::{Result as WinResult, HSTRING};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::rec::{Sound, RATE};

const CHANNELS: u16 = 2;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const BUFFERFLAGS_SILENT: u32 = 2;
/// Room for what is said between two reads, in 100 ns units (0.2 s).
const BUFFER: i64 = 2_000_000;

pub struct Mic {
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

/// Opens the input with this id (`svc_audio::devices`), or the default one for
/// an empty id. Listening starts at once.
pub fn open(id: &str) -> Result<Mic, String> {
    unsafe { begin(id) }.map_err(|e| e.message())
}

unsafe fn begin(id: &str) -> WinResult<Mic> {
    unsafe {
        let all: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = if id.is_empty() { all.GetDefaultAudioEndpoint(eCapture, eConsole)? } else { all.GetDevice(&HSTRING::from(id))? };
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let asked = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: CHANNELS,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * u32::from(CHANNELS) * 4,
            nBlockAlign: CHANNELS * 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, BUFFER, 0, &asked, None)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        Ok(Mic { client, capture })
    }
}

impl Sound for Mic {
    fn rate(&self) -> u32 {
        RATE
    }

    fn channels(&self) -> u16 {
        CHANNELS
    }

    fn read(&mut self, out: &mut Vec<f32>) -> Result<usize, String> {
        let mut total = 0;
        let drained: WinResult<()> = (|| unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let (at, samples) = (out.len(), frames as usize * usize::from(CHANNELS));
                out.resize(at + samples, 0.0);
                if flags & BUFFERFLAGS_SILENT == 0 && !data.is_null() {
                    std::ptr::copy_nonoverlapping(data, out[at..].as_mut_ptr().cast::<u8>(), samples * 4);
                }
                self.capture.ReleaseBuffer(frames)?;
                total += frames as usize;
            }
            Ok(())
        })();
        drained.map(|_| total).map_err(|e| e.message())
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
