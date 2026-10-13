//! Áudio: chime de notificação (síntese igual ao `playNotificationSound` do renderer) e
//! captura do microfone para transcrição (16 kHz mono). Tudo atrás da feature `audio` (cpal);
//! sem ela, o som é mudo e o microfone aparece como indisponível.

// Sem a feature, os auxiliares de síntese/reamostragem só são usados pelos testes.
#![cfg_attr(not(feature = "audio"), allow(dead_code))]

/// Taxa exigida pelo whisper.cpp.
pub const WHISPER_RATE: u32 = 16_000;

/// Sintetiza o chime de 2 tons (D5 587 Hz → A5 880 Hz) em mono, na taxa `rate`:
/// seno 587,33→880 Hz (rampa exponencial em 0,12 s) + triângulo 880 Hz a partir de 0,06 s,
/// ganho 0,001 → 0,2 (0,04 s) → 0,001 (0,45 s), exatamente como no Web Audio.
pub fn chime_samples(rate: u32) -> Vec<f32> {
    let rate = rate.max(1) as f64;
    let total = (0.45 * rate).ceil() as usize;
    let mut out = Vec::with_capacity(total);
    let (mut ph1, mut ph2) = (0.0f64, 0.0f64);
    for i in 0..total {
        let t = i as f64 / rate;
        // Frequência do seno: rampa exponencial até 0,12 s, depois constante.
        let f1 = if t < 0.12 { 587.33 * (880.0f64 / 587.33).powf(t / 0.12) } else { 880.0 };
        ph1 += f1 / rate;
        let s1 = (std::f64::consts::TAU * ph1).sin();
        let s2 = if t >= 0.06 {
            ph2 += 880.0 / rate;
            let p = ph2.fract();
            // Onda triangular (começa em 0, sobe até 1 em 1/4 do período).
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        } else {
            0.0
        };
        let g = if t < 0.04 { 0.001 * (0.2f64 / 0.001).powf(t / 0.04) } else { 0.2 * (0.001f64 / 0.2).powf((t - 0.04) / 0.41) };
        out.push(((s1 + s2) * g) as f32);
    }
    out
}

/// Mistura canais intercalados em mono.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved.chunks(channels).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect()
}

/// Reamostra mono `from_rate` → `to_rate`. Ao reduzir a taxa, faz média na janela de cada
/// amostra de saída (filtro anti-aliasing simples); ao aumentar, interpola linearmente.
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = i as f64 * ratio;
        if ratio > 1.0 {
            let start = (center - ratio / 2.0).max(0.0).floor() as usize;
            let end = ((center + ratio / 2.0).ceil() as usize).min(input.len()).max(start + 1);
            let slice = &input[start.min(input.len() - 1)..end];
            out.push(slice.iter().sum::<f32>() / slice.len() as f32);
        } else {
            let j = center.floor() as usize;
            let frac = (center - j as f64) as f32;
            let a = input[j.min(input.len() - 1)];
            let b = input[(j + 1).min(input.len() - 1)];
            out.push(a + (b - a) * frac);
        }
    }
    out
}

/// Converte captura (intercalada, `channels`, `rate`) para 16 kHz mono f32 −1..1.
pub fn to_whisper_pcm(interleaved: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    resample(&downmix(interleaved, channels), rate, WHISPER_RATE).into_iter().map(|s| s.clamp(-1.0, 1.0)).collect()
}

#[cfg(feature = "audio")]
mod imp {
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
    use parking_lot::Mutex;

    pub const AVAILABLE: bool = true;

    /// Toca o chime numa thread própria (o stream do cpal não é `Send` em todas as plataformas).
    pub fn play_chime() {
        let _ = std::thread::Builder::new().name("chime".into()).spawn(|| {
            if let Err(e) = play_blocking() {
                log::warn!("Erro ao reproduzir notificação sonora: {e}");
            }
        });
    }

    fn play_blocking() -> anyhow::Result<()> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("sem dispositivo de saída"))?;
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let samples = Arc::new(super::chime_samples(config.sample_rate));
        let stream = match format {
            SampleFormat::F32 => build_out::<f32>(&device, config, samples)?,
            SampleFormat::I16 => build_out::<i16>(&device, config, samples)?,
            SampleFormat::U16 => build_out::<u16>(&device, config, samples)?,
            SampleFormat::I32 => build_out::<i32>(&device, config, samples)?,
            SampleFormat::F64 => build_out::<f64>(&device, config, samples)?,
            other => anyhow::bail!("formato de saída não suportado: {other}"),
        };
        stream.play()?;
        std::thread::sleep(Duration::from_millis(600));
        drop(stream);
        Ok(())
    }

    fn build_out<T>(device: &cpal::Device, config: StreamConfig, samples: Arc<Vec<f32>>) -> anyhow::Result<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels as usize;
        let mut pos = 0usize;
        let stream = device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels.max(1)) {
                    let v = samples.get(pos).copied().unwrap_or(0.0);
                    pos += 1;
                    for s in frame {
                        *s = T::from_sample(v);
                    }
                }
            },
            |e| log::warn!("stream de saída: {e}"),
            None,
        )?;
        Ok(stream)
    }

    /// Gravação em andamento.
    pub struct Recorder {
        stop_tx: mpsc::Sender<()>,
        handle: Option<JoinHandle<()>>,
        buf: Arc<Mutex<Vec<f32>>>,
        channels: usize,
        rate: u32,
    }

    impl Recorder {
        /// Abre o microfone padrão e começa a capturar.
        pub fn start() -> Result<Recorder, String> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(usize, u32), String>>();
            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
            let buf_t = buf.clone();
            let handle = std::thread::Builder::new()
                .name("mic".into())
                .spawn(move || {
                    let stream = match open_input(buf_t) {
                        Ok((stream, ch, rate)) => {
                            let _ = ready_tx.send(Ok((ch, rate)));
                            stream
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = stop_rx.recv();
                    drop(stream);
                })
                .map_err(|e| e.to_string())?;
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok((channels, rate))) => Ok(Recorder { stop_tx, handle: Some(handle), buf, channels, rate }),
                Ok(Err(e)) => Err(e),
                Err(_) => Err("tempo esgotado ao abrir o microfone".into()),
            }
        }

        /// Para a captura e devolve PCM 16 kHz mono.
        pub fn stop(mut self) -> Vec<f32> {
            let _ = self.stop_tx.send(());
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            let raw = std::mem::take(&mut *self.buf.lock());
            super::to_whisper_pcm(&raw, self.channels, self.rate)
        }

        /// Descarta a gravação.
        pub fn cancel(mut self) {
            let _ = self.stop_tx.send(());
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn open_input(buf: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, usize, u32), String> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| "nenhum dispositivo de entrada encontrado".to_string())?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let (ch, rate) = (config.channels as usize, config.sample_rate);
        let stream = match format {
            SampleFormat::F32 => build_in::<f32>(&device, config, buf),
            SampleFormat::I16 => build_in::<i16>(&device, config, buf),
            SampleFormat::U16 => build_in::<u16>(&device, config, buf),
            SampleFormat::I32 => build_in::<i32>(&device, config, buf),
            SampleFormat::I8 => build_in::<i8>(&device, config, buf),
            SampleFormat::U8 => build_in::<u8>(&device, config, buf),
            SampleFormat::F64 => build_in::<f64>(&device, config, buf),
            other => Err(format!("formato de captura não suportado: {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, ch, rate))
    }

    fn build_in<T>(device: &cpal::Device, config: StreamConfig, buf: Arc<Mutex<Vec<f32>>>) -> Result<cpal::Stream, String>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        device
            .build_input_stream(
                config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    let mut b = buf.lock();
                    b.extend(data.iter().map(|s| s.to_sample::<f32>()));
                },
                |e| log::warn!("stream de entrada: {e}"),
                None,
            )
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(feature = "audio"))]
mod imp {
    pub const AVAILABLE: bool = false;

    pub fn play_chime() {}

    pub struct Recorder;

    impl Recorder {
        pub fn start() -> Result<Recorder, String> {
            Err("recurso de áudio desativado nesta compilação".into())
        }
        pub fn stop(self) -> Vec<f32> {
            Vec::new()
        }
        pub fn cancel(self) {}
    }
}

pub use imp::{play_chime, Recorder, AVAILABLE};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_envelope() {
        let s = chime_samples(48_000);
        assert_eq!(s.len(), (0.45f64 * 48_000.0).ceil() as usize);
        let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.15 && peak <= 0.41, "pico {peak}");
        // Começa quase mudo e termina quase mudo.
        assert!(s[..10].iter().all(|v| v.abs() < 0.01));
        assert!(s[s.len() - 10..].iter().all(|v| v.abs() < 0.01));
    }

    #[test]
    fn downmix_stereo() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn resample_lengths_and_dc() {
        let input = vec![0.25f32; 48_000];
        let out = resample(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-6));
        let up = resample(&[0.0, 1.0], 8_000, 16_000);
        assert_eq!(up.len(), 4);
        assert!((up[1] - 0.5).abs() < 1e-6);
        assert!(resample(&[], 44_100, 16_000).is_empty());
        assert_eq!(resample(&[0.3, 0.4], 16_000, 16_000), vec![0.3, 0.4]);
    }

    #[test]
    fn resample_keeps_low_frequency_sine() {
        // 440 Hz a 44,1 kHz → 16 kHz preserva amplitude.
        let input: Vec<f32> = (0..44_100).map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 44_100.0).sin()).collect();
        let out = to_whisper_pcm(&input, 1, 44_100);
        assert_eq!(out.len(), 16_000);
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.9 && peak <= 1.0, "pico {peak}");
    }
}
